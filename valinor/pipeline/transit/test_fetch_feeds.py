#!/usr/bin/env python3
"""Tests for the feed-fetching stage. They need no network and no Transitous checkout."""

import hashlib
import json
import tempfile
import unittest
from pathlib import Path
from unittest import mock

import fetch_feeds
from fetch_feeds import NATIONWIDE_REASON, Resolution, ResolvedSource

# Stands in for Transitous's `fetch.py`, driven by the source name.
FAKE_FETCH = """#!/usr/bin/env python3
import json, os, sys
from pathlib import Path
region = Path(sys.argv[1]).stem
name = json.load(open(sys.argv[1]))["sources"][0]["name"]
if name.startswith("bad"):
    print("Error: Could not fetch " + name + ": boom", file=sys.stderr)
    print("Error: 1 errors occurred during fetching.", file=sys.stderr)
    sys.exit(1)
if name.startswith("empty"):
    sys.exit(0)
Path("out", region + "_" + name + ".gtfs.zip").write_bytes(name.encode())
"""


# A later version of the fake, publishing different bytes under the same names.
FAKE_FETCH_REVISED = """#!/usr/bin/env python3
import json, sys
from pathlib import Path
region = Path(sys.argv[1]).stem
name = json.load(open(sys.argv[1]))["sources"][0]["name"]
Path("out", region + "_" + name + ".gtfs.zip").write_bytes(b"revised " + name.encode())
"""


class FakeResolver:
    """Resolves entries from a table, and fails a test if asked about anything else."""

    def __init__(self, table=None):
        self.table = table or {}

    def resolve(self, entry):
        return self.table[entry["name"]]


STATIC = Resolution((ResolvedSource("gtfs", "https://example.com/a.zip"),))


class ClassifyTests(unittest.TestCase):
    def classify(self, sources, table=None, excluded=frozenset()):
        return fetch_feeds.classify("r", sources, set(excluded), FakeResolver(table))

    def test_excluded_source_is_skipped_before_resolution(self):
        plan = self.classify([{"name": "Amtrak", "type": "http"}], excluded={"Amtrak"})
        self.assertEqual(plan.skipped, [{"id": "r-Amtrak", "reason": NATIONWIDE_REASON}])
        self.assertEqual(plan.static, [])

    def test_exclusion_wins_over_upstream_skip(self):
        plan = self.classify(
            [{"name": "Amtrak", "type": "http", "skip": True, "skip-reason": "x"}],
            excluded={"Amtrak"},
        )
        self.assertEqual(plan.skipped[0]["reason"], NATIONWIDE_REASON)

    def test_exclusion_is_exact(self):
        plan = self.classify(
            [{"name": "amtrak", "type": "http"}], {"amtrak": STATIC}, excluded={"Amtrak"}
        )
        self.assertEqual([feed.id for feed in plan.static], ["r-amtrak"])

    def test_upstream_skip_keeps_its_reason(self):
        plan = self.classify([{"name": "A", "type": "http", "skip": True, "skip-reason": "Expired"}])
        self.assertIn("Expired", plan.skipped[0]["reason"])

    def test_feed_proxy_is_skipped(self):
        plan = self.classify([{"name": "A", "type": "http", "use-feed-proxy": True}])
        self.assertIn("feed proxy", plan.skipped[0]["reason"])

    def test_nested_encrypted_value_is_skipped(self):
        entry = {
            "name": "A",
            "type": "http",
            "http-options": {"headers": {"X-Key": "AGE-ENCRYPTED:abc"}},
        }
        plan = self.classify([entry])
        self.assertIn("encrypted keys", plan.skipped[0]["reason"])

    def test_source_level_resolution_error_is_skipped_without_an_outage(self):
        resolution = Resolution(error="no key", source_level=True)
        plan = self.classify([{"name": "A", "type": "http"}], {"A": resolution})
        self.assertIn("no key", plan.skipped[0]["reason"])
        self.assertEqual(plan.outages, [])

    def test_any_other_resolution_error_is_an_outage(self):
        plan = self.classify([{"name": "A", "type": "http"}], {"A": Resolution(error="503")})
        self.assertIn("503", plan.outages[0])

    def test_realtime_urls_are_listed_with_the_feed_id(self):
        resolution = Resolution(
            (ResolvedSource("gtfs-rt", "https://a/1.pb"), ResolvedSource("gtfs-rt", "https://a/2.pb"))
        )
        plan = self.classify([{"name": "A", "type": "url"}], {"A": resolution})
        self.assertEqual(
            plan.realtime,
            [
                {"id": "r-A", "name": "A", "url": "https://a/1.pb"},
                {"id": "r-A", "name": "A", "url": "https://a/2.pb"},
            ],
        )
        self.assertEqual(plan.static, [])

    def test_static_and_realtime_from_one_entry(self):
        resolution = Resolution(
            (ResolvedSource("gtfs", "https://a/s.zip"), ResolvedSource("gtfs-rt", "https://a/r.pb"))
        )
        plan = self.classify([{"name": "A", "type": "http"}], {"A": resolution})
        self.assertEqual(len(plan.static), 1)
        self.assertEqual(len(plan.realtime), 1)

    def test_other_spec_is_skipped(self):
        resolution = Resolution((ResolvedSource("gbfs", "https://a/gbfs.json"),))
        plan = self.classify([{"name": "A", "type": "url"}], {"A": resolution})
        self.assertIn("spec gbfs", plan.skipped[0]["reason"])

    def test_duplicate_static_name_is_skipped_rather_than_overwritten(self):
        plan = self.classify(
            [{"name": "A", "type": "http"}, {"name": "A", "type": "http", "url": "u"}],
            {"A": STATIC},
        )
        self.assertEqual(len(plan.static), 1)
        self.assertIn("duplicate", plan.skipped[0]["reason"])

    def test_license_fields_are_copied_and_the_region_file_wins(self):
        resolution = Resolution(
            STATIC.sources, license={"spdx_identifier": "X", "url": "from-database"}
        )
        entry = {"name": "A", "type": "http", "license": {"url": "from-region"}}
        plan = self.classify([entry], {"A": resolution})
        self.assertEqual(plan.static[0].license, {"spdx_identifier": "X", "url": "from-region"})
        self.assertEqual(plan.static[0].entry, entry)


