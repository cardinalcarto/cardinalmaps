# wof_service, with entrypoint.sh in front of it, which can fill an empty volume with the
# database. .github/workflows/images.yml publishes it as cardinal-wof-service.
FROM rust:1.98-trixie AS build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:trixie-slim

# entrypoint.sh downloads the database over HTTPS and may decompress it.
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates curl bzip2 \
 && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/target/release/wof_service /usr/local/bin/wof_service
COPY --chmod=755 entrypoint.sh /usr/local/bin/wof-entrypoint

ENV WOF_BIND=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["/usr/local/bin/wof-entrypoint"]
