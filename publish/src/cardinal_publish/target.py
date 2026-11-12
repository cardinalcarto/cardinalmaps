"""Where releases go: an S3-compatible bucket, or a local directory for testing.

Both keep, for every object, its size and the SHA-256 of its bytes as stored (in S3, the
`x-amz-meta-sha256` header), so a rerun can tell what's already published.
"""

import base64
import dataclasses
import hashlib
import json
import os
import shutil
import sqlite3
import sys
import threading
from concurrent.futures import FIRST_COMPLETED, ThreadPoolExecutor, wait
from dataclasses import dataclass
from pathlib import Path

from .common import Progress, PublishError, Upload, sha256_hex

META_DIR = ".meta"


@dataclass(frozen=True)
class Remote:
    size: int
    sha256: str | None


class DirTarget:
    """A local directory laid out as the bucket would be, with headers kept in `.meta/`."""

    def __init__(self, root: Path):
        self.root = root

    def describe(self):
        return f"dir:{self.root}"

    def _meta(self, key):
        return self.root / META_DIR / (key + ".json")

    def list(self, prefix):
        found = {}
        base = self.root / prefix
        if not base.exists():
            return found
        for directory, _, files in os.walk(base):
            for name in files:
                if name.endswith(".part"):
                    continue
                path = Path(directory) / name
                found[path.relative_to(self.root).as_posix()] = path.stat().st_size
        return found

    def head(self, key):
        path = self.root / key
        if not path.is_file():
            return None
        sha = None
        meta = self._meta(key)
        if meta.is_file():
            sha = json.loads(meta.read_text()).get("sha256")
        return Remote(path.stat().st_size, sha)

    def get(self, key):
        path = self.root / key
        return path.read_bytes() if path.is_file() else None

    def put(self, upload: Upload, key: str):
        path = self.root / key
        path.parent.mkdir(parents=True, exist_ok=True)
        partial = path.with_name(path.name + ".part")
        if upload.body is not None:
            partial.write_bytes(upload.body)
        else:
            shutil.copyfile(upload.path, partial)
        self._write_meta(key, upload.size, upload.sha256, upload)
        os.replace(partial, path)

    def _write_meta(self, key, size, sha256, upload):
        meta = self._meta(key)
        meta.parent.mkdir(parents=True, exist_ok=True)
        meta.write_text(json.dumps({
            "size": size,
            "sha256": sha256,
            "content_type": upload.content_type,
            "content_encoding": upload.content_encoding,
            "cache_control": upload.cache_control,
        }))

    def put_large(self, path: Path, key: str, upload: Upload, part_size: int, jobs: int, state, dry_run):
        """Copies the file, hashing it as it goes; returns its SHA-256."""
        digest = hashlib.sha256()
        target = self.root / key
        partial = target.with_name(target.name + ".part")
        if not dry_run:
            target.parent.mkdir(parents=True, exist_ok=True)
        with open(path, "rb") as source, (open(partial, "wb") if not dry_run else open(os.devnull, "wb")) as out:
            while block := source.read(part_size):
                digest.update(block)
                out.write(block)
        sha = digest.hexdigest()
        if not dry_run:
            self._write_meta(key, upload.size, sha, upload)
            os.replace(partial, target)
        return sha


