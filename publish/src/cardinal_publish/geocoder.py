"""The geocoder step: the btrfs image of everything the geocoder serves, for servers, and
the models a phone's offline search downloads.

A geocoder release is `geocoder/DATE/`, holding, for one `data_pipeline` `build`:

- `geocoder.btrfs`, the image `build` writes (the index as `index/`, with `categories/` and
  `tagger.model`), for servers. `release.json` records its size and SHA-256 and the index's
  schema version, from the `geocoder.json` written beside the image: a geocoder only reads
  indexes of the schema it was built for, so a server can tell whether it can serve a
  release without downloading it.
- `tagger.model` and `categorical/LANG.amcats`, the query tagger and each language's
  categorical model `build` was given (the ones in the image), loose, for phones. A phone reads each one's size and SHA-256 from a
  `HEAD` (`Content-Length` and `x-amz-meta-sha256`), so they go up as single objects: a
  multipart upload doesn't carry the SHA-256 header. `release.json` lists them too.

Either part may be left out, but a phone only uses a release with a tagger, so `latest`
won't name one without it as `latest.json`'s `geocoder`.
"""

import json
import re
import sys
from pathlib import Path

from .common import (
    CACHE_RELEASE_MARKER, OCTET_STREAM, PublishError, Upload, publisher_commit, release_prefix, sha256_file,
)
from .image import publish_image
from .release import finish_release

IMAGE_FILE = "geocoder.btrfs"
TAGGER_FILE = "tagger.model"
CATEGORICAL_DIR = "categorical"
# A categorical model is named by its language's ISO 639-3 code, as `categorical-models` writes it.
MODEL_NAME = re.compile(r"^([a-z]{3})\.amcats$")


def schema_version(metadata: Path) -> int:
    try:
        value = json.loads(metadata.read_text())["schema_version"]
    except (OSError, ValueError, KeyError, TypeError) as error:
        raise PublishError(f"can't read the index's schema_version from {metadata}: {error}") from error
    if not isinstance(value, int):
        raise PublishError(f"{metadata}'s schema_version is {value!r}, not a number")
    return value


def model_files(tagger: Path, categories: Path | None):
    """`(key under the release, local path)` for each phone model, the tagger first."""
    if not tagger.is_file():
        raise PublishError(f"no tagger at {tagger}")
    files = [(TAGGER_FILE, tagger)]
    if categories is None:
        return files
    if not categories.is_dir():
        raise PublishError(f"no categorical models directory at {categories}")
    models = []
    for path in sorted(categories.iterdir()):
        if not MODEL_NAME.match(path.name) or not path.is_file():
            raise PublishError(
                f"{path} isn't a categorical model: --categories holds only LANG.amcats files, "
                "LANG an ISO 639-3 code"
            )
        models.append((f"{CATEGORICAL_DIR}/{path.name}", path))
    if not models:
        raise PublishError(f"{categories} holds no categorical models")
    return files + models


def run(publisher, image: Path | None, metadata: Path | None, date: str, source: str | None,
        build_commit: str | None, part_size: int, tagger: Path | None = None, categories: Path | None = None):
    prefix = release_prefix("geocoder", date)
    if (image is None) != (metadata is None):
        raise PublishError("--image and --metadata go together")
    if categories is not None and tagger is None:
        raise PublishError("--categories needs --tagger: a phone only uses categorical models with the tagger")
    if image is None and tagger is None:
        raise PublishError("a geocoder release needs --image and --metadata, or --tagger, or both")
    schema = schema_version(metadata) if metadata is not None else None
    models = model_files(tagger, categories) if tagger is not None else []

    existing = publisher.get_json(prefix + "release.json")
    if existing:
        print(f"geocoder {date} is published already; checking it matches, uploading nothing", file=sys.stderr)
        publisher = publisher.checking()
        if existing.get("image") and image is None:
            raise PublishError(f"geocoder {date} is published with an image; pass --image and --metadata to check it")
        if existing.get("tagger") and tagger is None:
            raise PublishError(f"geocoder {date} is published with models; pass --tagger and --categories to check them")
    remote = publisher.remote(prefix)

    release = {
        "kind": "geocoder",
        "date": date,
        "source": source,
        "build_commit": build_commit,
        "publisher_commit": publisher_commit(),
    }
    expected = set()
    changed = 0

    if models:
        uploads = []
        for name, path in models:
            print(f"hashing {path}", file=sys.stderr)
            uploads.append(Upload(
                key=prefix + name, body=None, path=path, size=path.stat().st_size, sha256=sha256_file(path),
                content_type=OCTET_STREAM,
            ))
        # Single PUTs (each read whole on a worker), so every model carries x-amz-meta-sha256.
        progress = publisher.sync(iter(uploads), remote, f"geocoder {date} models", len(uploads),
                                  sum(upload.size for upload in uploads))
        changed += progress.uploaded
        expected |= {upload.key for upload in uploads}
        fields = [{"file": name, "bytes": upload.size, "sha256": upload.sha256}
                  for (name, _), upload in zip(models, uploads)]
        release["tagger"] = fields[0]
        release["categorical"] = {
            MODEL_NAME.match(entry["file"].rsplit("/", 1)[-1])[1]: entry for entry in fields[1:]
        }

    if image is not None:
        key = prefix + IMAGE_FILE
        image_fields, image_changed = publish_image(
            publisher, image, key, part_size, existing.get("image") if existing else None
        )
        changed += image_changed
        expected.add(key)
        release["image"] = image_fields
        release["schema_version"] = schema

    finish_release(
        publisher, prefix, release, existing, expected, remote, CACHE_RELEASE_MARKER, changed=changed,
    )
    return release
