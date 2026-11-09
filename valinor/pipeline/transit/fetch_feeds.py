#!/usr/bin/env python3
"""Fetch and clean the GTFS feeds of named Transitous regions.

Transitous's `src/fetch.py` does the downloading and cleaning.
This wrapper decides what it may touch, runs it once per source, and writes the manifest.

Running one process per source, with a region file holding only that source, buys two things:
a process failure (`fetch.py` calls `sys.exit` on an unresolvable source) costs one feed,
and the output of each process explains that feed's failure.
"""

import argparse
import contextlib
import dataclasses
import datetime
import hashlib
import io
import json
import os
import re
import signal
import subprocess
import sys
import time
from collections.abc import Iterable
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path
from typing import Any, Protocol

NATIONWIDE_REASON = "nationwide agency"
ENCRYPTED_SENTINEL = "AGE-ENCRYPTED:"
REGION_PATTERN = re.compile(r"^[A-Za-z0-9-]+$")
MAX_REASON_LENGTH = 600
Entry = dict[str, Any]


@dataclasses.dataclass(frozen=True)
class ResolvedSource:
    """One concrete feed a source entry points at."""

    spec: str
    url: str | None = None


@dataclasses.dataclass(frozen=True)
class Resolution:
    """What a source entry resolved to, or why it could not be."""

    sources: tuple[ResolvedSource, ...] = ()
    # Fields from the upstream database, under the names it uses.
    license: dict[str, Any] = dataclasses.field(default_factory=dict)
    error: str | None = None


class Resolver(Protocol):
    def resolve(self, entry: Entry) -> Resolution: ...


class TransitousResolver:
    """Resolves source entries with Transitous's own Atlas and Mobility Database readers.

    Imports them on construction, because they exist only inside the container.
    Must be constructed with the working directory set to the fetch working directory:
    both readers use relative paths.
    """

    def __init__(self, transitous: Path):
        sys.path.insert(0, str(transitous / "src"))
        # pylint: disable=import-outside-toplevel
        import metadata
        import mobilitydatabase
        import transitland

        self.metadata = metadata
        self.atlas = transitland.Atlas.load(Path("transitland-atlas/"))
        self.database = None
        self.database_error = None
        # Mobility Database URLs change, so a cached export must not outlive a run.
        Path("mobilitydatabase.csv").unlink(missing_ok=True)
        try:
            self.database = mobilitydatabase.Database.load()
        except Exception as error:  # pylint: disable=broad-exception-caught
            self.database_error = f"could not load the Mobility Database export: {error}"

    def resolve(self, entry: Entry) -> Resolution:
        warnings = io.StringIO()
        try:
            with contextlib.redirect_stderr(warnings):
                return self._resolve(entry, warnings)
        except Exception as error:  # pylint: disable=broad-exception-caught
            return Resolution(error=f"could not resolve source: {error}")

    def _resolve(self, entry: Entry, warnings: io.StringIO) -> Resolution:
        match entry["type"]:
            case "transitland-atlas":
                source = self.metadata.TransitlandSource(entry)
                results = self.atlas.sources_by_id(source)
                feed = self.atlas.by_id.get(source.transitland_atlas_id, {})
                license_fields = dict(feed.get("license", {}))
                unresolved = f"could not resolve {source.transitland_atlas_id}"
            case "mobility-database":
                if self.database is None:
                    return Resolution(error=self.database_error)
                source = self.metadata.MobilityDatabaseSource(entry)
                result = self.database.source_by_id(source)
                results = [result] if result else None
                row = self.database.by_id.get(source.mdb_id, {})
                # Copied under the export's own column names. Transitous reads "license",
                # but the export's column is "urls.license", so keep whichever has a value.
                license_fields = {
                    key: row[key] for key in ("license", "urls.license") if row.get(key)
                }
                unresolved = f"could not resolve {source.mdb_id}"
            case _:
                return Resolution((ResolvedSource(entry.get("spec", "gtfs"), entry.get("url")),))

        if not results:
            return Resolution(error=warnings.getvalue().strip() or unresolved)
        return Resolution(
            tuple(ResolvedSource(result.spec, result.url) for result in results),
            license_fields,
        )


@dataclasses.dataclass(frozen=True)
class StaticFeed:
    """A static GTFS source to download and clean."""

    id: str
    region: str
    name: str
    entry: Entry
    license: dict[str, Any]


