"""Publishing a btrfs image (`data_pipeline`'s `BtrfsImage`) as one object, resumably.

Servers download an image whole and mount it read-only; its files are compressed inside it,
so the object isn't compressed again.
"""

from pathlib import Path

from .archive import publish_file
from .common import OCTET_STREAM, PublishError
from .target import Publisher

# Parts are large; a few in flight fill the uplink, as for the map.
IMAGE_JOBS = 8

# The btrfs superblock's primary copy is at 64 KiB, with its magic 64 bytes in.
SUPERBLOCK_MAGIC_OFFSET = 0x10000 + 0x40
SUPERBLOCK_MAGIC = b"_BHRfS_M"


def check_btrfs(path: Path):
    if not path.is_file():
        raise PublishError(f"no image at {path}")
    with open(path, "rb") as file:
        file.seek(SUPERBLOCK_MAGIC_OFFSET)
        magic = file.read(len(SUPERBLOCK_MAGIC))
    if magic != SUPERBLOCK_MAGIC:
        raise PublishError(f"{path} isn't a btrfs image (no superblock magic at 64 KiB)")


def publish_image(publisher, path: Path, key: str, part_size: int, published=None):
    """Uploads the image as `key`; returns `(release fields, changed)`.

    `published` is the `{"file", "bytes", "sha256"}` a complete release recorded for it, if any.
    """
    check_btrfs(path)
    if publisher.jobs > IMAGE_JOBS:
        publisher = Publisher(publisher.target, publisher.root, publisher.state, IMAGE_JOBS, publisher.dry_run)
    recorded = (published["bytes"], published["sha256"]) if published else None
    size, sha, changed = publish_file(publisher, path, key, part_size, recorded, OCTET_STREAM)
    return {"file": key.rsplit("/", 1)[-1], "bytes": size, "sha256": sha}, changed
