FROM rust:1.98 AS build

RUN apt update && apt install -y clang pkg-config

WORKDIR /usr/src/airmail
COPY ./airmail ./airmail
COPY ./airmail_indexer ./airmail_indexer
COPY ./airmail_service ./airmail_service
COPY ./Cargo.toml ./Cargo.toml
COPY ./Cargo.lock ./Cargo.lock

RUN cargo install --locked --path ./airmail_service

WORKDIR /app

RUN rm -rf /usr/src/airmail

FROM fedora:43

COPY --from=build /usr/local/cargo/bin/airmail_service /usr/local/bin/airmail_service

EXPOSE 3000

ENTRYPOINT ["airmail_service"]
