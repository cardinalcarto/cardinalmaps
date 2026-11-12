"""cardinal-publish: the command line. See publish/README.md."""

import argparse
import hashlib
import os
import sys
from pathlib import Path

from . import latest, mapfile, poi, routing, transit
from .common import PublishError, check_date
from .target import DirTarget, Publisher, State, parse_target

DEFAULT_ENDPOINT = "https://t3.storage.dev"


def state_home():
    base = os.environ.get("XDG_STATE_HOME") or str(Path.home() / ".local" / "state")
    return Path(base) / "cardinal-publish"


def cache_home():
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "cardinal-publish"


def add_target_args(parser, required=True):
    parser.add_argument("--target", required=required,
                        help="s3://BUCKET, or dir:/PATH to write the bucket's layout to a local directory")
    parser.add_argument("--prefix", default="releases", help="key prefix in the bucket (default: releases)")
    parser.add_argument("--endpoint", default=DEFAULT_ENDPOINT, help=f"S3 endpoint (default: {DEFAULT_ENDPOINT}, Tigris)")
    parser.add_argument("--region", default="auto", help="S3 region (default: auto)")
    parser.add_argument("--access-key-id", help="S3 access key ID")
    parser.add_argument("--secret-access-key", help="S3 secret access key")
    parser.add_argument("--jobs", type=int, default=32, help="concurrent uploads (default: 32)")
    parser.add_argument("--state", type=Path,
                        help="where to remember what's published (default: a file per target under "
                             "$XDG_STATE_HOME/cardinal-publish)")
    parser.add_argument("--dry-run", action="store_true",
                        help="read and check everything, list what's published, upload nothing")


def parser():
    top = argparse.ArgumentParser(prog="cardinal-publish", description=__doc__)
    steps = top.add_subparsers(dest="step", required=True)

    step = steps.add_parser("routing", help="publish Valhalla tiles from a build_extract tarball, loose")
    add_target_args(step)
    step.add_argument("--tar", type=Path, required=True, help="the tiles.tar of a routing build")
    step.add_argument("--date", required=True, help="the release date, YYYY-MM-DD[.N]")
    step.add_argument("--source", help="what the tiles were built from, for release.json (e.g. planet-261005)")
    step.add_argument("--build-commit", help="the Cardinal commit the tiles were built with, for release.json")

    step = steps.add_parser("map-convert", help="convert the MBTiles tileset to PMTiles (local only)")
    step.add_argument("--mbtiles", type=Path, required=True)
    step.add_argument("--out", type=Path, required=True)
    step.add_argument("--tmpdir", type=Path, required=True,
                      help="for go-pmtiles' temporary files, about the input's size; use another disk than --out")
    step.add_argument("--pmtiles-bin", type=Path,
                      help=f"a go-pmtiles binary (default: the pinned {mapfile.PMTILES_VERSION}, downloaded and checked)")
    step.add_argument("--mlt-header", action="store_true",
                      help="for an MLT tileset, set the tile type and compression go-pmtiles leaves unknown")

    step = steps.add_parser("map", help="publish a PMTiles map as one resumable multipart upload")
    add_target_args(step)
    step.add_argument("--pmtiles", type=Path, required=True)
    step.add_argument("--date", required=True, help="the release date, YYYY-MM-DD[.N]")
    step.add_argument("--source", help="what the tiles were built from, for release.json")
    step.add_argument("--build-commit", help="the Cardinal commit the tiles were built with, for release.json")
    step.add_argument("--part-size-mib", type=int, default=128, help="multipart part size (default: 128 MiB)")

    step = steps.add_parser("transit", help="publish a transit build and its feed index")
    add_target_args(step)
    step.add_argument("--build", type=Path, required=True, help="the transit build directory")
    step.add_argument("--staging", type=Path, required=True,
                      help="valinor-transit-build feed-index's output for that build")
    step.add_argument("--date", required=True, help="the release date, YYYY-MM-DD[.N]")
    step.add_argument("--routing-date", required=True,
                      help="the routing release the build's walks were computed on (its build ID must match)")
    step.add_argument("--build-commit", help="the Cardinal commit the build was made with, for release.json")
    step.add_argument("--work", type=Path, required=True,
                      help="where to pack the transfer tiles' archives (about their size); a rerun reuses them")
    step.add_argument("--pack-bin", type=Path,
                      help="a cardinal-pack binary (default: crates/target/release/cardinal-pack in this checkout)")
    step.add_argument("--part-size-mib", type=int, default=128, help="multipart part size (default: 128 MiB)")

    step = steps.add_parser("poi", help="publish the POI identity releases `poi_service export` staged")
    add_target_args(step)
    step.add_argument("--staging", type=Path, required=True, help="poi_service export's --staging directory")
    step.add_argument("--latest", action="store_true",
                      help="then point latest.json's poi at the newest checkpoint published")
    step.add_argument("--part-size-mib", type=int, default=128, help="multipart part size (default: 128 MiB)")

    step = steps.add_parser("poi-prune", help="delete old POI identity releases (never during an upload)")
    add_target_args(step, required=False)
    step.add_argument("--staging", type=Path,
                      help="prune this local staging directory instead of a --target")
    step.add_argument("--keep-days", type=int, default=28,
                      help="keep checkpoints this many days older than the newest, and the snapshots "
                           "they restore from (default: 28)")

    step = steps.add_parser("latest", help="point latest.json at releases, checking they fit together")
    add_target_args(step)
    for kind in ("map", "routing", "transit"):
        step.add_argument(f"--{kind}", help=f"the {kind} release (default: keep the current one)")
    step.add_argument("--poi", help="the POI identity checkpoint, a replication sequence (default: keep the current one)")
    return top


