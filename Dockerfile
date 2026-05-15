# Surch — multi-stage Dockerfile.
# Stage 1 (builder): pin a slim Rust toolchain and produce a release binary.
# Stage 2 (runtime): distroless cc-debian12 so the final image stays under
# ~30 MB compressed and exposes a non-root user out of the box.
#
# Build:
#   docker build -t ghcr.io/rhanka/surch:dev .
# Run:
#   docker run --rm -p 7700:7700 ghcr.io/rhanka/surch:dev

ARG RUST_VERSION=1.83

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

# Build the API binary in release mode; LTO + strip are already configured
# in the workspace Cargo.toml.
RUN cargo build --release --locked -p surch-api

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
