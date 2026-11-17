# wof_service on its own: give it the database with --db or WOF_DB. .github/workflows/images.yml
# publishes it as cardinal-wof-service, and builds cardinal-wof-service-flyio on top of it
# (deploy/wof_service), which downloads a published release of the database first.
FROM rust:1.98-trixie AS build

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked

FROM debian:trixie-slim

COPY --from=build /src/target/release/wof_service /usr/local/bin/wof_service

ENV WOF_BIND=0.0.0.0:3000
EXPOSE 3000
ENTRYPOINT ["wof_service"]