@dataclasses.dataclass
class Plan:
    """What to do with a region's sources, decided before anything is downloaded."""

    static: list[StaticFeed] = dataclasses.field(default_factory=list)
    skipped: list[dict[str, str]] = dataclasses.field(default_factory=list)
    realtime: list[dict[str, str]] = dataclasses.field(default_factory=list)
    excluded: list[StaticFeed] = dataclasses.field(default_factory=list)


def source_reference(entry: Entry) -> str:
    """Names one source entry, since a feed's static and real-time sources share a name."""
    for key in ("transitland-atlas-id", "mdb-id", "url"):
        if key in entry:
            return str(entry[key])
    return str(entry.get("type", "unknown"))


def contains_encrypted(value: Any) -> bool:
    """Whether any string in a source entry is encrypted with Transitous's keys."""
    if isinstance(value, str):
        return value.startswith(ENCRYPTED_SENTINEL)
    if isinstance(value, dict):
        return any(contains_encrypted(item) for item in value.values())
    if isinstance(value, list):
        return any(contains_encrypted(item) for item in value)
    return False


def merged_license(entry: Entry, resolution: Resolution) -> dict[str, Any]:
    """Copies license fields without interpreting them.

    Upstream database fields keep their own names.
    Where a name collides, the region file wins, as it does in Transitous's own reader.
    """
    return {**resolution.license, **entry.get("license", {})}


def classify(
    region: str, sources: Iterable[Entry], excluded: set[str], resolver: Resolver
) -> Plan:
    """Sorts a region's sources into feeds to fetch, skipped sources, and real-time URLs."""
    plan = Plan()
    seen_static: set[str] = set()

    for entry in sources:
        name = entry["name"]
        feed_id = f"{region}-{name}"
        reference = source_reference(entry)

        # First, so that excluded sources never reach a resolver or a server.
        if name in excluded:
            plan.skipped.append({"id": feed_id, "reason": NATIONWIDE_REASON})
            plan.excluded.append(StaticFeed(feed_id, region, name, entry, {}))
            continue
        if entry.get("skip"):
            detail = entry.get("skip-reason") or "no reason given"
            plan.skipped.append(
                {"id": feed_id, "reason": f"skipped upstream ({reference}): {detail}"}
            )
            continue
        if entry.get("use-feed-proxy"):
            plan.skipped.append(
                {"id": feed_id, "reason": f"needs Transitous's feed proxy ({reference})"}
            )
            continue
        if contains_encrypted(entry):
            plan.skipped.append(
                {"id": feed_id, "reason": f"needs Transitous's encrypted keys ({reference})"}
            )
            continue

        resolution = resolver.resolve(entry)
        if resolution.error is not None:
            plan.skipped.append(
                {"id": feed_id, "reason": f"unresolved ({reference}): {resolution.error}"}
            )
            continue

        wants_static = False
        for resolved in resolution.sources:
            if resolved.spec == "gtfs":
                # `fetch.py` only ever downloads the first result of a source.
                wants_static = True
            elif resolved.spec == "gtfs-rt" and resolved.url:
                plan.realtime.append({"id": feed_id, "name": name, "url": resolved.url})
            else:
                plan.skipped.append(
                    {
                        "id": feed_id,
                        "reason": f"spec {resolved.spec} is not handled by this stage "
                        f"({reference})",
                    }
                )

        if wants_static:
            if feed_id in seen_static:
                # `fetch.py` names files after the source, so a second one would overwrite it.
                plan.skipped.append(
                    {"id": feed_id, "reason": f"duplicate source name in region ({reference})"}
                )
                continue
            seen_static.add(feed_id)
            plan.static.append(
                StaticFeed(feed_id, region, name, entry, merged_license(entry, resolution))
            )

    return plan


def load_exclusions(path: Path) -> set[str]:
    """Reads the exclusion list: one source name per line, with `#` comments."""
    names = set()
    for line in path.read_text(encoding="utf-8").splitlines():
        name = line.partition("#")[0].strip()
        if name:
            names.add(name)
    return names


def failure_reason(returncode: int, output: str) -> str:
    """Boils the output of a failed `fetch.py` run down to the reason for the manifest."""
    errors = [
        line.strip()
        for line in output.splitlines()
        if line.startswith("Error:") and "errors occurred during fetching" not in line
    ]
    # `fetch.py` reports a `gtfsclean` failure only as a failed command; the cause comes before it.
    lines = output.splitlines()
    for index, line in enumerate(lines[:-1]):
        if line.strip() == "Error while parsing GTFS feed:" and lines[index + 1].strip():
            errors.insert(0, f"gtfsclean: {lines[index + 1].strip()}")
            break
    if errors:
        reason = "; ".join(errors)
    else:
        lines = [line.strip() for line in output.splitlines() if line.strip()]
        reason = lines[-1] if lines else f"fetch.py exited with status {returncode}"
    if len(reason) > MAX_REASON_LENGTH:
        reason = reason[:MAX_REASON_LENGTH] + "…"
    return reason


