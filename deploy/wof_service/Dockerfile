# wof_service for fly.io: the cardinal-wof-service image of the same commit, with entrypoint.sh
# in front of wof_service, which fills the volume with the pinned release's database.
# .github/workflows/images.yml builds it from the repository's root after cardinal-wof-service,
# as cardinal-wof-service-flyio; Dockerfile.dockerignore keeps the context to what's copied below.

ARG WOF_IMAGE
FROM ${WOF_IMAGE}

# curl downloads release.json and the database, jq reads release.json; sha256sum, df and find
# come with the base image.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl jq \
 && rm -rf /var/lib/apt/lists/*

COPY --chmod=755 deploy/wof_service/entrypoint.sh /usr/local/bin/wof-entrypoint

ENV WOF_BIND=0.0.0.0:3000
ENTRYPOINT ["/usr/local/bin/wof-entrypoint"]
