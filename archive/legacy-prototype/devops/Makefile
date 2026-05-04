IMAGE ?= surch:dev
RUST_VERSION ?= 1.85.0
DOCKER ?= docker

BUILDER_IMAGE ?= $(IMAGE)-builder

CARGO_REGISTRY_VOL ?= surch-cargo-registry
CARGO_GIT_VOL ?= surch-cargo-git
CARGO_TARGET_VOL ?= surch-target

.PHONY: help build test run docker-builder docker-build docker-test docker-run docker-sh docker-fmt docker-clippy docker-clean host-test host-build host-run dev dev-watch

help:
	@printf '%s\n' \
		'make build          Build runtime image' \
		'make test           Run cargo test in Docker' \
		'make run            Run API on localhost:9200 (mount ./data)' \
		'make dev            Run API on host (debug, RUST_LOG=debug)' \
		'make dev-watch      Auto-reload on changes (requires cargo-watch)' \
		'make docker-sh      Shell in builder container' \
		'make docker-fmt     Run cargo fmt in Docker' \
		'make docker-clippy  Run cargo clippy in Docker' \
		'make host-build     cargo build -p surch-api' \
		'make host-run       Run API on host (release)' \
		'make host-test      Run cargo test on host'

build: docker-build

test: docker-test

run: docker-run

docker-builder:
	$(DOCKER) build --build-arg RUST_VERSION=$(RUST_VERSION) --target builder -t $(BUILDER_IMAGE) .

docker-build:
	$(DOCKER) build --build-arg RUST_VERSION=$(RUST_VERSION) -t $(IMAGE) .

docker-test:
	$(DOCKER) build --build-arg RUST_VERSION=$(RUST_VERSION) --target test .

docker-run: docker-build
	mkdir -p ./data
	$(DOCKER) run --rm -p 9200:9200 -v $(PWD)/data:/app/data $(IMAGE)

docker-sh: docker-builder
	$(DOCKER) run --rm -it \
		-v $(PWD):/app -w /app \
		-v $(CARGO_REGISTRY_VOL):/usr/local/cargo/registry \
		-v $(CARGO_GIT_VOL):/usr/local/cargo/git \
		-v $(CARGO_TARGET_VOL):/app/target \
		--entrypoint bash \
		$(BUILDER_IMAGE)

docker-fmt: docker-builder
	$(DOCKER) run --rm -v $(PWD):/app -w /app $(BUILDER_IMAGE) cargo fmt --all

docker-clippy: docker-builder
	$(DOCKER) run --rm \
		-v $(PWD):/app -w /app \
		-v $(CARGO_REGISTRY_VOL):/usr/local/cargo/registry \
		-v $(CARGO_GIT_VOL):/usr/local/cargo/git \
		-v $(CARGO_TARGET_VOL):/app/target \
		$(BUILDER_IMAGE) cargo clippy --all-targets --all-features -- -D warnings

docker-clean:
	$(DOCKER) rmi -f $(IMAGE) $(BUILDER_IMAGE) 2>/dev/null || true
	$(DOCKER) volume rm -f $(CARGO_REGISTRY_VOL) $(CARGO_GIT_VOL) $(CARGO_TARGET_VOL) 2>/dev/null || true

host-test:
	cargo test

host-build:
	cargo build -p surch-api

host-run:
	mkdir -p ./data
	cargo run -p surch-api --release

dev:
	mkdir -p ./data
	RUST_LOG=debug cargo run -p surch-api

dev-watch:
	mkdir -p ./data
	@command -v cargo-watch >/dev/null 2>&1 && RUST_LOG=debug cargo watch -x 'run -p surch-api' || (echo 'cargo-watch not found; install with: cargo install cargo-watch' && exit 1)
