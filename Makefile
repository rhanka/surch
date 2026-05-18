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
        bench-smoke bench-local bench-recall bench-trec-covid \
        bench-stress bench-artillery-rs bench-perf \
        bench-remote-scw bench-k8s bench-all report \
        sbom \
        clean

help:
	@echo "Surch targets:"
	@echo "  test              cargo test --workspace --locked (~30 s)"
	@echo "  bench-smoke       BAN tiny smoke against a local engine"
	@echo "  bench-local       BAN 25k + INSEE 25k vs Surch & OS (~5 min)"
	@echo "  bench-recall      SciFact + TREC-COVID NDCG@10 vs Surch & OS (~10 min)"
	@echo "  bench-trec-covid  TREC-COVID NDCG@10 + Recall@10 vs Surch & OS (~7 min)"
	@echo "  bench-pair-<wl>   run a single workload vs Surch then OS (wl: ban25k|insee25k|insee25k-multi|scifact|trec-covid)"
	@echo "  bench-stress      artillery-replay (bash fallback) vs Surch & OS (~10 min)"
	@echo "  bench-artillery-rs Rust keep-alive artillery_bench vs Surch & OS (~6 min)"
	@echo "  bench-perf        bench-local + bench-stress + RSS sampling"
	@echo "  bench-remote-scw  bench-perf on a Scaleway DEV1-M (hard 30 min cap)"
	@echo "  bench-k8s         dispatch the .github/workflows/ci-k8s.yml burst-pool bench (gh CLI)"
	@echo "  bench-all         full local suite, sequenced"
	@echo "  surch-up          launch surch-api release in the background"
	@echo "  surch-down        stop a backgrounded surch-api"
	@echo "  opensearch-up     start the dedicated OpenSearch docker"
	@echo "  opensearch-down   stop and remove the OpenSearch docker"
	@echo "  release           cargo build --release --workspace"
	@echo "  docker-build      build the multi-stage Docker image locally"
	@echo "  docker-smoke      build the image, start it on port 7711, hit /"
	@echo "  sbom              generate CycloneDX SBOM (bom.json) via cargo-cyclonedx"
	@echo "  report            aggregate target/bench-reports/<sha>/*.json -> summary.md + SLO gate"

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
	bash scripts/bench/scifact-ndcg.sh    "scifact-surch-$(SHA)"    $(REPORTS_DIR)/scifact-surch.out    $(SURCH_URL)
	bash scripts/bench/scifact-ndcg.sh    "scifact-os-$(SHA)"       $(REPORTS_DIR)/scifact-os.out       $(OS_URL)
	bash scripts/bench/trec-covid-ndcg.sh "trec-covid-surch-$(SHA)" $(REPORTS_DIR)/trec-covid-surch.out $(SURCH_URL)
	bash scripts/bench/trec-covid-ndcg.sh "trec-covid-os-$(SHA)"    $(REPORTS_DIR)/trec-covid-os.out    $(OS_URL)
	$(MAKE) surch-down opensearch-down

bench-trec-covid: opensearch-up surch-up | $(REPORTS_DIR)
	bash scripts/bench/trec-covid-ndcg.sh "trec-covid-surch-$(SHA)" $(REPORTS_DIR)/trec-covid-surch.out $(SURCH_URL)
	bash scripts/bench/trec-covid-ndcg.sh "trec-covid-os-$(SHA)"    $(REPORTS_DIR)/trec-covid-os.out    $(OS_URL)
	$(MAKE) surch-down opensearch-down

# bench-pair-<workload> — orchestrate a single workload against Surch then
# OpenSearch via scripts/bench/run-pair.sh. Both engines are trap-cleaned
# even on crash. Workload is the wildcard portion (ban25k, insee25k, ...).
bench-pair-%: | $(REPORTS_DIR)
	bash scripts/bench/run-pair.sh $* $(REPORTS_DIR)

bench-stress: opensearch-up surch-up | $(REPORTS_DIR)
	bash scripts/bench/artillery-replay.sh "art-surch-$(SHA)" $(REPORTS_DIR)/art-surch.out $(SURCH_URL)
	$(MAKE) surch-down
	bash scripts/bench/artillery-replay.sh "art-os-$(SHA)"    $(REPORTS_DIR)/art-os.out    $(OS_URL)
	$(MAKE) opensearch-down

# B-RUST-HARNESS: keep-alive Rust harness replacement for the bash
# artillery-replay.sh. Each engine is benched with the matchID phase
# scenario (2,2,5,10,20,50 rps), a shared hyper-util HTTP/1.1 pool of
# $(ART_WORKERS) workers, and emits surch.bench.artillery.v1 JSON.
ART_WORKERS ?= 8
ART_PHASES  ?= 2:30,2:30,5:30,10:30,20:30,50:60
ART_NAMES   ?= /home/antoinefa/src/surch/target/insee/artillery_names.txt
ART_INDEX   ?= deces_25k

bench-artillery-rs: opensearch-up surch-up | $(REPORTS_DIR)
	cargo build --release -p surch-demo --bin artillery_bench --locked
	./target/release/artillery_bench \
	    --url $(SURCH_URL) --index $(ART_INDEX) --names $(ART_NAMES) \
	    --workers $(ART_WORKERS) --phases '$(ART_PHASES)' \
	    --report $(REPORTS_DIR)/art-surch.json
	$(MAKE) surch-down
	./target/release/artillery_bench \
	    --url $(OS_URL) --index $(ART_INDEX) --names $(ART_NAMES) \
	    --workers $(ART_WORKERS) --phases '$(ART_PHASES)' \
	    --report $(REPORTS_DIR)/art-os.json
	$(MAKE) opensearch-down
	@echo "bench-artillery-rs reports under $(REPORTS_DIR)"

