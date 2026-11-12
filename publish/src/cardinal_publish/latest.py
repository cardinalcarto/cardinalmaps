"""The latest step: pointing `latest.json` at a release of each kind.

The app downloads one release of each kind, so `latest.json` must never pair a transit
release with routing tiles other than the ones its walks were computed on.

`poi` names a POI identity checkpoint by its replication sequence. Identifiers are stable, so
it pairs with any map or geocoder release and isn't checked against them.
"""

import sys

from .common import CACHE_LATEST, KINDS, PublishError, Upload, check_date

LATEST = "latest.json"


def check_sequence(sequence) -> int:
    try:
        value = int(sequence)
    except (TypeError, ValueError):
        value = -1
    if value < 0 or str(value) != str(sequence).strip():
        raise PublishError(f"POI checkpoint {sequence!r} isn't a replication sequence number")
    return value


def run(publisher, wanted):
    current = publisher.get_json(LATEST) or {}
    latest = {kind: current.get(kind) for kind in (*KINDS, "poi")}
    for kind, date in wanted.items():
        if date is not None:
            latest[kind] = check_sequence(date) if kind == "poi" else check_date(date)
    releases = {}
    for kind, date in latest.items():
        if date is None:
            continue
        release = publisher.get_json(f"{kind}/{date}/release.json")
        if release is None:
            raise PublishError(f"{kind} release {date} isn't published (no {kind}/{date}/release.json)")
        releases[kind] = release
    transit, routing = releases.get("transit"), releases.get("routing")
    if transit and not routing:
        raise PublishError("latest.json can't name a transit release without a routing release")
    if transit and transit["valhalla_build_id"] != routing["valhalla_build_id"]:
        raise PublishError(
            f"transit {latest['transit']} was built on Valhalla build {transit['valhalla_build_id']} "
            f"(routing release {transit.get('routing_release')}), but routing {latest['routing']} is build "
            f"{routing['valhalla_build_id']}; publish transit on that routing release, or point both at a matching pair"
        )
    document = {kind: date for kind, date in latest.items() if date is not None}
    if document == {kind: current.get(kind) for kind in document} and set(current) <= set(document):
        print(f"{publisher.key(LATEST)} already says {document}", file=sys.stderr)
        return document
    publisher.put(Upload.of_plain_json(LATEST, document, CACHE_LATEST))
    print(f"{'would write' if publisher.dry_run else 'wrote'} {publisher.key(LATEST)}: {document}", file=sys.stderr)
    return document
