# syntax=docker/dockerfile:1

ARG RUST_VERSION=1.85.0

FROM rust:${RUST_VERSION}-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        pkg-config \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY Cargo.toml Cargo.lock ./
COPY surch-core/Cargo.toml surch-core/Cargo.toml
COPY surch-api/Cargo.toml surch-api/Cargo.toml

RUN mkdir -p surch-core/src surch-api/src \
    && printf '%s\n' 'pub fn _placeholder() {}' > surch-core/src/lib.rs \
    && printf '%s\n' 'fn main() {}' > surch-api/src/main.rs \
    && cargo build -p surch-api --release --locked \
    && rm -rf surch-core/src surch-api/src

COPY surch-core/src surch-core/src
COPY surch-api/src surch-api/src

RUN cargo build -p surch-api --release --locked

FROM builder AS test
RUN cargo test --locked

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libssl3 \
    && rm -rf /var/lib/apt/lists/* \
    && useradd -m -u 10001 -s /usr/sbin/nologin surch

WORKDIR /app

COPY --from=builder /app/target/release/surch /usr/local/bin/surch

RUN mkdir -p /app/data \
    && chown -R surch:surch /app

USER surch

ENV RUST_LOG=info
EXPOSE 9200
VOLUME ["/app/data"]

ENTRYPOINT ["/usr/local/bin/surch"]
