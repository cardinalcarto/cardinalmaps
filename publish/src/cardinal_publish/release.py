"""Finishing a release: checking what's under its prefix, then writing `release.json`.

`release.json` is written last and marks a release complete. Once it exists the release
doesn't change: a rerun only checks that its inputs give the same objects (see
`Publisher.checking`), and a different release on the same day needs the next `.N` date.
"""

import sys

from .common import PublishError, Upload

RELEASE_FILE = "release.json"

# Fields that describe the run rather than the release, left out when comparing.
RUN_FIELDS = ("publisher_commit",)


def comparable(release):
    return {key: value for key, value in release.items() if key not in RUN_FIELDS}


def finish_release(publisher, prefix, release, existing, expected, remote, cache_control, changed):
    """Writes `release.json`, or checks the published one is the same release.

    `expected` is every key the release has besides `release.json`, `remote` what was
    under the prefix before this run, and `changed` how many objects differed from it.
    """
    if existing is not None:
        if changed or comparable(existing) != comparable(release):
            raise PublishError(
                f"{publisher.key(prefix)} is published already with other contents; "
                f"publish this as the next release of the day ({release['date']}.1, .2, ...) instead"
            )
        print(f"{publisher.key(prefix)} is published already and matches", file=sys.stderr)
        return
    extra = sorted(set(remote) - set(expected) - {prefix + RELEASE_FILE})
    if extra:
        shown = ", ".join(extra[:5]) + (", ..." if len(extra) > 5 else "")
        raise PublishError(
            f"{len(extra)} objects under {publisher.key(prefix)} aren't part of this release "
            f"(left by a run with other inputs?): {shown}. Remove them, or use another date"
        )
    upload = Upload.of_plain_json(prefix + RELEASE_FILE, release, cache_control)
    publisher.put(upload)
    print(
        f"{'would write' if publisher.dry_run else 'wrote'} {publisher.key(upload.key)}",
        file=sys.stderr,
    )
