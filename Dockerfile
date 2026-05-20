# Surch — multi-stage Dockerfile.
# Stage 1 (builder): pin a slim Rust toolchain and produce release binaries.
# Stage 2 (bench-driver): shell-capable CI/K8s benchmark tools.
# Stage 3 (runtime): distroless cc-debian12 so the final image stays under
# ~30 MB compressed and exposes a non-root user out of the box.
#
# Build:
#   docker build -t ghcr.io/rhanka/surch:dev .
# Run:
#   docker run --rm -p 7700:7700 ghcr.io/rhanka/surch:dev

# Keep this at or above the Cargo.lock MSRV floor. The AWS SDK family
# currently requires rustc 1.91.1.
ARG RUST_VERSION=1.91.1

FROM rust:${RUST_VERSION}-slim-bookworm AS builder
WORKDIR /usr/src/surch

# Install minimal build deps. `pkg-config` and `libssl-dev` are needed by
# transitive crates that may link against system OpenSSL during dependency
# resolution; if Surch eventually switches every dep to rustls we can
# drop these.
RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      pkg-config \
      libssl-dev \
      ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Copy the workspace manifests first so dependency resolution is cached.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY tests ./tests

# Build release binaries; LTO + strip are already configured in the
# workspace Cargo.toml. The distroless runtime only copies `surch-api`,
# while the K8s benchmark driver copies the reporting binaries below.
RUN cargo build --release --locked -p surch-api -p surch-demo --bins

# Bench driver stage: used only by K8s Jobs that need `/bin/sh`, curl,
# wget, jq, awk, tar, and the benchmark/reporting tools. Keeping it
# separate preserves the small distroless runtime image for the actual
# API server.
FROM debian:bookworm-slim AS bench-driver

RUN apt-get update \
 && apt-get install -y --no-install-recommends \
      bash \
      ca-certificates \
      curl \
      gawk \
      jq \
      libssl3 \
      tar \
      wget \
 && rm -rf /var/lib/apt/lists/*

COPY --from=builder /usr/src/surch/target/release/artillery_bench /usr/local/bin/artillery_bench
COPY --from=builder /usr/src/surch/target/release/bench_report /usr/local/bin/bench_report
COPY --from=builder /usr/src/surch/target/release/b1_oracle /usr/local/bin/b1_oracle
COPY scripts/bench/scifact-ndcg.sh /usr/local/bin/scifact-ndcg.sh
COPY scripts/bench/trec-covid-ndcg.sh /usr/local/bin/trec-covid-ndcg.sh

# INSEE 10k fixture (mapping + gzipped NDJSON bulk payload). Shipped
# inside the bench-driver image so insee-bench K8s Jobs can bootstrap
# the `deces` index on both engines before driving artillery_bench.
COPY tests/matchid_compat/deces/mapping.json /usr/local/share/deces/mapping.json
COPY tests/matchid_compat/deces/slice-10000.ndjson.gz /usr/local/share/deces/slice-10000.ndjson.gz
# matchID v1 replay fixture used by the Elasticsearch 8.6.1 b1_oracle
# gate.
COPY tests/matchid_compat/replays/deces_v1.json /usr/local/share/deces/replays/deces_v1.json

RUN chmod 0755 \
      /usr/local/bin/artillery_bench \
      /usr/local/bin/bench_report \
      /usr/local/bin/b1_oracle \
      /usr/local/bin/scifact-ndcg.sh \
      /usr/local/bin/trec-covid-ndcg.sh \
 && chmod 0644 \
      /usr/local/share/deces/mapping.json \
      /usr/local/share/deces/slice-10000.ndjson.gz \
      /usr/local/share/deces/replays/deces_v1.json

USER 65532:65532
WORKDIR /work

# Runtime stage: distroless gives us /etc/ssl/certs, /etc/passwd, and a
# `nonroot` user (uid 65532). No shell, no package manager, no surface.
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

# OCI image labels per https://github.com/opencontainers/image-spec/blob/main/annotations.md
LABEL org.opencontainers.image.title="surch" \
      org.opencontainers.image.description="OpenSearch-compatible Rust search engine" \
      org.opencontainers.image.source="https://github.com/rhanka/surch" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.documentation="https://github.com/rhanka/surch/blob/main/README.md"

COPY --from=builder /usr/src/surch/target/release/surch-api /usr/local/bin/surch-api

USER nonroot:nonroot
EXPOSE 7700
ENV SURCH_HOST=0.0.0.0 SURCH_PORT=7700 RUST_LOG=warn

ENTRYPOINT ["/usr/local/bin/surch-api"]
