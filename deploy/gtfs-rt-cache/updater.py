"""Refresh the public GTFS-RT cache using independently scheduled feed downloads."""

import hashlib
import logging
import math
import os
import resource
import tempfile
import time
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
from threading import BoundedSemaphore
from urllib.parse import urlsplit

import boto3
import requests
from botocore.config import Config
from google.transit import gtfs_realtime_pb2

BUCKET = os.environ.get('BUCKET_NAME', 'cardinal-carto-public')
RELEASES = os.environ.get('RELEASES_URL', f'https://{BUCKET}.t3.tigrisfiles.io/releases').rstrip('/')
INTERVAL = 60
RETRY_INTERVAL = 15
MAX_RETRY_INTERVAL = 300
MAX_BYTES = 32 * 1024 * 1024
WORKERS = int(os.environ.get('GTFS_RT_WORKERS', '64'))
VALIDATORS = int(os.environ.get('GTFS_RT_VALIDATORS', '1'))
LOG = logging.getLogger(__name__)


def object_key(url):
    return 'gtfs-rt/' + hashlib.sha256(url.encode('utf-8')).hexdigest() + '.pb'


def sources():
    latest = requests.get(f'{RELEASES}/latest.json', timeout=10)
    latest.raise_for_status()
    date = latest.json()['transit']
    manifest = requests.get(f'{RELEASES}/transit/{date}/realtime.json', timeout=15)
    manifest.raise_for_status()
    return {entry['url'] for entry in manifest.json()['feeds']}


@dataclass(frozen=True)
class RefreshResult:
    fetched_at: int | None = None
    failed_stage: str | None = None

    @property
    def succeeded(self):
        return self.failed_stage is None and self.fetched_at is not None


def validate(body):
    """Validate the complete protobuf before replacing the public object."""
    message = gtfs_realtime_pb2.FeedMessage.FromString(body.read())
    if not message.header.IsInitialized() or not message.header.gtfs_realtime_version:
        raise ValueError('invalid GTFS-RT message')
    if message.header.incrementality == gtfs_realtime_pb2.FeedHeader.DIFFERENTIAL:
        raise ValueError('differential feeds are unsupported')
    return message.header.timestamp if message.header.HasField('timestamp') else None


def refresh(s3, url, validators, due_at):
    key = object_key(url)
    start = time.monotonic()
    stage = 'download'
    try:
        if urlsplit(url).scheme not in ('http', 'https'):
            raise ValueError('unsupported URL scheme')
        # A disk-backed, seekable file makes boto's upload and retries streaming, and
        # lets us validate before publishing. Never expose partial or invalid bytes.
        with tempfile.TemporaryFile() as body:
            size = 0
            deadline = start + 30
            with requests.get(url, timeout=(5, 15), stream=True) as response:
                response.raise_for_status()
                if response.status_code != 200:
                    raise ValueError('expected a complete response')
                for chunk in response.iter_content(65536):
                    size += len(chunk)
                    if size > MAX_BYTES or time.monotonic() > deadline:
                        raise ValueError('feed exceeded download limit')
                    body.write(chunk)
            fetched_at = int(time.time())
            downloaded = time.monotonic()
            stage = 'validation'
            # The protobuf runtime still needs the whole message in memory. Limit
            # simultaneous parses independently of the number of network workers.
            with validators:
                validating = time.monotonic()
                body.seek(0)
                header_timestamp = validate(body)
            validated = time.monotonic()
            stage = 'upload'
            body.seek(0)
            s3.put_object(Bucket=BUCKET, Key=key, Body=body, ContentLength=size,
                          ContentType='application/x-protobuf', CacheControl='no-cache',
                          Metadata={'fetched-at': str(fetched_at)})
            finished = time.monotonic()
        LOG.info('refreshed %s (%d bytes) fetched_at=%d header_timestamp=%s '
                 'queue_s=%.2f download_s=%.2f validation_wait_s=%.2f validation_s=%.2f upload_s=%.2f',
                 key, size, fetched_at, header_timestamp, max(0, start - due_at),
                 downloaded - start, validating - downloaded, validated - validating,
                 finished - validated)
        return RefreshResult(fetched_at=fetched_at)
    except Exception as error:
        # Never log upstream URLs (which may contain credentials) or overwrite good bytes.
        response = getattr(error, 'response', None)
        status = (response.get('ResponseMetadata', {}).get('HTTPStatusCode')
                  if isinstance(response, dict) else getattr(response, 'status_code', None))
        LOG.warning('refresh failed for %s: %s stage=%s http_status=%s queue_s=%.2f elapsed_s=%.2f',
                    key, type(error).__name__, stage, status, max(0, start - due_at),
                    time.monotonic() - start)
        return RefreshResult(failed_stage=stage)


@dataclass
class FeedSchedule:
    due_at: float
    started_at: float = 0
    failures: int = 0
    last_success: int | None = None
    interval: int | None = None


