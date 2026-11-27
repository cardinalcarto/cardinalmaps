"""Refresh the public GTFS-RT cache using independently scheduled feed downloads."""

import hashlib
import heapq
import logging
import math
import os
import resource
import ssl
import tempfile
import time
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from contextlib import contextmanager
from dataclasses import dataclass
from threading import BoundedSemaphore
from urllib.parse import unquote, urlsplit

import boto3
import certifi
import urllib3
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
# The current release has about 200 upstream origins, plus redirect targets.
HTTP_ORIGINS = 512
HTTP_CONNECTIONS_PER_ORIGIN = 16
LOG = logging.getLogger(__name__)


def object_key(url):
    return 'gtfs-rt/' + hashlib.sha256(url.encode('utf-8')).hexdigest() + '.pb'


class HTTPStatusError(Exception):
    def __init__(self, status):
        self.status = status
        super().__init__(f'HTTP {status}')


def upstream_pool():
    # Load trust roots once, not on every new TLS connection. PoolManager is
    # thread-safe and shares connections by origin across all feed workers.
    context = ssl.create_default_context(cafile=certifi.where())
    return urllib3.PoolManager(
        ssl_context=context, num_pools=HTTP_ORIGINS,
        maxsize=HTTP_CONNECTIONS_PER_ORIGIN, block=False,
        headers={'Accept-Encoding': 'gzip, deflate', 'User-Agent': 'cardinal-gtfs-rt-cache/1.0'},
        # Keep redirects, but let the per-feed scheduler own network retries.
        retries=urllib3.Retry(total=5, connect=0, read=0, status=0, other=0,
                              redirect=5, raise_on_redirect=False,
                              respect_retry_after_header=False),
    )


@contextmanager
def get(http, url, timeout):
    parsed = urlsplit(url)
    if parsed.scheme not in ('http', 'https'):
        raise ValueError('unsupported URL scheme')
    headers = None
    if parsed.username is not None:
        # Requests formerly supported credentials embedded in the URL. Preserve
        # that behavior without ever storing credentials in the shared pool.
        headers = dict(http.headers)
        headers.update(urllib3.make_headers(basic_auth=(
            f'{unquote(parsed.username)}:{unquote(parsed.password or "")}'
        )))
        url = parsed._replace(netloc=parsed.netloc.rsplit('@', 1)[-1]).geturl()
    response = http.request('GET', url, timeout=timeout, headers=headers,
                            preload_content=False, decode_content=True)
    try:
        if response.status != 200:
            raise HTTPStatusError(response.status)
        yield response
    finally:
        # Fully consumed responses have already returned their connection to the
        # pool. Close incomplete responses before returning their slot.
        response.close()
        response.release_conn()


def sources(http):
    with get(http, f'{RELEASES}/latest.json', urllib3.Timeout(connect=5, read=10)) as latest:
        date = latest.json()['transit']
    with get(http, f'{RELEASES}/transit/{date}/realtime.json',
             urllib3.Timeout(connect=5, read=15)) as manifest:
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


def refresh(s3, http, url, validators, due_at):
    key = object_key(url)
    start = time.monotonic()
    cpu_start = time.thread_time()
    stage = 'download'
    try:
        # A disk-backed, seekable file makes boto's upload and retries streaming, and
        # lets us validate before publishing. Never expose partial or invalid bytes.
        with tempfile.TemporaryFile() as body:
            size = 0
            deadline = start + 30
            with get(http, url, urllib3.Timeout(connect=5, read=15)) as response:
                for chunk in response.stream(65536, decode_content=True):
                    size += len(chunk)
                    if size > MAX_BYTES or time.monotonic() > deadline:
                        raise ValueError('feed exceeded download limit')
                    body.write(chunk)
            fetched_at = int(time.time())
            downloaded = time.monotonic()
            downloaded_cpu = time.thread_time()
            stage = 'validation'
            # The protobuf runtime still needs the whole message in memory. Limit
            # simultaneous parses independently of the number of network workers.
            with validators:
                validating = time.monotonic()
                validating_cpu = time.thread_time()
                body.seek(0)
                header_timestamp = validate(body)
            validated = time.monotonic()
            validated_cpu = time.thread_time()
            stage = 'upload'
            body.seek(0)
            s3.put_object(Bucket=BUCKET, Key=key, Body=body, ContentLength=size,
                          ContentType='application/x-protobuf', CacheControl='no-cache',
                          Metadata={'fetched-at': str(fetched_at)})
            finished = time.monotonic()
            finished_cpu = time.thread_time()
        LOG.info('refreshed %s (%d bytes) fetched_at=%d header_timestamp=%s '
                 'queue_s=%.2f download_s=%.2f validation_wait_s=%.2f validation_s=%.2f upload_s=%.2f '
                 'download_cpu_ms=%.3f validation_cpu_ms=%.3f upload_cpu_ms=%.3f',
                 key, size, fetched_at, header_timestamp, max(0, start - due_at),
                 downloaded - start, validating - downloaded, validated - validating,
                 finished - validated, 1000 * (downloaded_cpu - cpu_start),
                 1000 * (validated_cpu - validating_cpu), 1000 * (finished_cpu - validated_cpu))
        return RefreshResult(fetched_at=fetched_at)
    except Exception as error:
        # Never log upstream URLs (which may contain credentials) or overwrite good bytes.
        response = getattr(error, 'response', None)
        status = (error.status if isinstance(error, HTTPStatusError) else
                  response.get('ResponseMetadata', {}).get('HTTPStatusCode')
                  if isinstance(response, dict) else None)
        reason = error.reason if isinstance(error, urllib3.exceptions.MaxRetryError) else error
        LOG.warning('refresh failed for %s: %s stage=%s http_status=%s queue_s=%.2f elapsed_s=%.2f cpu_ms=%.3f',
                    key, type(reason).__name__, stage, status, max(0, start - due_at),
                    time.monotonic() - start, 1000 * (time.thread_time() - cpu_start))
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
        self.deadlines = []

    def reconcile(self, urls, now):
        self.active = set(urls)
        for url in self.active:
            self.feeds.setdefault(url, FeedSchedule(now))
        for url in self.feeds.keys() - self.active - self.running:
            del self.feeds[url]
        # Manifest refreshes are infrequent. Rebuild once here instead of scanning
        # and sorting all 1,000+ sources on every worker completion.
        self.deadlines = [(self.feeds[url].due_at, url) for url in self.active - self.running]
        heapq.heapify(self.deadlines)

    def ready(self, now, limit):
        ready = []
        while len(ready) < limit and self.deadlines and self.deadlines[0][0] <= now:
            due_at, url = heapq.heappop(self.deadlines)
            self.running.add(url)
            self.feeds[url].started_at = now
            ready.append((due_at, url))
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
        heapq.heappush(self.deadlines, (feed.due_at, url))

    def next_due(self):
        return self.deadlines[0][0] if self.deadlines else math.inf

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
    with upstream_pool() as http, ThreadPoolExecutor(max_workers=WORKERS) as pool, \
            ThreadPoolExecutor(max_workers=1) as manifests:
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
                manifest = manifests.submit(sources, http)
            for due_at, url in scheduler.ready(now, WORKERS - len(pending)):
                pending[pool.submit(refresh, s3, http, url, validators, due_at)] = url
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
    # urllib3 may log redirect targets and retry URLs (including credentials).
    # Our per-feed diagnostics report the hashed key and error type instead.
    logging.getLogger('urllib3').setLevel(logging.CRITICAL + 1)
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
