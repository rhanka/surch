# Surch packaging plan

Date: 2026-05-15. Surch is `0.1.0` alpha, in-memory only, no Dockerfile or
release CI yet. The release profile in `Cargo.toml` already has
`opt-level = 3`, `lto = true`, `codegen-units = 1`, `strip = true`.

## Artifact pipeline

```
                                          ┌──────────────────────────┐
                                          │   managed offering       │ (future SaaS)
                                          │   (terraform + console)  │
                                          └────────────▲─────────────┘
                                                       │
                          ┌────────────────────────────┴───────────────┐
                          │  surch-operator (kube-rs CRDs)             │
                          │  SurchCluster, SurchSnapshot               │
                          └────────────▲───────────────────────────────┘
                                       │
            ┌──────────────────────────┴────────────────────┐
            │ Helm chart (charts/surch)                     │
            │ StatefulSet + Service + ConfigMap + PVC       │
            └────────────▲──────────────────────────────────┘
                         │
   ┌─────────────────────┴────────┐
   │  OCI image ghcr.io/rhanka/   │
   │  surch:<semver> (distroless  │
   │  + multi-arch amd64/arm64)   │
   └─────────────▲────────────────┘
                 │
   ┌─────────────┴────────────────┐
   │ Static binary               │
   │ surch-api-<ver>-<triple>    │ (5 targets) + sha256 + minisign
   └─────────────────────────────┘
```

## Decisions by axis

### 1. Binary distribution

