"""The geocoder step: the btrfs image of everything the geocoder serves, for servers.

A geocoder release is `geocoder/DATE/geocoder.btrfs`, the image `data_pipeline`'s `build`
writes (the index as `index/`, with `categories/` and `tagger.model`), and `release.json`,
which records the image's size and SHA-256 and the index's schema version, from the
`geocoder.json` written beside the image:
a geocoder only reads indexes of the schema it was built for, so a server can tell whether
it can serve a release without downloading it. Only servers read geocoder releases, so
`latest.json` doesn't name them.
"""

import json
import sys
from pathlib import Path

from .common import CACHE_RELEASE_MARKER, PublishError, publisher_commit, release_prefix
from .image import publish_image
from .release import finish_release

IMAGE_FILE = "geocoder.btrfs"


def schema_version(metadata: Path) -> int:
    try:
        value = json.loads(metadata.read_text())["schema_version"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise PublishError(f"can't read the index's schema_version from {metadata}: {error}") from error
    if not isinstance(value, int):
        raise PublishError(f"{metadata}'s schema_version is {value!r}, not a number")
    return value


def run(publisher, image: Path, metadata: Path, date: str, source: str | None, build_commit: str | None,
        part_size: int):
    prefix = release_prefix("geocoder", date)
    schema = schema_version(metadata)
    existing = publisher.get_json(prefix + "release.json")
    if existing:
        print(f"geocoder {date} is published already; checking it matches, uploading nothing", file=sys.stderr)
        publisher = publisher.checking()
    key = prefix + IMAGE_FILE
    fields, changed = publish_image(publisher, image, key, part_size, existing.get("image") if existing else None)
    release = {
        "kind": "geocoder",
        "date": date,
        "image": fields,
        "schema_version": schema,
        "source": source,
        "build_commit": build_commit,
        "publisher_commit": publisher_commit(),
    }
    finish_release(
        publisher, prefix, release, existing, {key}, publisher.remote(prefix), CACHE_RELEASE_MARKER,
        changed=changed,
    )
    return release
