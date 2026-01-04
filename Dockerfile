FROM rustlang/rust:nightly AS builder
RUN update-ca-certificates
WORKDIR /usr/src/

RUN USER=root cargo new as207960-esign
WORKDIR /usr/src/woistdasq

COPY src ./src
COPY Cargo.toml Cargo.lock ./
RUN cargo install --path .

FROM debian:buster-slim

RUN apt-get update && apt-get install -y ca-certificates && apt-get clean && rm -rf /var/lib/apt/lists/*
RUN update-ca-certificates

WORKDIR /woistdasq

COPY --from=builder --chown=0:0 /usr/local/cargo/bin/woistdasq /woistdasq/woistdasq

ENTRYPOINT ["/woistdasq/woistdasq"]