def run_killing_group(command: list[str], cwd: Path, timeout: float) -> tuple[int, str]:
    """Runs a command, killing its whole process group on a timeout.

    `fetch.py` runs `gtfsclean` as a child, and `subprocess.run` would leave that one running.
    """
    process = subprocess.Popen(
        command,
        cwd=cwd,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        errors="replace",
        start_new_session=True,
    )
    try:
        output, _ = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        output, _ = process.communicate()
        return -1, f"{output}\nError: timed out after {timeout:.0f} seconds"
    return process.returncode, output


def log_name(feed_id: str) -> str:
    return re.sub(r"[^\w.-]", "_", feed_id) + ".log"


def fetch_one(
    feed: StaticFeed, index: int, transitous: Path, work: Path, logs: Path, timeout: float
) -> tuple[StaticFeed, Path | None, str | None]:
    """Fetches and cleans one feed; returns its cleaned file, or the reason there is none."""
    # `fetch.py` takes the region name from the file name, so that has to stay `<region>.json`.
    region_dir = work / "split" / f"{index:05d}"
    region_dir.mkdir(parents=True, exist_ok=True)
    region_file = region_dir / f"{feed.region}.json"
    region_file.write_text(
        json.dumps({"maintainers": [], "sources": [feed.entry]}), encoding="utf-8"
    )

    started = time.monotonic()
    returncode, output = run_killing_group(
        [sys.executable, str(transitous / "src" / "fetch.py"), str(region_file)], work, timeout
    )
    (logs / log_name(feed.id)).write_text(output, encoding="utf-8")
    print(f"{feed.id}: exit {returncode} in {time.monotonic() - started:.0f} s", flush=True)

    if returncode != 0:
        return feed, None, failure_reason(returncode, output)
    cleaned = work / "out" / f"{feed.region}_{feed.name}.gtfs.zip"
    if not cleaned.is_file():
        return feed, None, "fetch.py produced no output"
    return feed, cleaned, None


def publish(cleaned: Path, destination: Path) -> None:
    """Puts a cleaned feed in the output, sharing storage with the cache where it can."""
    destination.unlink(missing_ok=True)
    try:
        os.link(cleaned, destination)
    except OSError:
        # Hard links can't cross mounts, which separate cache and output volumes may do.
        temporary = destination.with_name(f".tmp-{destination.name}")
        temporary.write_bytes(cleaned.read_bytes())
        temporary.rename(destination)


