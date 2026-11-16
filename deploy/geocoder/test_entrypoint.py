"""Tests of entrypoint.sh against a local bucket, with `mount` and `airmail_service` faked.

Run with `python3 deploy/geocoder/test_entrypoint.py` (needs curl and jq). The
bucket is served over HTTP with range requests, as the real one is, and logs each request so a
test can tell what was downloaded.
"""

import hashlib
import http.server
import json
import os
import shutil
import subprocess
import tempfile
import threading
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ENTRYPOINT = HERE / "entrypoint.sh"
RELEASE = "2026-11-15"
SCHEMA = 7


class Bucket(http.server.ThreadingHTTPServer):
    """Serves `root` with single-range support; `requests` logs (path, Range header)."""

    def __init__(self, root: Path):
        self.root = root
        self.requests = []
        self.corrupt = False  # serve the image with its bytes changed
        super().__init__(("127.0.0.1", 0), Handler)


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        server = self.server
        server.requests.append((self.path, self.headers.get("Range")))
        path = server.root / self.path.lstrip("/")
        if not path.is_file():
            self.send_error(404)
            return
        data = path.read_bytes()
        if server.corrupt and path.suffix == ".btrfs":
            data = bytes(b ^ 1 for b in data)
        start = 0
        if (spec := self.headers.get("Range")) and spec.startswith("bytes="):
            start = int(spec[len("bytes="):].split("-")[0])
            if start >= len(data):
                self.send_response(416)
                self.send_header("Content-Range", f"bytes */{len(data)}")
                self.end_headers()
                return
            self.send_response(206)
            self.send_header("Content-Range", f"bytes {start}-{len(data) - 1}/{len(data)}")
        else:
            self.send_response(200)
        body = data[start:]
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)


class EntrypointTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.volume = self.tmp / "data"
        self.volume.mkdir()
        self.bucket_root = self.tmp / "bucket"
        self.image = os.urandom(300_000)
        self.publish(self.image)
        self.bucket = Bucket(self.bucket_root)
        threading.Thread(target=self.bucket.serve_forever, daemon=True).start()
        self.addCleanup(self.bucket.server_close)
        self.addCleanup(self.bucket.shutdown)
        # Fakes on PATH: mount records its arguments, airmail_service records its own.
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.log = self.tmp / "calls"
        for name in ("mount", "airmail_service"):
            fake = self.bin / name
            fake.write_text(f'#!/bin/sh\necho "{name} $*" >> "{self.log}"\n')
            fake.chmod(0o755)
        self.schema_file = self.tmp / "schema-version"
        self.schema_file.write_text(f"{SCHEMA}\n")

    def publish(self, image, schema=SCHEMA, release=RELEASE):
        prefix = self.bucket_root / "releases/geocoder" / release
        prefix.mkdir(parents=True, exist_ok=True)
        (prefix / "geocoder.btrfs").write_bytes(image)
        (prefix / "release.json").write_text(json.dumps({
            "kind": "geocoder",
            "date": release,
            "image": {"file": "geocoder.btrfs", "bytes": len(image), "sha256": hashlib.sha256(image).hexdigest()},
            "schema_version": schema,
        }))

    def run_entrypoint(self, release=RELEASE):
        env = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "GEOCODER_RELEASE": release,
            "GEOCODER_RELEASES_URL": f"http://127.0.0.1:{self.bucket.server_port}/releases",
            "GEOCODER_VOLUME": str(self.volume),
            "GEOCODER_MOUNT": str(self.tmp / "mnt"),
            "GEOCODER_SCHEMA_FILE": str(self.schema_file),
        }
        return subprocess.run([str(ENTRYPOINT), "--extra"], env=env, capture_output=True, text=True, timeout=60)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def image_requests(self):
        return [entry for entry in self.bucket.requests if entry[0].endswith(".btrfs")]

    def assert_served(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.volume / "geocoder.btrfs").read_bytes(), self.image)
        mnt = self.tmp / "mnt"
        self.assertEqual(self.calls(), [
            f"mount -o loop,ro {self.volume / 'geocoder.btrfs'} {mnt}",
            f"airmail_service --index {mnt}/index --categories {mnt}/categories "
            f"--tagger {mnt}/tagger.model --extra",
        ])

    def test_downloads_into_an_empty_volume(self):
        self.assert_served(self.run_entrypoint())
        self.assertEqual(len(self.image_requests()), 1)

    def test_uses_a_matching_image_without_downloading(self):
        (self.volume / "geocoder.btrfs").write_bytes(self.image)
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.image_requests(), [])

    def test_replaces_another_release_and_clears_the_volume(self):
        (self.volume / "geocoder.btrfs").write_bytes(b"an older release's image")
        (self.volume / "geocoder-2026-11-01.partial").write_bytes(b"left over")
        (self.volume / "stray").mkdir()
        (self.volume / "lost+found").mkdir()
        self.assert_served(self.run_entrypoint())
        self.assertEqual(sorted(p.name for p in self.volume.iterdir()), ["geocoder.btrfs", "lost+found"])

    def test_resumes_a_partial_download_of_this_release(self):
        (self.volume / f"geocoder-{RELEASE}.partial").write_bytes(self.image[:100_000])
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.image_requests(), [(f"/releases/geocoder/{RELEASE}/geocoder.btrfs", "bytes=100000-")])

    def test_a_complete_partial_download_needs_no_request(self):
        (self.volume / f"geocoder-{RELEASE}.partial").write_bytes(self.image)
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.image_requests(), [])

    def test_refuses_a_download_that_doesnt_match(self):
        self.bucket.corrupt = True
        result = self.run_entrypoint()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("doesn't match", result.stderr)
        self.assertEqual(list(self.volume.iterdir()), [])
        self.assertEqual(self.calls(), [])

    def test_refuses_a_missing_release_and_leaves_the_volume(self):
        (self.volume / "geocoder.btrfs").write_bytes(b"the release being served")
        result = self.run_entrypoint(release="2026-12-01")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("is GEOCODER_RELEASE published", result.stderr)
        self.assertEqual((self.volume / "geocoder.btrfs").read_bytes(), b"the release being served")
        self.assertEqual(self.calls(), [])

    def test_refuses_another_schema_and_leaves_the_volume(self):
        self.publish(self.image, schema=SCHEMA + 1)
        (self.volume / "geocoder.btrfs").write_bytes(b"the release being served")
        result = self.run_entrypoint()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(f"index schema {SCHEMA + 1}", result.stderr)
        self.assertEqual((self.volume / "geocoder.btrfs").read_bytes(), b"the release being served")
        self.assertEqual(self.image_requests(), [])

    def test_refuses_a_malformed_release_date(self):
        result = self.run_entrypoint(release="latest")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("isn't YYYY-MM-DD", result.stderr)
        self.assertEqual(self.bucket.requests, [])


if __name__ == "__main__":
    unittest.main()
