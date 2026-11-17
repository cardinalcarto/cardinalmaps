FROM rust:1.98-trixie AS build

# poi_service shares airmail's importance code (../airmail/airmail_importance),
# given as the build context of that name:
#   docker build --build-context airmail_importance=../airmail/airmail_importance .
WORKDIR /src/poi_service
COPY --from=airmail_importance . /src/airmail/airmail_importance
COPY Cargo.toml Cargo.lock ./
COPY migrations ./migrations
COPY src ./src
RUN cargo build --release --locked

FROM debian:trixie-slim

# Replication diffs are fetched over HTTPS, verified against the system roots.
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=build /src/poi_service/target/release/poi_service /usr/local/bin/poi_service

USER 65534:65534
ENTRYPOINT ["poi_service"]