class S3Target:
    """A bucket on an S3-compatible service (Tigris by default)."""

    def __init__(self, bucket, endpoint, region, access_key_id, secret_access_key, jobs):
        import boto3
        from botocore.config import Config

        self.bucket = bucket
        self.endpoint = endpoint
        self.client = boto3.client(
            "s3",
            endpoint_url=endpoint,
            region_name=region,
            aws_access_key_id=access_key_id,
            aws_secret_access_key=secret_access_key,
            config=Config(
                max_pool_connections=max(jobs * 2, 10),
                retries={"mode": "standard", "max_attempts": 10},
                # Integrity comes from Content-MD5 on each request; the newer default CRC
                # checksums aren't supported by every S3-compatible service.
                request_checksum_calculation="when_required",
                response_checksum_validation="when_required",
                s3={"addressing_style": "virtual"},
            ),
        )

    def describe(self):
        return f"s3://{self.bucket} at {self.endpoint}"

    def list(self, prefix):
        found = {}
        paginator = self.client.get_paginator("list_objects_v2")
        for page in paginator.paginate(Bucket=self.bucket, Prefix=prefix):
            for item in page.get("Contents", []):
                found[item["Key"]] = item["Size"]
        return found

    def head(self, key):
        from botocore.exceptions import ClientError

        try:
            response = self.client.head_object(Bucket=self.bucket, Key=key)
        except ClientError as error:
            if error.response.get("Error", {}).get("Code") in ("404", "NoSuchKey", "NotFound"):
                return None
            raise
        return Remote(response["ContentLength"], response.get("Metadata", {}).get("sha256"))

    def get(self, key):
        from botocore.exceptions import ClientError

        try:
            response = self.client.get_object(Bucket=self.bucket, Key=key)
        except ClientError as error:
            if error.response.get("Error", {}).get("Code") in ("404", "NoSuchKey", "NotFound"):
                return None
            raise
        body = response["Body"].read()
        # boto3 doesn't inflate Content-Encoding: gzip, so the stored bytes come back.
        return body

    def _headers(self, upload: Upload):
        headers = {
            "ContentType": upload.content_type,
            "CacheControl": upload.cache_control,
        }
        if upload.content_encoding:
            headers["ContentEncoding"] = upload.content_encoding
        return headers

    def put(self, upload: Upload, key: str):
        body = upload.read()
        md5 = base64.b64encode(hashlib.md5(body).digest()).decode()
        self.client.put_object(
            Bucket=self.bucket,
            Key=key,
            Body=body,
            ContentMD5=md5,
            Metadata={"sha256": upload.sha256},
            **self._headers(upload),
        )

    def put_large(self, path: Path, key: str, upload: Upload, part_size: int, jobs: int, state, dry_run):
        """A multipart upload that resumes the one a previous run left unfinished.

        The file is read in order, once, to hash it; parts the bucket already holds with the
        same MD5 are not sent again. Returns the file's SHA-256.
        """
        size = upload.size
        identity = f"{size}:{int(path.stat().st_mtime)}:{part_size}"
        upload_id = state.multipart(self.describe(), key, identity) if state else None
        done = {}
        if upload_id:
            try:
                for page in self.client.get_paginator("list_parts").paginate(
                    Bucket=self.bucket, Key=key, UploadId=upload_id
                ):
                    for part in page.get("Parts", []):
                        done[part["PartNumber"]] = (part["ETag"].strip('"'), part["Size"])
                print(f"resuming upload {upload_id}: {len(done)} parts already sent", file=sys.stderr)
            except self.client.exceptions.NoSuchUpload:
                upload_id = None
        if not upload_id and not dry_run:
            upload_id = self.client.create_multipart_upload(
                Bucket=self.bucket, Key=key, **self._headers(upload)
            )["UploadId"]
            state.set_multipart(self.describe(), key, identity, upload_id)

        digest = hashlib.sha256()
        parts = {}
        progress = Progress(f"upload {key}", (size + part_size - 1) // part_size, size)

        def send(number, block, md5):
            self.client.upload_part(
                Bucket=self.bucket, Key=key, UploadId=upload_id, PartNumber=number,
                Body=block, ContentMD5=base64.b64encode(md5).decode(),
            )
            return number

        with open(path, "rb") as source, ThreadPoolExecutor(jobs) as pool:
            pending = set()
            number = 0
            while block := source.read(part_size):
                number += 1
                digest.update(block)
                md5 = hashlib.md5(block).digest()
                parts[number] = md5.hex()
                if done.get(number) == (md5.hex(), len(block)) or dry_run:
                    progress.add(len(block), uploaded=False)
                    continue
                # At most `jobs` parts wait in memory besides those being sent.
                while len(pending) >= jobs:
                    finished, pending = wait(pending, return_when=FIRST_COMPLETED)
                    for future in finished:
                        future.result()
                pending.add(pool.submit(send, number, block, md5))
                progress.add(len(block), uploaded=True)
            for future in pending:
                future.result()
        progress.print(final=True)
        if dry_run:
            return digest.hexdigest()
        self.client.complete_multipart_upload(
            Bucket=self.bucket, Key=key, UploadId=upload_id,
            MultipartUpload={"Parts": [{"PartNumber": n, "ETag": f'"{etag}"'} for n, etag in parts.items()]},
        )
        state.clear_multipart(self.describe(), key)
        return digest.hexdigest()


def parse_target(spec, args, jobs):
    if spec.startswith("dir:"):
        return DirTarget(Path(spec[len("dir:"):]).resolve())
    if spec.startswith("s3://"):
        bucket = spec[len("s3://"):].strip("/")
        if not bucket or "/" in bucket:
            raise PublishError(f"use --target s3://BUCKET (no path), not {spec!r}; see --prefix")
        if not args.access_key_id or not args.secret_access_key:
            raise PublishError("an s3:// target needs --access-key-id and --secret-access-key")
        return S3Target(bucket, args.endpoint, args.region, args.access_key_id, args.secret_access_key, jobs)
    raise PublishError(f"--target is s3://BUCKET or dir:/PATH, not {spec!r}")


class State:
    """What this machine has published, so a rerun needn't ask the bucket for each SHA-256.

    It is only a cache: an object counts as published when the bucket has it at the right
    size and either this state or the object's own `sha256` header says it has the same bytes.
    """

    def __init__(self, path: Path):
        path.parent.mkdir(parents=True, exist_ok=True)
        self.lock = threading.Lock()
        self.db = sqlite3.connect(path, check_same_thread=False, isolation_level=None)
        self.db.execute("PRAGMA journal_mode=WAL")
        self.db.execute(
            "CREATE TABLE IF NOT EXISTS objects (target TEXT, key TEXT, size INTEGER, sha256 TEXT,"
            " PRIMARY KEY (target, key))"
        )
        self.db.execute(
            "CREATE TABLE IF NOT EXISTS multipart (target TEXT, key TEXT, identity TEXT,"
            " upload_id TEXT, PRIMARY KEY (target, key))"
        )
        self.db.execute(
            "CREATE TABLE IF NOT EXISTS local_files (path TEXT PRIMARY KEY, identity TEXT, sha256 TEXT)"
        )
        self.pending = 0
        self.db.execute("BEGIN")

    def known(self, target, key):
        with self.lock:
            row = self.db.execute(
                "SELECT size, sha256 FROM objects WHERE target = ? AND key = ?", (target, key)
            ).fetchone()
        return row

    def record(self, target, key, size, sha256):
        with self.lock:
            self.db.execute(
                "INSERT OR REPLACE INTO objects VALUES (?, ?, ?, ?)", (target, key, size, sha256)
            )
            self.pending += 1
            if self.pending >= 500:
                self.db.execute("COMMIT")
                self.db.execute("BEGIN")
                self.pending = 0

    def multipart(self, target, key, identity):
        with self.lock:
            row = self.db.execute(
                "SELECT identity, upload_id FROM multipart WHERE target = ? AND key = ?", (target, key)
            ).fetchone()
        return row[1] if row and row[0] == identity else None

    def set_multipart(self, target, key, identity, upload_id):
        with self.lock:
            self.db.execute(
                "INSERT OR REPLACE INTO multipart VALUES (?, ?, ?, ?)", (target, key, identity, upload_id)
            )
            self.db.execute("COMMIT")
            self.db.execute("BEGIN")

    def clear_multipart(self, target, key):
        with self.lock:
            self.db.execute("DELETE FROM multipart WHERE target = ? AND key = ?", (target, key))
            self.db.execute("COMMIT")
            self.db.execute("BEGIN")

    def file_sha(self, path, identity):
        """A local file's SHA-256 from an earlier run, if it hasn't changed since (size, mtime)."""
        with self.lock:
            row = self.db.execute("SELECT identity, sha256 FROM local_files WHERE path = ?", (path,)).fetchone()
        return row[1] if row and row[0] == identity else None

    def set_file_sha(self, path, identity, sha256):
        with self.lock:
            self.db.execute("INSERT OR REPLACE INTO local_files VALUES (?, ?, ?)", (path, identity, sha256))
            self.db.execute("COMMIT")
            self.db.execute("BEGIN")

    def close(self):
        with self.lock:
            self.db.execute("COMMIT")
            self.db.close()


class Publisher:
    """Uploads objects under one root prefix, skipping those already there with the same bytes."""

    def __init__(self, target, root, state: State | None, jobs, dry_run):
        self.target = target
        self.root = root.strip("/")
        self.state = state
        self.jobs = jobs
        self.dry_run = dry_run

    def checking(self):
        """A publisher that only reads, for checking a release that's complete already."""
        return Publisher(self.target, self.root, self.state, self.jobs, dry_run=True)

    def key(self, relative):
        return f"{self.root}/{relative}" if self.root else relative

    def get(self, relative):
        return self.target.get(self.key(relative))

    def get_json(self, relative):
        import gzip

        body = self.get(relative)
        if body is None:
            return None
        if body[:2] == b"\x1f\x8b":
            body = gzip.decompress(body)
        return json.loads(body)

    def remote(self, prefix):
        """Sizes of what's under `prefix`, keyed relative to the root."""
        cut = len(self.key(""))
        return {key[cut:]: size for key, size in self.target.list(self.key(prefix)).items()}

    def published(self, upload: Upload, remote_size):
        if remote_size != upload.size:
            return False
        target = self.target.describe()
        if self.state:
            known = self.state.known(target, upload.key)
            if known and tuple(known) == (upload.size, upload.sha256):
                return True
        head = self.target.head(self.key(upload.key))
        if head and head.size == upload.size and head.sha256 == upload.sha256:
            if self.state and not self.dry_run:
                self.state.record(target, upload.key, upload.size, upload.sha256)
            return True
        return False

    def put(self, upload: Upload):
        if self.dry_run:
            return
        self.target.put(upload, self.key(upload.key))
        if self.state:
            self.state.record(self.target.describe(), upload.key, upload.size, upload.sha256)

    def sync(self, uploads, remote, label, total_items=None, total_bytes=None):
        """Publishes each upload not already in `remote` with the same bytes, `jobs` at a time.

        `uploads` may be a generator; at most a few times `jobs` objects are held in memory.
        Returns the progress counts.
        """
        progress = Progress(label, total_items, total_bytes, "would upload" if self.dry_run else "uploaded")
        with ThreadPoolExecutor(self.jobs) as pool:
            pending = set()

            def one(upload):
                if upload.body is None:
                    # Read here, on a worker, so only `jobs` files are in memory at once.
                    body = upload.path.read_bytes()
                    sha = sha256_hex(body)
                    if len(body) != upload.size or (upload.sha256 and sha != upload.sha256):
                        raise PublishError(
                            f"{upload.path} is {len(body)} bytes with SHA-256 {sha}, but its build "
                            f"recorded {upload.size} bytes{' and ' + upload.sha256 if upload.sha256 else ''}"
                        )
                    upload = dataclasses.replace(upload, body=body, sha256=sha)
                if self.published(upload, remote.get(upload.key)):
                    return upload.size, False
                self.put(upload)
                return upload.size, True

            def drain(limit):
                nonlocal pending
                while len(pending) > limit:
                    finished, pending = wait(pending, return_when=FIRST_COMPLETED)
                    for future in finished:
                        size, uploaded = future.result()
                        progress.add(size, uploaded)

            for upload in uploads:
                drain(self.jobs * 4)
                pending.add(pool.submit(one, upload))
            drain(0)
        progress.print(final=True)
        return progress