def run(args):
    if args.step == "map-convert":
        def binary():
            return args.pmtiles_bin or mapfile.pmtiles_binary(cache_home())

        mapfile.convert(args.mbtiles.resolve(), args.out.resolve(), args.tmpdir.resolve(), binary, args.mlt_header)
        return

    if args.jobs < 1:
        raise PublishError("--jobs must be at least 1")
    if args.step == "poi-prune" and args.staging:
        if args.target:
            raise PublishError("poi-prune takes --target or --staging, not both")
        publisher = Publisher(DirTarget(args.staging.resolve()), "", None, args.jobs, args.dry_run)
        poi.prune(publisher, args.keep_days, base="")
        return
    if args.target is None:
        raise PublishError("--target is required")
    target = parse_target(args.target, args, args.jobs)
    state_path = args.state or state_home() / (
        hashlib.sha256(f"{target.describe()}/{args.prefix}".encode()).hexdigest()[:16] + ".sqlite"
    )
    state = State(state_path)
    try:
        jobs = args.jobs
        if args.step == "map":
            # Parts are large; a few in flight fill the uplink.
            jobs = min(jobs, 8)
        publisher = Publisher(target, args.prefix, state, jobs, args.dry_run)
        print(f"publishing to {target.describe()} under {args.prefix!r}"
              f"{' (dry run)' if args.dry_run else ''}", file=sys.stderr)
        if args.step == "routing":
            routing.run(publisher, args.tar.resolve(), check_date(args.date), args.source, args.build_commit)
        elif args.step == "map":
            mapfile.publish(publisher, args.pmtiles.resolve(), check_date(args.date), args.source,
                            args.build_commit, args.part_size_mib << 20)
        elif args.step == "transit":
            transit.run(publisher, args.build.resolve(), args.staging.resolve(), check_date(args.date),
                        args.routing_date, args.build_commit, args.work.resolve(),
                        args.pack_bin.resolve() if args.pack_bin else None, args.part_size_mib << 20)
        elif args.step == "poi":
            poi.run(publisher, args.staging.resolve(), args.latest, args.part_size_mib << 20)
        elif args.step == "poi-prune":
            poi.prune(publisher, args.keep_days)
        elif args.step == "latest":
            latest.run(publisher, {"map": args.map, "routing": args.routing, "transit": args.transit, "poi": args.poi})
    finally:
        state.close()


def main(argv=None):
    args = parser().parse_args(argv)
    try:
        run(args)
    except PublishError as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