class ReasonTests(unittest.TestCase):
    def test_keeps_errors_and_drops_the_final_tally(self):
        output = "Fetching x\nError: Could not fetch x: nope\nError: 1 errors occurred during fetching.\n"
        self.assertEqual(fetch_feeds.failure_reason(1, output), "Error: Could not fetch x: nope")

    def test_puts_the_gtfsclean_cause_first(self):
        output = (
            "Parsing ...\nError while parsing GTFS feed:\nin trip 1 the times are wrong\n\n"
            "Error: Could not postprocess x: Command failed\n"
        )
        self.assertEqual(
            fetch_feeds.failure_reason(1, output),
            "gtfsclean: in trip 1 the times are wrong; Error: Could not postprocess x: Command failed",
        )

    def test_falls_back_to_the_last_line(self):
        self.assertEqual(fetch_feeds.failure_reason(1, "a\nTraceback tail\n"), "Traceback tail")

    def test_falls_back_to_the_status(self):
        self.assertEqual(fetch_feeds.failure_reason(3, ""), "fetch.py exited with status 3")

    def test_truncates(self):
        reason = fetch_feeds.failure_reason(1, "Error: " + "x" * 5000)
        self.assertLessEqual(len(reason), fetch_feeds.MAX_REASON_LENGTH + 1)


class RealtimeManifestTests(unittest.TestCase):
    def test_repeated_urls_are_listed_once(self):
        realtime = [
            {"id": "b", "name": "B", "url": "u"},
            {"id": "a", "name": "A", "url": "z"},
            {"id": "b", "name": "B", "url": "u"},
        ]
        self.assertEqual(
            fetch_feeds.sorted_unique_realtime(realtime),
            [{"id": "a", "name": "A", "url": "z"}, {"id": "b", "name": "B", "url": "u"}],
        )


