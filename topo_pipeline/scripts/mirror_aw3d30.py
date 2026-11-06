#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["boto3>=1.35"]
# ///
"""Mirrors JAXA's AW3D30 mosaics to an S3-compatible bucket, for future reference.

Each zip is streamed straight from JAXA to the bucket, so this needs no local
disk. It downloads one mosaic at a time, as JAXA asks. It can be rerun at any
point: a mosaic is done once `<zip>.sha256` is in the bucket and the zip there
is the size JAXA reports, and it's skipped from then on.

    export JAXA_USER=... JAXA_PASSWORD=...
    export AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=...
    mirror_aw3d30.py --bucket BUCKET --endpoint HOST [--only NAME ...] [--plan]

The data is free to use, with conditions: https://www.eorc.jaxa.jp/ALOS/en/aw3d30/terms.htm
Anything published from it must credit JAXA.
"""

import argparse
import base64
import hashlib
import io
import os
import re
import sys
import time
import urllib.error
import urllib.request
import zipfile

import boto3
from boto3.s3.transfer import TransferConfig
from botocore.config import Config
from botocore.exceptions import ClientError

RELEASE = "release_v2404"
SITE = "https://www.eorc.jaxa.jp/ALOS"
INDEX = f"{SITE}/en/aw3d30/data/index.htm"
ATTEMPTS = 5
PART_SIZE = 64 * 1024 * 1024