- **Tool**: `cargo-dist` (https://github.com/axodotdev/cargo-dist) drives the 5 targets
  - `x86_64-unknown-linux-gnu`
  - `x86_64-unknown-linux-musl` (static, used inside the distroless image)
  - `aarch64-unknown-linux-gnu`
  - `aarch64-apple-darwin`
  - `x86_64-apple-darwin`
- Windows deferred (no server-side use case).
- Symbols stripped from the shipped binary, kept in a separate `.dbg` file via `objcopy --only-keep-debug` for `addr2line` debugging.
- **Signing**: `minisign` (Rust crate `rsign2`). Cosign is reserved for the OCI image.
- References: `astral-sh/ruff`, `astral-sh/uv` use exactly this `cargo-dist` setup.
- **Effort**: 1–2 days.

### 2. Versioning

- **SemVer strict** on the Surch binary and on the REST surface that is **not** OS-compatible.
- Separate `opensearch_compat_version` field exposed in the `GET /` root response (mirrors Quickwit's `quickwit_version` + `elasticsearch_version` split).
- Surch stays in `0.x.y` until the internal API is stable; breaking changes on minor are allowed during alpha (state explicitly in `CONTRIBUTING.md`).
- `opensearch_compat_version` pinned at `2.11.0` today (per `spec/`). Bumps documented in `CHANGELOG.md`.
- Git tag `v0.2.0` style; do not publish individual crates on crates.io until someone needs them as a library.
- **Effort**: 0.5 day (root endpoint change + policy doc).

### 3. Docker image

- **Base**: `gcr.io/distroless/cc-debian12:nonroot`. Provides `/etc/ssl/certs` for S3 TLS and `/etc/passwd` for the `nonroot` user. Pure `scratch` is rejected for those reasons.
- **Build pipeline**: `cargo-chef` for dependency caching + multi-stage builder + `docker buildx` multi-arch (`linux/amd64`, `linux/arm64`).
- **Publication**: primary `ghcr.io/rhanka/surch` (OIDC auth, free, scoped to the repo); mirror to Docker Hub `surch/surch` for visibility.
- **Tags**: `latest`, `0.2`, `0.2.3`, `sha-<short>`, `edge` (main).
- **Signing**: cosign keyless (OIDC GitHub Actions). SBOM via `cargo-cyclonedx` (CycloneDX format) attached to each tag.
- **Verification command** (downstream consumers):

  ```bash
  cosign verify ghcr.io/rhanka/surch:<tag> \
    --certificate-identity-regexp 'https://github.com/rhanka/surch/.github/workflows/release\.yml@.*' \
    --certificate-oidc-issuer https://token.actions.githubusercontent.com
  ```

  The signature is anchored on the immutable image digest emitted by
  `docker/build-push-action`; every tag pushed by `docker/metadata-action`
  resolves to the same manifest, so a single `cosign sign` covers them all.
- **Size target**: < 30 MB compressed. Reference: `opensearchproject/opensearch:2.11.0` ≈ 1.2 GB (anti-example), `getmeili/meilisearch` ≈ 150 MB, `quickwit/quickwit` ≈ 80 MB.
- **Effort**: 1 day.

#### SBOM (CycloneDX)

The `release.yml` workflow ships a CycloneDX 1.5 SBOM alongside every
tag and bound to the OCI image:

1. `publish-release` installs `cargo-cyclonedx` and runs
   `cargo cyclonedx --format json --all --target-in-filename`. The
   workspace-level `bom.json` is renamed to
   `dist/surch-sbom-<tag>.cdx.json` and:
   - uploaded as an artifact (`sbom-cyclonedx`) for the next job,
   - attached to the GitHub release as a public asset.
2. `publish-image` downloads that artifact, then runs

   ```
   cosign attest --yes \
     --predicate dist/surch-sbom-<tag>.cdx.json \
     --type cyclonedx \
     ghcr.io/rhanka/surch@<digest>
   ```

   so the SBOM is anchored on the same image digest as the `cosign
   sign` signature. End users verify both with `cosign verify` +
   `cosign verify-attestation --type cyclonedx`.

A helper script `scripts/verify-release.sh <image-ref>` runs both
checks for downstream consumers and prints the top 5 dependencies
extracted from the verified SBOM payload. Locally, contributors can
generate the same SBOM offline via `make sbom`.

SPDX support is on the Phase D backlog; CycloneDX is the format
required by most enterprise procurement flows today, so it is the
one shipped first.

### 4. Snapshots compatible with the Elasticsearch SLM surface

Implement the REST subset that real clients use:

```
PUT  /_snapshot/{repo}              # register S3 repo
PUT  /_snapshot/{repo}/{name}       # take snapshot
GET  /_snapshot/{repo}/{name}       # status
POST /_snapshot/{repo}/{name}/_restore
DELETE /_snapshot/{repo}/{name}
PUT  /_slm/policy/{id}              # cron schedule
```

**On-S3 layout** (prefix per snapshot; segments, not a single tarball, for parallel download + partial restore):

```
{repo-prefix}/
  index-{N}                     # root manifest JSON (active snapshots list)
  snap-{uuid}.dat               # snapshot metadata
  indices/{index-uuid}/
    meta-{snap-uuid}.dat        # mapping + settings
    0/data/__1, __2, ...        # NDJSON corpus chunks (gzip)
```

Close to ES `BlobStoreRepository` to keep clients happy and force us to think about versioning from day 1. Every manifest carries a `surch_snapshot_format_version: u32`; restore refuses unknown versions.

- **Rust stack**: `aws-sdk-s3` (official AWS SDK; rusoto is abandoned). Cron via `tokio-cron-scheduler`. Auth via `aws-config` (env, ECS/EC2 metadata, profile, static keys).
- **Counter-model**: Quickwit `.split` files are designed for serverless search-from-S3, not snapshot/restore of a local in-memory index. Wrong fit for Surch.
- **Effort**: 8–12 days (restore is heavier than snapshot — requires write fencing during restore + format validation).
- **Non-negotiable**: an end-to-end CI test `take → wipe → restore → cat indices` in the same PR that ships restore.

### 5. Kubernetes operator

- **Library**: `kube-rs` (https://github.com/kube-rs/kube). Surch is all-Rust; the kube-rs ecosystem is production-grade (Stackable runs ~10 operators on it).
- **MVP CRDs**: `SurchCluster` (image, replicas, resources, storage PVC template, snapshot repo Secret) + `SurchSnapshot` (cluster ref, cron, retention). **No `SurchIndex` CRD** at the start — let users drive the REST API. ECK has had recurring issues with `Elasticsearch.spec.nodeSets` drifting versus `cat indices`.
- StatefulSet for data nodes (stable PVC + hostname). Single StatefulSet until cluster mode lands; no router/data split today.
- **Effort**: 15–25 days. Do **not** ship before Surch has a real cluster mode — a one-pod operator is `kubectl apply -f deployment.yaml` in disguise.

## Sequencing

1. **Phase A — week 1**
   - D1–D2: `cargo-dist` + GitHub Actions `release.yml` (tag-triggered, 5 targets, sha256, minisign)
   - D3: `Dockerfile` (cargo-chef multi-stage + distroless), `docker-bake.hcl` multi-arch, push ghcr.io + cosign keyless
   - D4: minimal Helm chart in `charts/surch/` (StatefulSet + Service + ConfigMap), publish via `helm/chart-releaser-action`
   - D5: `INSTALL.md` docs (binary / docker / helm), `opensearch_compat_version` in `crates/surch-api/src/root.rs`
2. **Phase B — weeks 2–3**
   - `/_prometheus_metrics` (`metrics-exporter-prometheus`)
   - OpenTelemetry export via `opentelemetry-otlp` (tracing is already wired)
   - Snapshots S3: register + take + status + delete first, restore at the end of the phase. SLM cron policy as a bonus.
3. **Phase C — weeks 4–6**
   - `surch-operator` (kube-rs) with `SurchCluster` and `SurchSnapshot`
   - Helm chart for the operator
   - e2e tests on `kind` (`kubectl get surchcluster`)
4. **Phase D — later**
   - Terraform provider (accept Go for this; Rust providers are not first-class yet)
   - Marketplace listings, hosted offering
   - SPDX SBOM in addition to CycloneDX

## Known pitfalls (from ES/OpenSearch history)

- **Unversioned snapshot format** → catastrophic cross-version restore. Mitigation: `surch_snapshot_format_version: 1` in the very first manifest published; refuse `> COMPAT_MAX` explicitly.
- **Image > 100 MB** → security signal + bandwidth + slow cold start. Mitigation: distroless + `strip = true` + statically linked musl binary in the image.
- **Operator before cluster mode** → reconciles a single pod, equivalent to a plain Deployment. Wait until `surch-store` ships replication.
- **Version number confusion** → tagging `v8.11.0` to match OS compat would look like an ES fork. Mitigation: keep `v0.x.y` Surch and use the separate `opensearch_compat_version` field. Quickwit does exactly this.
- **Snapshot restore not tested** → silent metadata corruption is the classic ES-era bug. Mitigation: the e2e take/restore test is non-negotiable, blocks the PR that introduces snapshots.
- **Keyless cosign without published verification command** → signature theatre. Mitigation: README ships `cosign verify ghcr.io/rhanka/surch:0.2.0 --certificate-identity-regexp ...` from D3.
- **Helm chart too permissive too early** → 700-line `values.yaml` no one reads (ECK). Mitigation: stay minimal (image, replicas, resources, storage, env). Meilisearch community chart at ~80 lines is the role model.

## References

- `axodotdev/cargo-dist`, `astral-sh/uv` — multi-arch Rust release pattern
- `tikv/tikv` Dockerfile — cargo-chef + distroless
- `quickwit-oss/quickwit` — root endpoint compat version, helm chart
- `kube-rs/kube`, `stackabletech/operator-rs` — production Rust operators
- `elastic/elasticsearch` `BlobStoreRepository` — ES snapshot format
- `elastic/cloud-on-k8s`, `opensearch-project/opensearch-k8s-operator` — what not to do too early
- `getmeili/meilisearch` — pragmatic versioning + minimal packaging
