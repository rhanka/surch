# Surch — entry point for tests, benches, packaging.
# All paths are relative to the workspace root. Targets are idempotent
# and trap-clean their background processes.

SHA           := $(shell git rev-parse --short HEAD 2>/dev/null || echo dev)
REPORTS_DIR   := target/bench-reports/$(SHA)
SURCH_URL     ?= http://127.0.0.1:7700
OS_URL        ?= http://127.0.0.1:9200
SURCH_PORT    ?= 7700

# matchID-style remote knobs. Hard caps are enforced inside the scw scripts.
SCW_TYPE              ?= DEV1-M
SCW_IMAGE             ?= ubuntu_jammy
SCW_ZONE              ?= fr-par-1
SCW_MAX_COST_EUR      ?= 2
SCW_MAX_DURATION_MIN  ?= 30
SCW_TAG               := surch-bench-$(SHA)-$(shell date +%s)

# ---------------------------------------------------------------------------
# Phony index
# ---------------------------------------------------------------------------
.PHONY: help test build release \
        surch-build surch-up surch-down \
        opensearch-up opensearch-down \
        bench-smoke bench-local bench-recall bench-stress bench-perf \
        bench-remote-scw bench-all report \
        clean

help:
	@echo "Surch targets:"
	@echo "  test              cargo test --workspace --locked (~30 s)"
	@echo "  bench-smoke       BAN tiny smoke against a local engine"
	@echo "  bench-local       BAN 25k + INSEE 25k vs Surch & OS (~5 min)"
	@echo "  bench-recall      SciFact NDCG@10 vs Surch & OS (~3 min)"
	@echo "  bench-stress      artillery-replay vs Surch & OS (~10 min)"
	@echo "  bench-perf        bench-local + bench-stress + RSS sampling"
	@echo "  bench-remote-scw  bench-perf on a Scaleway DEV1-M (hard 30 min cap)"
	@echo "  bench-all         full local suite, sequenced"
	@echo "  surch-up          launch surch-api release in the background"
	@echo "  surch-down        stop a backgrounded surch-api"
	@echo "  opensearch-up     start the dedicated OpenSearch docker"
	@echo "  opensearch-down   stop and remove the OpenSearch docker"
	@echo "  release           cargo build --release --workspace"
	@echo "  docker-build      build the multi-stage Docker image locally"
	@echo "  docker-smoke      build the image, start it on port 7711, hit /"
	@echo "  report            aggregate target/bench-reports/<sha>/*.json -> summary.md"

# ---------------------------------------------------------------------------
# Build + tests
# ---------------------------------------------------------------------------
test:
	cargo test --workspace --locked

build:
	cargo build --workspace --locked

release:
	cargo build --release --workspace --locked

surch-build:
	cargo build --release -p surch-api --locked

# ---------------------------------------------------------------------------
# Engine lifecycle
# ---------------------------------------------------------------------------
surch-up: surch-build
	@if pgrep -f 'target/release/surch-api$$' >/dev/null 2>&1; then \
	  echo "surch-api already running"; \
	else \
	  RUST_LOG=warn nohup target/release/surch-api >/dev/null 2>&1 & \
	  disown || true; \
	fi
	@until curl -fsS --max-time 1 $(SURCH_URL)/ >/dev/null 2>&1; do sleep 0.2; done
	@echo "surch-api ready at $(SURCH_URL)"

surch-down:
	-@kill -INT $$(pgrep -f 'target/release/surch-api$$' 2>/dev/null) 2>/dev/null || true
	@sleep 1

opensearch-up:
	scripts/bench/opensearch-start.sh
	scripts/bench/opensearch-wait.sh
	@echo "opensearch ready at $(OS_URL)"

opensearch-down:
	-@scripts/bench/opensearch-stop.sh || true

$(REPORTS_DIR):
	@mkdir -p $(REPORTS_DIR)