def log(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


class Jaxa:
    def __init__(self, user, password):
        token = base64.b64encode(f"{user}:{password}".encode()).decode()
        self.headers = {"Authorization": f"Basic {token}"}

    def open(self, url, method="GET"):
        request = urllib.request.Request(url, headers=self.headers, method=method)
        return urllib.request.urlopen(request, timeout=120)

    def text(self, url):
        with self.open(url) as response:
            return response.read().decode("utf-8", "replace")

    def mosaics(self):
        """Every mosaic's name, like N045W125_N050W120, from the site's map pages."""
        index = self.text(INDEX)
        regions = sorted(set(re.findall(r"html_v2404/([ns]\d+[ew]\d+_[ns]\d+[ew]\d+)\.htm", index)))
        if not regions:
            raise RuntimeError("the index has no region pages; has the site changed?")
        names = set()
        for region in regions:
            page = self.text(f"{SITE}/en/aw3d30/data/html_v2404/{region}.htm")
            names |= {f"{a}_{b}" for a, b in re.findall(r"comp\('([NS]\d+[EW]\d+)','([NS]\d+[EW]\d+)'", page)}
        return sorted(names)

    def size(self, url):
        with self.open(url, method="HEAD") as response:
            return int(response.headers["Content-Length"])


class HashingReader:
    """A file-like view of a stream, hashing and counting what passes through."""

    def __init__(self, stream):
        self.stream = stream
        self.sha256 = hashlib.sha256()
        self.count = 0

    def read(self, size=-1):
        data = self.stream.read(size)
        self.sha256.update(data)
        self.count += len(data)
        return data


class ObjectFile(io.RawIOBase):
    """A seekable view of an object, reading only the byte ranges asked for."""

    def __init__(self, s3, bucket, key, size):
        self.s3, self.bucket, self.key, self.size, self.position = s3, bucket, key, size, 0

    def readable(self):
        return True

    def seekable(self):
        return True

    def tell(self):
        return self.position

    def seek(self, offset, whence=io.SEEK_SET):
        base = {io.SEEK_SET: 0, io.SEEK_CUR: self.position, io.SEEK_END: self.size}[whence]
        self.position = base + offset
        return self.position

    def readinto(self, buffer):
        if self.position >= self.size or not len(buffer):
            return 0
        end = min(self.position + len(buffer), self.size) - 1
        data = self.s3.get_object(Bucket=self.bucket, Key=self.key, Range=f"bytes={self.position}-{end}")["Body"].read()
        buffer[: len(data)] = data
        self.position += len(data)
        return len(data)


def object_size(s3, bucket, key):
    try:
        return s3.head_object(Bucket=bucket, Key=key)["ContentLength"]
    except ClientError as error:
        if error.response["ResponseMetadata"]["HTTPStatusCode"] == 404:
            return None
        raise


def mirror(jaxa, s3, bucket, name):
    """Returns the bytes transferred, or None if the mosaic was already there."""
    url = f"{SITE}/aw3d30/data/{RELEASE}/{name}.zip"
    key = f"aw3d30/{RELEASE}/{name}.zip"
    expected = jaxa.size(url)
    if object_size(s3, bucket, key) == expected and object_size(s3, bucket, key + ".sha256") is not None:
        return None

    with jaxa.open(url) as response:
        reader = HashingReader(response)
        # One thread, so parts leave in order and only one read of JAXA is ever open.
        config = TransferConfig(multipart_threshold=PART_SIZE, multipart_chunksize=PART_SIZE, use_threads=False)
        s3.upload_fileobj(reader, bucket, key, Config=config, ExtraArgs={"ContentType": "application/zip"})
    if reader.count != expected:
        raise RuntimeError(f"read {reader.count} bytes of {expected}")
    stored = object_size(s3, bucket, key)
    if stored != expected:
        raise RuntimeError(f"the bucket has {stored} bytes of {expected}")

    # A truncated or garbled zip would fail here, on its central directory.
    file = io.BufferedReader(ObjectFile(s3, bucket, key, stored), buffer_size=1024 * 1024)
    with zipfile.ZipFile(file) as archive:
        if not any(n.endswith("_DSM.tif") for n in archive.namelist()):
            raise RuntimeError("the zip has no DSM rasters")

    # Written last, since it's what marks the mosaic done.
    s3.put_object(Bucket=bucket, Key=key + ".sha256", Body=f"{reader.sha256.hexdigest()}  {name}.zip\n".encode())
    return reader.count


def is_missing(error):
    return isinstance(error, urllib.error.HTTPError) and error.code == 404


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--bucket", required=True)
    parser.add_argument("--endpoint", required=True, help="the S3 endpoint's host")
    parser.add_argument("--region", help="the signing region; by default, taken from the endpoint")
    parser.add_argument("--only", nargs="*", help="mirror just these mosaics")
    parser.add_argument("--plan", action="store_true", help="report how much is left, transferring nothing")
    args = parser.parse_args()

    region = args.region or args.endpoint.split(".")[1]
    s3 = boto3.client(
        "s3",
        endpoint_url=f"https://{args.endpoint}",
        region_name=region,
        config=Config(
            signature_version="s3v4",
            retries={"max_attempts": 8, "mode": "standard"},
            # Backblaze doesn't take the checksums that boto3 adds by default.
            request_checksum_calculation="when_required",
            response_checksum_validation="when_required",
        ),
    )
    jaxa = Jaxa(os.environ["JAXA_USER"], os.environ["JAXA_PASSWORD"])
    names = args.only or jaxa.mosaics()
    log(f"{len(names)} mosaics")

    if args.plan:
        total, left, missing = 0, 0, []
        for name in names:
            try:
                size = jaxa.size(f"{SITE}/aw3d30/data/{RELEASE}/{name}.zip")
            except urllib.error.HTTPError as error:
                if not is_missing(error):
                    raise
                missing.append(name)
                continue
            total += size
            key = f"aw3d30/{RELEASE}/{name}.zip"
            if object_size(s3, args.bucket, key) != size or object_size(s3, args.bucket, key + ".sha256") is None:
                left += size
        log(f"{total / 1e9:.1f} GB in total, {left / 1e9:.1f} GB left; not at JAXA: {missing}")
        return 0

    done, skipped, moved, failed, missing = 0, 0, 0, [], []
    started = time.time()
    for position, name in enumerate(names, 1):
        for attempt in range(1, ATTEMPTS + 1):
            try:
                count = mirror(jaxa, s3, args.bucket, name)
            except Exception as error:  # noqa: BLE001 - the network can fail in many ways, and the next mosaic may be fine
                if is_missing(error):
                    log(f"{name}: not at JAXA")
                    missing.append(name)
                    break
                log(f"{name}: attempt {attempt}: {error!r}")
                if attempt == ATTEMPTS:
                    failed.append(name)
                else:
                    time.sleep(30 * 2**attempt)
                continue
            if count is None:
                skipped += 1
            else:
                done += 1
                moved += count
                rate = moved / max(time.time() - started, 1) / 1e6
                log(f"{position}/{len(names)} {name}: {count / 1e6:.0f} MB; {moved / 1e9:.1f} GB so far at {rate:.1f} MB/s")
            break

    log(f"{done} mirrored, {skipped} already there; not at JAXA: {missing}; failed: {failed}")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
