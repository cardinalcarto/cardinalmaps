"""What every step shares: release dates, object headers, JSON encoding, and progress."""

import gzip
import hashlib
import json
import re
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path

KINDS = ("map", "routing", "transit")

# A release is named by its date; a second release on one day is `.1`, then `.2`.
DATE_PATTERN = re.compile(r"^\d{4}-\d{2}-\d{2}(\.[1-9]\d*)?$")

# Dated objects never change once their release is complete, but one may be replaced while
# its release is still being uploaded, so caches keep them for a week rather than forever.
CACHE_RELEASE_FILE = "public, max-age=604800"
# release.json marks a release complete, so it must show up soon after it's written.
CACHE_RELEASE_MARKER = "public, max-age=300"
CACHE_LATEST = "public, max-age=60"

OCTET_STREAM = "application/octet-stream"
JSON = "application/json"


class PublishError(Exception):
    """A step can't go on; the message says why and what to do."""


def check_date(date: str) -> str:
    if not DATE_PATTERN.match(date):
        raise PublishError(f"release date {date!r} isn't YYYY-MM-DD or YYYY-MM-DD.N")
    return date


def release_prefix(kind: str, date: str) -> str:
    return f"{kind}/{check_date(date)}/"


def sha256_hex(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def sha256_file(path: Path, chunk: int = 8 << 20) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as file:
        while block := file.read(chunk):
            digest.update(block)
    return digest.hexdigest()


def json_bytes(value) -> bytes:
    """Compact, key-sorted JSON, so the same value always gives the same bytes."""
    return json.dumps(value, separators=(",", ":"), sort_keys=True).encode()


def gzip_bytes(data: bytes) -> bytes:
    # mtime=0 keeps the output byte-identical across runs, so a rerun sees the same SHA-256.
    return gzip.compress(data, compresslevel=9, mtime=0)


@dataclass(frozen=True)
class Upload:
    """One object to publish: the bytes as stored, and their headers."""

    key: str
    body: bytes | None
    path: Path | None
    size: int
    sha256: str
    content_type: str
    content_encoding: str | None = None
    cache_control: str = CACHE_RELEASE_FILE

    @staticmethod
    def of_bytes(key, body, content_type, content_encoding=None, cache_control=CACHE_RELEASE_FILE):
        return Upload(
            key=key,
            body=body,
            path=None,
            size=len(body),
            sha256=sha256_hex(body),
            content_type=content_type,
            content_encoding=content_encoding,
            cache_control=cache_control,
        )

    @staticmethod
    def of_json(key, value, cache_control=CACHE_RELEASE_FILE):
        """Gzipped JSON, served with `Content-Encoding: gzip` so HTTP clients inflate it."""
        return Upload.of_bytes(key, gzip_bytes(json_bytes(value)), JSON, "gzip", cache_control)

    @staticmethod
    def of_plain_json(key, value, cache_control):
        """Uncompressed JSON, for the small files people also read by hand."""
        return Upload.of_bytes(
            key, json.dumps(value, indent=1, sort_keys=True).encode() + b"\n", JSON, None, cache_control
        )

    def read(self) -> bytes:
        if self.body is not None:
            return self.body
        return self.path.read_bytes()


def publisher_commit() -> str | None:
    """The commit of the checkout this tool runs from, if it's a clean git checkout."""
    here = Path(__file__).resolve().parent
    try:
        commit = subprocess.run(
            ["git", "-C", str(here), "rev-parse", "HEAD"],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
        dirty = subprocess.run(
            ["git", "-C", str(here), "status", "--porcelain", "--", "."],
            check=True, capture_output=True, text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None
    return commit + ("-dirty" if dirty else "")


class Progress:
    """Prints counts and rates to stderr every few seconds."""

    def __init__(self, label: str, total_items: int | None = None, total_bytes: int | None = None,
                 verb: str = "uploaded"):
        self.label = label
        self.verb = verb
        self.total_items = total_items
        self.total_bytes = total_bytes
        self.items = 0
        self.bytes = 0
        self.uploaded = 0
        self.uploaded_bytes = 0
        self.started = time.monotonic()
        self.last = 0.0

    def add(self, size: int, uploaded: bool):
        self.items += 1
        self.bytes += size
        if uploaded:
            self.uploaded += 1
            self.uploaded_bytes += size
        now = time.monotonic()
        if now - self.last >= 10:
            self.last = now
            self.print()

    def print(self, final: bool = False):
        elapsed = max(time.monotonic() - self.started, 1e-9)
        of_items = f"/{self.total_items}" if self.total_items is not None else ""
        of_bytes = f"/{self.total_bytes / 1e9:.1f}" if self.total_bytes is not None else ""
        print(
            f"{self.label}: {'done, ' if final else ''}{self.items}{of_items} objects, "
            f"{self.bytes / 1e9:.2f}{of_bytes} GB read in {elapsed:.0f} s "
            f"({self.bytes / elapsed / 1e6:.0f} MB/s); "
            f"{self.verb} {self.uploaded} ({self.uploaded_bytes / 1e9:.2f} GB, "
            f"{self.uploaded / elapsed:.0f}/s)",
            file=sys.stderr,
            flush=True,
        )
