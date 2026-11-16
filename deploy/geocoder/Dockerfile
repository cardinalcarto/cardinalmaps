# The geocoder for fly.io: the cardinal-airmail image of the same commit, with entrypoint.sh in
# front of airmail_service, which fills the volume with the pinned release's image and mounts
# it. .github/workflows/images.yml builds it from the repository's root after cardinal-airmail,
# as cardinal-airmail-flyio; Dockerfile.dockerignore keeps the context to what's copied below.

ARG AIRMAIL_IMAGE
FROM ${AIRMAIL_IMAGE}

# jq reads release.json; curl, sha256sum and mount come with the base image.
RUN dnf install -y --setopt=install_weak_deps=False jq && dnf clean all

# The index schema this airmail reads, from the source it was built from, which entrypoint.sh
# checks each release against.
COPY airmail/airmail/src/index.rs /tmp/index.rs
RUN mkdir -p /etc/airmail \
 && sed -n 's/^pub const SCHEMA_VERSION: u32 = \([0-9]*\);$/\1/p' /tmp/index.rs > /etc/airmail/schema-version \
 && grep -qx '[0-9][0-9]*' /etc/airmail/schema-version \
 && rm /tmp/index.rs

COPY --chmod=755 deploy/geocoder/entrypoint.sh /usr/local/bin/geocoder-entrypoint

ENV AIRMAIL_BIND=0.0.0.0:3000
ENTRYPOINT ["/usr/local/bin/geocoder-entrypoint"]