class ExclusionFileTests(unittest.TestCase):
    def test_comments_and_blanks_are_ignored(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "excluded.txt"
            path.write_text("# comment\n\nAmtrak\nflixbus  # trailing\n", encoding="utf-8")
            self.assertEqual(fetch_feeds.load_exclusions(path), {"Amtrak", "flixbus"})


class StageTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        root = Path(self.temp.name)
        self.transitous = root / "transitous"
        (self.transitous / "src").mkdir(parents=True)
        (self.transitous / "transitland-atlas").mkdir()
        (self.transitous / "src" / "fetch.py").write_text(FAKE_FETCH, encoding="utf-8")
        self.input = root / "input"
        self.input.mkdir()
        self.output = root / "output"
        self.exclusions = root / "excluded.txt"
        self.exclusions.write_text("Amtrak\n", encoding="utf-8")

    def args(self, *extra):
        return fetch_feeds.parse_args(
            [
                "--input",
                str(self.input),
                "--output",
                str(self.output),
                "--transitous",
                str(self.transitous),
                "--exclusions",
                str(self.exclusions),
                "--commit",
                "abc",
                "--region",
                "r",
                *extra,
            ]
        )

    def write_region(self, names):
        sources = [{"name": name, "type": "http", "url": "u"} for name in names]
        (self.input / "r.json").write_text(json.dumps({"sources": sources}), encoding="utf-8")

    def run_stage(self, names):
        self.write_region(names)
        resolver = FakeResolver(dict.fromkeys(names, STATIC))
        status = fetch_feeds.run(self.args(), resolver)
        manifest = json.loads((self.output / "feeds.json").read_text(encoding="utf-8"))
        return status, manifest

    def test_failures_do_not_stop_the_rest(self):
        status, manifest = self.run_stage(["good-b", "bad", "empty", "good-a", "Amtrak"])
        self.assertEqual(status, 0)
        self.assertEqual([feed["id"] for feed in manifest["feeds"]], ["r-good-a", "r-good-b"])
        self.assertEqual(
            manifest["skipped"],
            [
                {"id": "r-Amtrak", "reason": NATIONWIDE_REASON},
                {"id": "r-bad", "reason": "Error: Could not fetch bad: boom"},
                {"id": "r-empty", "reason": "fetch.py produced no output"},
            ],
        )

    def test_manifest_entry_describes_the_file(self):
        _, manifest = self.run_stage(["good"])
        feed = manifest["feeds"][0]
        self.assertEqual(manifest["transitous_commit"], "abc")
        self.assertEqual(feed["file"], "feeds/r-good.gtfs.zip")
        self.assertEqual(feed["bytes"], len(b"good"))
        self.assertEqual(feed["sha256"], hashlib.sha256(b"good").hexdigest())
        self.assertTrue((self.output / feed["file"]).is_file())

    def test_a_feed_that_stops_working_leaves_the_output(self):
        self.run_stage(["good", "other"])
        self.run_stage(["other", "bad"])
        files = sorted(path.name for path in (self.output / "feeds").iterdir())
        self.assertEqual(files, ["r-other.gtfs.zip"])

    def test_excluding_a_fetched_feed_removes_it_everywhere(self):
        self.run_stage(["good"])
        self.exclusions.write_text("good\n", encoding="utf-8")
        status, manifest = self.run_stage(["good", "kept"])
        self.assertEqual(status, 0)
        self.assertEqual([feed["id"] for feed in manifest["feeds"]], ["r-kept"])
        self.assertFalse((self.output / "feeds" / "r-good.gtfs.zip").exists())
        cached = self.output / ".cache" / "work" / "out" / "r_good.gtfs.zip"
        self.assertFalse(cached.exists())

    def test_no_feeds_is_a_failure(self):
        status, manifest = self.run_stage(["bad"])
        self.assertEqual(status, 1)
        self.assertEqual(manifest["feeds"], [])

    def assert_listing_matches_archives(self):
        listing = json.loads((self.output / "feeds.json").read_text(encoding="utf-8"))
        on_disk = sorted(path.name for path in (self.output / "feeds").iterdir())
        self.assertEqual(on_disk, sorted(Path(feed["file"]).name for feed in listing["feeds"]))
        for feed in listing["feeds"]:
            actual = hashlib.sha256((self.output / feed["file"]).read_bytes()).hexdigest()
            self.assertEqual(actual, feed["sha256"], feed["id"])
        return listing

    def revise_and_run(self, names):
        """Runs the stage again with every archive changed, for runs that are made to die."""
        (self.transitous / "src" / "fetch.py").write_text(FAKE_FETCH_REVISED, encoding="utf-8")
        self.write_region(names)
        return fetch_feeds.run(self.args(), FakeResolver(dict.fromkeys(names, STATIC)))

    def test_a_resolver_outage_is_not_success_and_keeps_feeds(self):
        self.assertEqual(self.run_stage(["good", "mdb-feed"])[0], 0)
        before = (self.output / "feeds.json").read_bytes()
        outage = {
            "good": STATIC,
            "mdb-feed": Resolution(error="could not load the Mobility Database export: 503"),
        }
        self.write_region(["good", "mdb-feed"])
        self.assertNotEqual(fetch_feeds.run(self.args(), FakeResolver(outage)), 0)
        self.assertEqual((self.output / "feeds.json").read_bytes(), before)
        self.assertEqual(len(self.assert_listing_matches_archives()["feeds"]), 2)

    def test_a_source_level_resolution_error_is_still_success(self):
        table = {"good": STATIC, "keyed": Resolution(error="no key", source_level=True)}
        self.write_region(["good", "keyed"])
        self.assertEqual(fetch_feeds.run(self.args(), FakeResolver(table)), 0)
        self.assertEqual(len(self.assert_listing_matches_archives()["feeds"]), 1)

    def test_a_run_where_every_feed_fails_keeps_feeds(self):
        self.assertEqual(self.run_stage(["good"])[0], 0)
        before = (self.output / "feeds.json").read_bytes()
        (self.transitous / "src" / "fetch.py").write_text(
            "import sys\nprint('Error: down', file=sys.stderr)\nsys.exit(1)\n", encoding="utf-8"
        )
        self.write_region(["good"])
        self.assertEqual(fetch_feeds.run(self.args(), FakeResolver({"good": STATIC})), 1)
        self.assertEqual((self.output / "feeds.json").read_bytes(), before)
        self.assert_listing_matches_archives()

    def test_a_killed_publish_leaves_feeds_and_listing_consistent(self):
        self.assertEqual(self.run_stage(["good", "other"])[0], 0)
        # The listing's temporary path is taken, so writing it fails.
        (self.output / ".tmp-feeds.json").mkdir()
        with self.assertRaises(OSError):
            self.revise_and_run(["good", "other"])
        self.assert_listing_matches_archives()

    def test_a_kill_before_the_switch_keeps_the_old_pair(self):
        self.assertEqual(self.run_stage(["good", "other"])[0], 0)
        with mock.patch.object(fetch_feeds, "switch_generation", side_effect=OSError):
            with self.assertRaises(OSError):
                self.revise_and_run(["good"])
        self.assertEqual(len(self.assert_listing_matches_archives()["feeds"]), 2)

    def test_a_kill_after_the_switch_has_the_new_pair(self):
        self.assertEqual(self.run_stage(["good", "other"])[0], 0)
        with mock.patch.object(fetch_feeds, "prune_generations", side_effect=OSError):
            with self.assertRaises(OSError):
                self.revise_and_run(["good"])
        self.assertEqual(len(self.assert_listing_matches_archives()["feeds"]), 1)

    def test_a_run_after_a_killed_one_cleans_up(self):
        self.assertEqual(self.run_stage(["good"])[0], 0)
        with mock.patch.object(fetch_feeds, "switch_generation", side_effect=OSError):
            with self.assertRaises(OSError):
                self.revise_and_run(["good"])
        self.assertEqual(self.revise_and_run(["good"]), 0)
        self.assertEqual(len(list((self.output / fetch_feeds.GENERATIONS).iterdir())), 1)
        self.assert_listing_matches_archives()

    def test_a_flat_output_is_refused_rather_than_replaced(self):
        (self.output / "feeds").mkdir(parents=True)
        (self.output / "feeds" / "kept").write_text("x", encoding="utf-8")
        self.write_region(["good"])
        self.assertEqual(fetch_feeds.run(self.args(), FakeResolver({"good": STATIC})), 2)
        self.assertTrue((self.output / "feeds" / "kept").exists())

    def test_region_names_cannot_be_patterns(self):
        args = self.args()
        args.region = ["../r"]
        self.assertEqual(fetch_feeds.run(args, FakeResolver()), 2)


if __name__ == "__main__":
    unittest.main()