# ---------------------------------------------------------------------------
# Benchmarks
# ---------------------------------------------------------------------------
bench-smoke: surch-up | $(REPORTS_DIR)
	# Sanity: 5-iteration Rue Payenne + Place Patrice Chereau on BAN 25k.
	bash scripts/bench/bench.sh "smoke-$(SHA)" $(REPORTS_DIR)/smoke-surch.out $(SURCH_URL)
	$(MAKE) surch-down

bench-local: opensearch-up surch-up | $(REPORTS_DIR)
	bash scripts/bench/bench.sh        "ban25k-surch-$(SHA)" $(REPORTS_DIR)/ban25k-surch.out $(SURCH_URL)
	bash scripts/bench/insee-bench.sh  "insee25k-surch-$(SHA)" $(REPORTS_DIR)/insee25k-surch.out $(SURCH_URL) 30
	$(MAKE) surch-down
	bash scripts/bench/bench.sh        "ban25k-os-$(SHA)" $(REPORTS_DIR)/ban25k-os.out $(OS_URL)
	bash scripts/bench/insee-bench.sh  "insee25k-os-$(SHA)" $(REPORTS_DIR)/insee25k-os.out $(OS_URL) 30
	$(MAKE) opensearch-down
	@echo "bench-local reports under $(REPORTS_DIR)"

bench-recall: opensearch-up surch-up | $(REPORTS_DIR)
	bash scripts/bench/scifact-ndcg.sh "scifact-surch-$(SHA)" $(REPORTS_DIR)/scifact-surch.out $(SURCH_URL)
	bash scripts/bench/scifact-ndcg.sh "scifact-os-$(SHA)"    $(REPORTS_DIR)/scifact-os.out    $(OS_URL)
	$(MAKE) surch-down opensearch-down

bench-stress: opensearch-up surch-up | $(REPORTS_DIR)
	bash scripts/bench/artillery-replay.sh "art-surch-$(SHA)" $(REPORTS_DIR)/art-surch.out $(SURCH_URL)
	$(MAKE) surch-down
	bash scripts/bench/artillery-replay.sh "art-os-$(SHA)"    $(REPORTS_DIR)/art-os.out    $(OS_URL)
	$(MAKE) opensearch-down

bench-perf: bench-local bench-stress
	@echo "bench-perf reports under $(REPORTS_DIR)"

bench-remote-scw:
	@echo "scw harness not implemented yet — see docs/ops/test-automation-plan.md"
	@exit 1

bench-all: bench-local bench-recall bench-stress

report:
	@ls -1 $(REPORTS_DIR)/*.out 2>/dev/null || (echo "no reports for $(SHA)"; exit 1)
	@echo "summary aggregation tool (bench_report) not implemented yet"

# ---------------------------------------------------------------------------
# Docker
# ---------------------------------------------------------------------------
DOCKER_IMAGE ?= ghcr.io/rhanka/surch
DOCKER_TAG   ?= dev-$(SHA)
DOCKER_CONTAINER ?= surch-bench-image-smoke

.PHONY: docker-build docker-smoke
docker-build:
	docker build -t $(DOCKER_IMAGE):$(DOCKER_TAG) .

docker-smoke: docker-build
	-@docker rm -f $(DOCKER_CONTAINER) >/dev/null 2>&1
	docker run -d --name $(DOCKER_CONTAINER) -p 7711:7700 $(DOCKER_IMAGE):$(DOCKER_TAG)
	@until curl -fsS --max-time 1 http://127.0.0.1:7711/ >/dev/null 2>&1; do sleep 0.3; done
	@echo "container reports:" && curl -s http://127.0.0.1:7711/ | head -c 400 && echo
	-@docker rm -f $(DOCKER_CONTAINER) >/dev/null 2>&1

# ---------------------------------------------------------------------------
# Clean
# ---------------------------------------------------------------------------
clean: surch-down opensearch-down
	cargo clean