bench-perf: bench-local bench-stress
	@echo "bench-perf reports under $(REPORTS_DIR)"

bench-remote-scw:
	@echo "scw harness not implemented yet — see docs/ops/test-automation-plan.md"
	@exit 1

# ---------------------------------------------------------------------------
# K8s burst-pool dispatch (poc-k8s)
# ---------------------------------------------------------------------------
# Dispatches the .github/workflows/ci-k8s.yml workflow that runs a Surch Job
# (ndcg-gate | insee-bench | 00-init-corpora) on the Scaleway Kapsule burst
# pool. Set K8S_DRY_RUN=1 to print the gh command without dispatching.
K8S_JOB ?= ndcg-gate
K8S_REF ?= $(shell git rev-parse --abbrev-ref HEAD 2>/dev/null || echo main)
K8S_WATCH ?= 1
K8S_DRY_RUN ?= 0
K8S_IMAGE_REPO ?= ghcr.io/rhanka/surch
K8S_SHA ?= $(shell git rev-parse "$(K8S_REF)" 2>/dev/null || git rev-parse HEAD 2>/dev/null || echo unknown)
K8S_IMAGE_TAG ?= sha-$(K8S_SHA)
K8S_IMAGE ?= $(K8S_IMAGE_REPO):$(K8S_IMAGE_TAG)
K8S_BENCH_IMAGE_TAG ?= bench-sha-$(K8S_SHA)
K8S_BENCH_IMAGE ?= $(K8S_IMAGE_REPO):$(K8S_BENCH_IMAGE_TAG)
bench-k8s:
	@case "$(K8S_JOB)" in \
	  ndcg-gate|insee-bench|00-init-corpora) ;; \
	  *) echo "bench-k8s: invalid K8S_JOB=$(K8S_JOB) (expected ndcg-gate|insee-bench|00-init-corpora)" >&2; exit 2 ;; \
	esac
	@if [ "$(K8S_SHA)" = "unknown" ]; then \
	  echo "bench-k8s: could not resolve K8S_REF=$(K8S_REF) to a commit SHA" >&2; \
	  exit 2; \
	fi
	@echo "bench-k8s: expected runtime image=$(K8S_IMAGE)"
	@if [ "$(K8S_JOB)" != "00-init-corpora" ]; then \
	  echo "bench-k8s: expected bench driver image=$(K8S_BENCH_IMAGE)"; \
	fi
	@echo "bench-k8s: if the image is missing, run: gh workflow run docker-build.yml --ref $(K8S_REF)"
	@echo "bench-k8s: dispatching ci-k8s.yml (job=$(K8S_JOB), ref=$(K8S_REF))"
	@if [ "$(K8S_DRY_RUN)" = "1" ]; then \
	  echo "gh workflow run docker-build.yml --ref $(K8S_REF)"; \
	  echo "gh workflow run ci-k8s.yml --ref $(K8S_REF) -f job=$(K8S_JOB)"; \
	else \
	  gh workflow run ci-k8s.yml --ref "$(K8S_REF)" -f job="$(K8S_JOB)"; \
	  sleep 5; \
	  run_id=$$(gh run list --workflow=ci-k8s.yml --branch "$(K8S_REF)" --event workflow_dispatch --limit=1 --json databaseId --jq '.[0].databaseId'); \
	  run_url=$$(gh run list --workflow=ci-k8s.yml --branch "$(K8S_REF)" --event workflow_dispatch --limit=1 --json url --jq '.[0].url'); \
	  if [ -z "$$run_id" ]; then \
	    echo "bench-k8s: dispatched workflow, but could not resolve the run id for ref $(K8S_REF)" >&2; \
	    exit 1; \
	  fi; \
	  echo "bench-k8s: run_id=$$run_id"; \
	  if [ -n "$$run_url" ]; then \
	    echo "bench-k8s: run_url=$$run_url"; \
	  fi; \
	  if [ "$(K8S_WATCH)" = "1" ]; then \
	    gh run watch --exit-status "$$run_id"; \
	  fi; \
	fi

bench-all: bench-local bench-recall bench-stress

# bench_report aggregates target/bench-reports/<sha>/*.json envelopes into
# a Markdown summary and gates the run on the matchID v1 SLOs. Set
# REPORT_BASELINE=target/bench-reports/<other_sha> to also enforce the
# regression budget (p95 +15 %, RSS +25 %). Exit code is 0 iff every SLO
# passes and no regression breaches its threshold.
REPORT_BASELINE ?=
report:
	cargo build --release -p surch-demo --bin bench_report --locked
	@if [ -n "$(REPORT_BASELINE)" ]; then \
	  ./target/release/bench_report --dir $(REPORTS_DIR) --baseline $(REPORT_BASELINE); \
	else \
	  ./target/release/bench_report --dir $(REPORTS_DIR); \
	fi

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
# SBOM (CycloneDX)
# ---------------------------------------------------------------------------
# Generates one bom.json per crate at the workspace root + per crate dir.
# CI re-runs this in publish-release and renames the workspace bom.json to
# dist/surch-sbom-<tag>.cdx.json before attaching it to the release.
sbom:
	cargo install --locked cargo-cyclonedx
	cargo cyclonedx --format json --output-pattern bom

# ---------------------------------------------------------------------------
# Clean
# ---------------------------------------------------------------------------
clean: surch-down opensearch-down
	cargo clean