class Scheduler:
    """Oldest deadline first, at most one in-flight refresh per upstream URL."""

    def __init__(self):
        self.active = set()
        self.feeds = {}
        self.running = set()
        self.outcomes = {}

    def reconcile(self, urls, now):
        self.active = set(urls)
        for url in self.active:
            self.feeds.setdefault(url, FeedSchedule(now))
        for url in self.feeds.keys() - self.active - self.running:
            del self.feeds[url]

    def ready(self, now, limit):
        due = sorted((self.feeds[url].due_at, url) for url in self.active - self.running
                     if self.feeds[url].due_at <= now)
        ready = due[:limit]
        self.running.update(url for _, url in ready)
        for _, url in ready:
            self.feeds[url].started_at = now
        return ready

    def complete(self, url, result, now):
        self.running.remove(url)
        outcome = 'success' if result.succeeded else result.failed_stage or 'unknown'
        self.outcomes[outcome] = self.outcomes.get(outcome, 0) + 1
        if url not in self.active:
            del self.feeds[url]
            return
        feed = self.feeds[url]
        if result.succeeded:
            if feed.last_success is not None:
                feed.interval = result.fetched_at - feed.last_success
            feed.last_success = result.fetched_at
            feed.failures = 0
            # Measure from dispatch, not an overdue deadline: avoid catch-up bursts.
            feed.due_at = max(now, feed.started_at + INTERVAL)
        else:
            feed.failures += 1
            delay = min(MAX_RETRY_INTERVAL, RETRY_INTERVAL * 2 ** min(feed.failures - 1, 5))
            feed.due_at = now + delay

    def next_due(self):
        return min((self.feeds[url].due_at for url in self.active - self.running), default=math.inf)

    def report(self, now, wall_now, cpu_seconds):
        feeds = [self.feeds[url] for url in self.active]
        ages = sorted(max(0, wall_now - f.last_success) for f in feeds if f.last_success is not None)
        intervals = sorted(f.interval for f in feeds if f.interval is not None)
        queued = [max(0, now - self.feeds[url].due_at) for url in self.active - self.running
                  if self.feeds[url].due_at <= now]

        def percentile(values, p):
            return values[math.ceil(len(values) * p) - 1] if values else None

        LOG.info('scheduler feeds=%d running=%d overdue=%d oldest_queue_s=%.1f '
                 'never_refreshed=%d age_over_120s=%d age_over_300s=%d '
                 'age_p50_s=%s age_p95_s=%s age_p99_s=%s '
                 'interval_p50_s=%s interval_p95_s=%s interval_p99_s=%s '
                 'outcomes=%s cpu_s=%.2f peak_rss_kib=%d',
                 len(feeds), len(self.running), len(queued), max(queued, default=0),
                 len(feeds) - len(ages), sum(a >= 120 for a in ages), sum(a >= 300 for a in ages),
                 *(percentile(ages, p) for p in (0.5, 0.95, 0.99)),
                 *(percentile(intervals, p) for p in (0.5, 0.95, 0.99)),
                 self.outcomes, cpu_seconds, resource.getrusage(resource.RUSAGE_SELF).ru_maxrss)
        self.outcomes.clear()


def run(s3):
    scheduler = Scheduler()
    validators = BoundedSemaphore(VALIDATORS)
    pending = {}
    manifest = None
    next_manifest = next_report = time.monotonic()
    last_cpu = time.process_time()
    # Manifest IO must not hold up even a single due feed.
    with ThreadPoolExecutor(max_workers=WORKERS) as pool, ThreadPoolExecutor(max_workers=1) as manifests:
        while True:
            now = time.monotonic()
            if manifest is not None and manifest.done():
                try:
                    scheduler.reconcile(manifest.result(), now)
                except Exception as error:
                    LOG.warning('manifest refresh failed: %s; retaining previous sources', type(error).__name__)
                manifest = None
                next_manifest = now + INTERVAL
            for future in [f for f in pending if f.done()]:
                url = pending.pop(future)
                try:
                    result = future.result()
                except Exception as error:
                    LOG.warning('refresh worker failed for %s: %s', object_key(url), type(error).__name__)
                    result = RefreshResult(failed_stage='worker')
                scheduler.complete(url, result, now)
            if manifest is None and now >= next_manifest:
                manifest = manifests.submit(sources)
            for due_at, url in scheduler.ready(now, WORKERS - len(pending)):
                pending[pool.submit(refresh, s3, url, validators, due_at)] = url
            if now >= next_report:
                cpu = time.process_time()
                scheduler.report(now, int(time.time()), cpu - last_cpu)
                last_cpu = cpu
                next_report = now + INTERVAL
            wake = min(next_report, next_manifest if manifest is None else math.inf,
                       scheduler.next_due() if len(pending) < WORKERS else math.inf)
            futures = list(pending) + ([manifest] if manifest is not None else [])
            timeout = max(0, wake - time.monotonic())
            if futures:
                wait(futures, timeout=timeout, return_when=FIRST_COMPLETED)
            else:
                time.sleep(timeout)


def main():
    logging.basicConfig(level=logging.INFO, format='%(asctime)s %(levelname)s %(message)s')
    if WORKERS < 1 or VALIDATORS < 1:
        raise ValueError('worker and validator counts must be positive')
    s3 = boto3.client('s3', endpoint_url=os.environ.get('AWS_ENDPOINT_URL_S3',
                      os.environ.get('AWS_ENDPOINT_URL', 'https://t3.storage.dev')),
                      region_name=os.environ.get('AWS_REGION', 'auto'),
                      config=Config(max_pool_connections=WORKERS,
                                    connect_timeout=5, read_timeout=15,
                                    retries={'total_max_attempts': 2}))
    run(s3)


if __name__ == '__main__':
    main()
