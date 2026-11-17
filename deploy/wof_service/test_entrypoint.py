"""Tests of entrypoint.sh against a local bucket, with `wof_service` faked.

Run with `python3 deploy/wof_service/test_entrypoint.py` (needs curl and jq). The bucket is
served over HTTP with range requests, as the real one is, and logs each request so a test can
tell what was downloaded. It can also drop connections partway through the database, or
answer with errors, to test retrying.
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
RELEASE = "2026-11-17"
FILE = "whosonfirst-data-admin.spatial.db"


class Bucket(http.server.ThreadingHTTPServer):
    """Serves `root` with single-range support; `requests` logs (path, Range header)."""

    def __init__(self, root: Path):
        self.root = root
        self.requests = []
        self.corrupt = 0  # serve the database with its bytes changed this many times
        self.drops = []  # for each of the next requests for the database, bytes to send before dropping
        self.errors = []  # HTTP statuses to answer the next requests for the database with
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
        is_db = path.name == FILE
        if is_db and server.errors:
            self.send_error(server.errors.pop(0))
            return
        if is_db and server.corrupt:
            server.corrupt -= 1
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
        if is_db and server.drops:
            # Promise the whole body, send part of it, and hang up.
            self.wfile.write(body[:server.drops.pop(0)])
            self.wfile.flush()
            self.close_connection = True
            return
        self.wfile.write(body)


class EntrypointTest(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.volume = self.tmp / "data"
        self.volume.mkdir()
        self.bucket_root = self.tmp / "bucket"
        self.db = os.urandom(300_000)
        self.publish(self.db)
        self.bucket = Bucket(self.bucket_root)
        threading.Thread(target=self.bucket.serve_forever, daemon=True).start()
        self.addCleanup(self.bucket.server_close)
        self.addCleanup(self.bucket.shutdown)
        # A fake wof_service on PATH records its arguments and WOF_DB.
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.log = self.tmp / "calls"
        fake = self.bin / "wof_service"
        fake.write_text(f'#!/bin/sh\necho "wof_service WOF_DB=$WOF_DB $*" >> "{self.log}"\n')
        fake.chmod(0o755)

    def publish(self, db, release=RELEASE, kind="wof"):
        prefix = self.bucket_root / "releases/wof" / release
        prefix.mkdir(parents=True, exist_ok=True)
        (prefix / FILE).write_bytes(db)
        (prefix / "release.json").write_text(json.dumps({
            "kind": kind,
            "date": release,
            "database": {"file": FILE, "bytes": len(db), "sha256": hashlib.sha256(db).hexdigest()},
        }))

    def run_entrypoint(self, release=RELEASE, **env):
        env = {
            "PATH": f"{self.bin}:{os.environ['PATH']}",
            "WOF_RELEASE": release,
            "WOF_RELEASES_URL": f"http://127.0.0.1:{self.bucket.server_port}/releases",
            "WOF_VOLUME": str(self.volume),
            "WOF_RETRY_DELAY": "0",
            **env,
        }
        return subprocess.run([str(ENTRYPOINT), "--extra"], env=env, capture_output=True, text=True, timeout=60)

    def calls(self):
        return self.log.read_text().splitlines() if self.log.exists() else []

    def db_requests(self):
        return [entry for entry in self.bucket.requests if entry[0].endswith(FILE)]

    def sha(self, data=None):
        return hashlib.sha256(self.db if data is None else data).hexdigest()

    def install(self, data, sidecar):
        """A database on the volume, as a previous start left it."""
        (self.volume / FILE).write_bytes(data)
        if sidecar is not None:
            (self.volume / f"{FILE}.sha256").write_text(sidecar + "\n")

    def assert_served(self, result):
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual((self.volume / FILE).read_bytes(), self.db)
        self.assertEqual((self.volume / f"{FILE}.sha256").read_text(), self.sha() + "\n")
        self.assertEqual(self.calls(), [f"wof_service WOF_DB={self.volume / FILE} --extra"])

    def assert_untouched_failure(self, result, message, data=b"the release being served"):
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(message, result.stderr)
        self.assertEqual((self.volume / FILE).read_bytes(), data)
        self.assertEqual(self.calls(), [])

    def test_downloads_into_an_empty_volume(self):
        self.assert_served(self.run_entrypoint())
        self.assertEqual(len(self.db_requests()), 1)
        self.assertEqual(sorted(p.name for p in self.volume.iterdir()), [FILE, f"{FILE}.sha256"])

    def test_uses_a_recorded_database_without_downloading_or_hashing(self):
        self.install(self.db, self.sha())
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.db_requests(), [])
        self.assertNotIn("checking the SHA-256", self.run_entrypoint().stderr)

    def test_a_database_without_a_sidecar_is_deleted_and_downloaded(self):
        # Even with the right bytes: nothing recorded that a download of it was checked.
        self.install(self.db, None)
        result = self.run_entrypoint()
        self.assert_served(result)
        self.assertIn("no recorded SHA-256", result.stderr)
        self.assertEqual(len(self.db_requests()), 1)

    def test_another_releases_database_is_deleted_before_anything_else(self):
        self.install(b"an older release's database", self.sha(b"an older release's database"))
        (self.volume / "stray").mkdir()
        (self.volume / "lost+found").mkdir()
        result = self.run_entrypoint()
        self.assert_served(result)
        self.assertIn("isn't wof 2026-11-17's database", result.stderr)
        # Deleted before the download started.
        self.assertLess(result.stderr.index("deleting it"), result.stderr.index("downloading"))
        self.assertEqual(sorted(p.name for p in self.volume.iterdir()), sorted([FILE, f"{FILE}.sha256", "lost+found"]))

    def test_a_truncated_database_with_the_right_sidecar_is_deleted(self):
        self.install(self.db[:1000], self.sha())
        self.assert_served(self.run_entrypoint())
        self.assertEqual(len(self.db_requests()), 1)

    def test_a_stale_sidecar_without_a_database_is_deleted(self):
        (self.volume / f"{FILE}.sha256").write_text(self.sha() + "\n")
        result = self.run_entrypoint()
        self.assert_served(result)
        self.assertIn("has no database beside it", result.stderr)
        self.assertEqual(len(self.db_requests()), 1)

    def test_resumes_a_partial_download_of_this_release(self):
        (self.volume / f"{FILE}-{RELEASE}.partial").write_bytes(self.db[:100_000])
        (self.volume / f"{FILE}-2026-11-01.partial").write_bytes(b"another release's")
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.db_requests(), [(f"/releases/wof/{RELEASE}/{FILE}", "bytes=100000-")])
        self.assertEqual(sorted(p.name for p in self.volume.iterdir()), [FILE, f"{FILE}.sha256"])

    def test_a_complete_partial_download_needs_no_request(self):
        (self.volume / f"{FILE}-{RELEASE}.partial").write_bytes(self.db)
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.db_requests(), [])

    def test_retries_dropped_connections_resuming_each_time(self):
        self.bucket.drops = [50_000, 70_000]
        self.bucket.errors = []
        result = self.run_entrypoint()
        self.assert_served(result)
        self.assertEqual([r for _, r in self.db_requests()], [None, "bytes=50000-", "bytes=120000-"])
        self.assertIn("attempt 1 failed", result.stderr)
        self.assertIn("with 50000 of 300000 bytes", result.stderr)
        self.assertIn("attempt 3 of 30: from byte 120000", result.stderr)

    def test_retries_server_errors(self):
        self.bucket.errors = [503, 500]
        self.assert_served(self.run_entrypoint())
        self.assertEqual(len(self.db_requests()), 3)

    def test_gives_up_after_its_attempts_keeping_the_partial(self):
        self.bucket.drops = [10_000, 10_000, 10_000]
        result = self.run_entrypoint(WOF_DOWNLOAD_ATTEMPTS="3")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("gave up downloading", result.stderr)
        self.assertIn("with 30000 of 300000 bytes", result.stderr)
        self.assertEqual((self.volume / f"{FILE}-{RELEASE}.partial").read_bytes(), self.db[:30_000])
        self.assertEqual(self.calls(), [])
        # The next start resumes it.
        self.assert_served(self.run_entrypoint())
        self.assertEqual(self.db_requests()[-1][1], "bytes=30000-")

    def test_a_missing_file_isnt_retried(self):
        self.bucket.errors = [404]
        result = self.run_entrypoint()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("answered HTTP 404", result.stderr)
        self.assertEqual(len(self.db_requests()), 1)

    def test_a_corrupt_download_is_deleted_and_downloaded_once_more(self):
        self.bucket.corrupt = 1
        result = self.run_entrypoint()
        self.assert_served(result)
        self.assertIn("DOESN'T MATCH", result.stderr)
        self.assertEqual([r for _, r in self.db_requests()], [None, None])

    def test_a_download_that_fails_its_check_twice_gives_up(self):
        self.bucket.corrupt = 5
        result = self.run_entrypoint()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("failed its size and SHA-256 check again", result.stderr)
        self.assertEqual(len(self.db_requests()), 2)
        self.assertEqual(list(self.volume.iterdir()), [])
        self.assertEqual(self.calls(), [])

    def test_refuses_a_missing_release_and_leaves_the_volume(self):
        self.install(b"the release being served", "anything")
        result = self.run_entrypoint(release="2026-12-01")
        self.assert_untouched_failure(result, "is WOF_RELEASE published")
        self.assertEqual(self.db_requests(), [])

    def test_refuses_another_kind_and_leaves_the_volume(self):
        self.publish(self.db, kind="geocoder")
        self.install(b"the release being served", "anything")
        result = self.run_entrypoint()
        self.assert_untouched_failure(result, "is a geocoder release of 2026-11-17, not wof 2026-11-17")
        self.assertEqual(self.db_requests(), [])

    def test_refuses_a_malformed_release_date(self):
        result = self.run_entrypoint(release="latest")
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("isn't YYYY-MM-DD", result.stderr)
        self.assertEqual(self.bucket.requests, [])

    def test_refuses_to_start_without_room(self):
        # df reports 1 KiB free.
        fake = self.bin / "df"
        fake.write_text("#!/bin/sh\necho 'Filesystem 1024-blocks Used Available Capacity Mounted on'\n"
                        "echo 'volume 100 99 1 99% /data'\n")
        fake.chmod(0o755)
        (self.volume / f"{FILE}-{RELEASE}.partial").write_bytes(self.db[:100_000])
        result = self.run_entrypoint()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("300000 bytes, 100000 of them downloaded, but", result.stderr)
        self.assertEqual(self.db_requests(), [])
        self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()
