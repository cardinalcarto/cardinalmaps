"""Refresh the public GTFS-RT cache using the published transit release manifest."""

import hashlib
import logging
import os
import time
from concurrent.futures import ThreadPoolExecutor
from urllib.parse import urlsplit

import boto3
import requests
from botocore.config import Config
from google.transit import gtfs_realtime_pb2

BUCKET = os.environ.get('BUCKET_NAME', 'cardinal-carto-public')
RELEASES = os.environ.get('RELEASES_URL', f'https://{BUCKET}.t3.tigrisfiles.io/releases').rstrip('/')
INTERVAL = 60
MAX_BYTES = 32 * 1024 * 1024
WORKERS = int(os.environ.get('GTFS_RT_WORKERS', '16'))
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


def refresh(s3, url):
    key = object_key(url)
    try:
        if urlsplit(url).scheme not in ('http', 'https'):
            raise ValueError('unsupported URL scheme')
        deadline = time.monotonic() + 30
        with requests.get(url, timeout=(5, 15), stream=True) as response:
            response.raise_for_status()
            if response.status_code != 200:
                raise ValueError('expected a complete response')
            body = bytearray()
            for chunk in response.iter_content(65536):
                body.extend(chunk)
                if len(body) > MAX_BYTES or time.monotonic() > deadline:
                    raise ValueError('feed exceeded download limit')
        fetched_at = int(time.time())
        message = gtfs_realtime_pb2.FeedMessage.FromString(bytes(body))
        if not message.header.IsInitialized() or not message.header.gtfs_realtime_version:
            raise ValueError('invalid GTFS-RT message')
        if message.header.incrementality == gtfs_realtime_pb2.FeedHeader.DIFFERENTIAL:
            raise ValueError('differential feeds are unsupported')
        del message
        s3.put_object(Bucket=BUCKET, Key=key, Body=bytes(body),
                      ContentType='application/x-protobuf', CacheControl='no-cache',
                      Metadata={'fetched-at': str(fetched_at)})
        LOG.info('refreshed %s (%d bytes)', key, len(body))
    except Exception as error:
        # Never log upstream URLs (which may contain credentials) or overwrite good bytes.
        LOG.warning('refresh failed for %s: %s', key, type(error).__name__)


def main():
    logging.basicConfig(level=logging.INFO, format='%(asctime)s %(levelname)s %(message)s')
    s3 = boto3.client('s3', endpoint_url=os.environ.get('AWS_ENDPOINT_URL_S3',
                      os.environ.get('AWS_ENDPOINT_URL', 'https://t3.storage.dev')),
                      region_name=os.environ.get('AWS_REGION', 'auto'),
                      config=Config(max_pool_connections=WORKERS,
                                    connect_timeout=5, read_timeout=15,
                                    retries={'total_max_attempts': 2}))
    urls = set()
    with ThreadPoolExecutor(max_workers=WORKERS) as pool:
        while True:
            start = time.monotonic()
            try:
                urls = sources()
            except Exception as error:
                LOG.warning('manifest refresh failed: %s; retaining previous sources', type(error).__name__)
            list(pool.map(lambda url: refresh(s3, url), sorted(urls)))
            LOG.info('finished refreshing %d feeds in %.1fs', len(urls), time.monotonic() - start)
            time.sleep(max(0, INTERVAL - (time.monotonic() - start)))


if __name__ == '__main__':
    main()