def sha256_of(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def prepare_work_dir(cache: Path, transitous: Path) -> Path:
    """Lays out the working directory `fetch.py` expects, without writing to its checkout."""
    work = cache / "work"
    for directory in ("downloads", "out", "split"):
        (work / directory).mkdir(parents=True, exist_ok=True)
    for name in ("src", "transitland-atlas"):
        link = work / name
        if link.is_symlink() or link.exists():
            link.unlink()
        link.symlink_to(transitous / name)
    return work


def remove_excluded_from_cache(work: Path, excluded: Iterable[StaticFeed]) -> None:
    for feed in excluded:
        for directory in ("downloads", "out"):
            (work / directory / f"{feed.region}_{feed.name}.gtfs.zip").unlink(missing_ok=True)


def sorted_unique_skipped(skipped: Iterable[dict[str, str]]) -> list[dict[str, str]]:
    unique = {(item["id"], item["reason"]): item for item in skipped}
    return [unique[key] for key in sorted(unique)]


def sorted_unique_realtime(realtime: Iterable[dict[str, str]]) -> list[dict[str, str]]:
    """Sorts real-time URLs, keeping each once.

    A `url-override` replaces all three URLs of an Atlas feed, so a source can repeat one.
    """
    unique = {(item["id"], item["url"]): item for item in realtime}
    return [unique[key] for key in sorted(unique)]


def run(args: argparse.Namespace, resolver: Resolver | None = None) -> int:
    started = time.monotonic()
    for region in args.region:
        if not REGION_PATTERN.fullmatch(region):
            print(f"error: invalid region name {region!r}", file=sys.stderr)
            return 2
        if not (args.input / f"{region}.json").is_file():
            print(f"error: no region file for {region} in {args.input}", file=sys.stderr)
            return 2

    excluded = load_exclusions(args.exclusions)
    output = args.output.resolve()
    (output / "feeds").mkdir(parents=True, exist_ok=True)
    cache = args.cache.resolve() if args.cache else output / ".cache"
    work = prepare_work_dir(cache, args.transitous)
    logs = cache / "logs"
    logs.mkdir(parents=True, exist_ok=True)

    os.chdir(work)
    if resolver is None:
        resolver = TransitousResolver(args.transitous)

    plan = Plan()
    for region in args.region:
        with open(args.input / f"{region}.json", encoding="utf-8") as file:
            sources = json.load(file)["sources"]
        region_plan = classify(region, sources, excluded, resolver)
        plan.static += region_plan.static
        plan.skipped += region_plan.skipped
        plan.realtime += region_plan.realtime
        plan.excluded += region_plan.excluded
    remove_excluded_from_cache(work, plan.excluded)

    print(f"{len(plan.static)} static feeds to fetch with {args.jobs} jobs", flush=True)
    with ThreadPoolExecutor(max_workers=args.jobs) as pool:
        futures = [
            pool.submit(fetch_one, feed, index, args.transitous, work, logs, args.timeout)
            for index, feed in enumerate(plan.static)
        ]
        results = [future.result() for future in futures]

    feeds = []
    skipped = list(plan.skipped)
    for feed, cleaned, reason in results:
        if cleaned is None:
            skipped.append({"id": feed.id, "reason": reason or "unknown failure"})
            continue
        file_name = f"{feed.id}.gtfs.zip"
        destination = output / "feeds" / file_name
        publish(cleaned, destination)
        feeds.append(
            {
                "id": feed.id,
                "region": feed.region,
                "name": feed.name,
                "file": f"feeds/{file_name}",
                "sha256": sha256_of(destination),
                "bytes": destination.stat().st_size,
                "license": feed.license,
                "source": feed.entry,
            }
        )

    # `feeds/` always matches the manifest, so a feed that now fails can't linger as a stale file.
    published = {item["file"] for item in feeds}
    for path in (output / "feeds").glob("*.zip"):
        if f"feeds/{path.name}" not in published:
            path.unlink()

    manifest = {
        "transitous_commit": args.commit,
        "fetched_at": datetime.datetime.now(datetime.UTC).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "feeds": sorted(feeds, key=lambda item: item["id"]),
        "skipped": sorted_unique_skipped(skipped),
        "realtime": sorted_unique_realtime(plan.realtime),
    }
    temporary = output / ".tmp-feeds.json"
    temporary.write_text(
        json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    temporary.rename(output / "feeds.json")

    print(
        f"done in {time.monotonic() - started:.0f} s: {len(feeds)} feeds, "
        f"{len(manifest['skipped'])} skipped, {len(manifest['realtime'])} real-time URLs, "
        f"{sum(item['bytes'] for item in feeds)} bytes",
        flush=True,
    )
    return 0 if feeds else 1


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__.partition("\n")[0])
    parser.add_argument(
        "--input",
        type=Path,
        required=True,
        help="directory of Transitous region files (feeds/<region>.json)",
    )
    parser.add_argument("--output", type=Path, required=True, help="output directory")
    parser.add_argument(
        "--region",
        action="append",
        required=True,
        help="region to fetch, such as us-wa; repeatable, and deliberately without a wildcard",
    )
    parser.add_argument(
        "--cache",
        type=Path,
        help="directory for downloads and cleaned feeds that later runs reuse "
        "(default: <output>/.cache)",
    )
    parser.add_argument("--jobs", type=int, default=2, help="feeds fetched at once (default: 2)")
    parser.add_argument(
        "--timeout", type=float, default=1800, help="seconds allowed per feed (default: 1800)"
    )
    parser.add_argument(
        "--transitous",
        type=Path,
        default=Path(os.environ.get("TRANSITOUS_DIR", "/opt/transitous")),
        help="the pinned Transitous checkout",
    )
    parser.add_argument(
        "--commit",
        default=os.environ.get("TRANSITOUS_COMMIT", ""),
        help="Transitous commit to record in the manifest",
    )
    parser.add_argument(
        "--exclusions",
        type=Path,
        default=Path(__file__).resolve().parent / "excluded-sources.txt",
        help="file of Transitous source names never to fetch",
    )
    args = parser.parse_args(argv)
    if args.jobs < 1:
        parser.error("--jobs must be at least 1")
    return args


if __name__ == "__main__":
    sys.exit(run(parse_args(sys.argv[1:])))
