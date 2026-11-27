"""Compare the upstream and public cached copies of one transit feed."""

import argparse
import hashlib
import json
import time

import requests
from google.transit import gtfs_realtime_pb2

from updater import BUCKET, MAX_BYTES, RELEASES, object_key


def manifest_sources(feed, release):
    if release is None:
        latest = requests.get(f'{RELEASES}/latest.json', timeout=10)
        latest.raise_for_status()
        release = latest.json()['transit']
    response = requests.get(f'{RELEASES}/transit/{release}/realtime.json', timeout=15)
    response.raise_for_status()
    urls = sorted({entry['url'] for entry in response.json()['feeds'] if entry['id'] == feed})
    return release, urls


def probe(upstream, url, location):
    result = {'key': object_key(upstream), 'location': location}
    start = time.monotonic()
    try:
        with requests.get(url, timeout=(5, 15), stream=True) as response:
            result['http_status'] = response.status_code
            response.raise_for_status()
            if response.status_code != 200:
                raise ValueError('expected a complete response')
            body = bytearray()
            for chunk in response.iter_content(65536):
                if len(body) + len(chunk) > MAX_BYTES or time.monotonic() - start > 30:
                    raise ValueError('feed exceeded download limit')
                body.extend(chunk)
            fetched = response.headers.get('x-amz-meta-fetched-at')
            now = int(time.time())
            result.update(observed_at=now, fetched_at=int(fetched) if fetched is not None else None,
                          http_age=response.headers.get('age'), cache_control=response.headers.get('cache-control'),
                          bytes=len(body), sha256=hashlib.sha256(body).hexdigest())
        message = gtfs_realtime_pb2.FeedMessage.FromString(bytes(body))
        header = message.header.timestamp if message.header.HasField('timestamp') else None
        result.update(header_timestamp=header, header_age_s=now - header if header is not None else None,
                      cache_age_s=now - result['fetched_at'] if result['fetched_at'] is not None else None,
                      trip_updates=sum(e.HasField('trip_update') for e in message.entity),
                      vehicles=sum(e.HasField('vehicle') for e in message.entity),
                      alerts=sum(e.HasField('alert') for e in message.entity))
    except Exception as error:
        # Errors and upstream URLs can contain credentials. Print only the type.
        result['error'] = type(error).__name__
    result['elapsed_s'] = round(time.monotonic() - start, 3)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('feed', help='transit feed ID, e.g. us-wa-Metro-Transit')
    parser.add_argument('--release', help='pinned transit release date (default: latest)')
    parser.add_argument('--samples', type=int, default=1)
    parser.add_argument('--interval', type=float, default=30, help='seconds between samples')
    parser.add_argument('--cache-url', default=f'https://{BUCKET}.t3.tigrisfiles.io/gtfs-rt')
    args = parser.parse_args()
    if args.samples < 1 or args.interval < 0:
        parser.error('samples must be positive and interval must be nonnegative')
    release, urls = manifest_sources(args.feed, args.release)
    if not urls:
        parser.exit(1, f'No sources for {args.feed} in release {release}\n')
    print(json.dumps({'feed': args.feed, 'release': release, 'sources': len(urls)}), flush=True)
    for sample in range(args.samples):
        start = time.monotonic()
        for upstream in urls:
            cached = f'{args.cache_url.rstrip("/")}/{object_key(upstream).split("/", 1)[1]}'
            for url, location in ((cached, 'cache'), (upstream, 'agency')):
                print(json.dumps({'sample': sample + 1, **probe(upstream, url, location)}), flush=True)
        if sample + 1 < args.samples:
            time.sleep(max(0, args.interval - (time.monotonic() - start)))


if __name__ == '__main__':
    main()
