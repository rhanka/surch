# Surch packaging plan

Date: 2026-05-15. Last checked against the repo: 2026-05-18.
Surch is still `0.1.0` alpha and in-memory only, but the packaging
baseline has moved: Dockerfile, Helm chart, release workflow,
minisign signing, cosign image signing, CycloneDX SBOM generation and
`scripts/verify-release.sh` are now present in the repo. The release
profile in `Cargo.toml` already has `opt-level = 3`, `lto = true`,
`codegen-units = 1`, `strip = true`.

## Status checkpoint — repo state on 2026-05-18

### Already shipped on `main`

- Multi-stage distroless image in `Dockerfile`.
- Local docker helpers in `Makefile` (`docker-build`, `docker-smoke`).
- Release workflow in `.github/workflows/release.yml` with
  `cargo-dist`, five binary targets, minisign, CycloneDX SBOM, GitHub
  release publication, `ghcr.io` image push, cosign signing and SBOM
  attestation.
- Tag-triggered fallback image workflow in
  `.github/workflows/docker-build.yml`.
- Minimal Helm chart in `charts/surch/` using a `Deployment`, not the
  older StatefulSet sketch.
- Public minisign verification flow documented in `README.md` and
  helper verification script in `scripts/verify-release.sh`.

### Still open / intentionally incomplete

- No evidence yet in this repo of a published Helm repository via
  `helm/chart-releaser-action`.
- No Docker Hub mirror; publication is scoped to `ghcr.io`.
- The Docker build is multi-stage distroless, but not the earlier
  `cargo-chef` / `docker-bake.hcl` design.
- Snapshot packaging remains partially delivered: `_snapshot` /
  `_slm` are present, but persistent registries, S3 e2e and retention
  are still open.

### Next Track C steps

1. Keep release verification reproducible from workflow artefacts and
   cite release run ids in Track C reports.
2. Decide whether chart publication is still needed or whether a
   repo-local chart is sufficient for this phase.
3. Finish the remaining snapshot / SLM gaps documented in
   `docs/ops/snapshot-plan.md`.

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
            │ Deployment + Service + probes                 │
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

#### cargo-dist wiring

`[workspace.metadata.dist]` in `Cargo.toml` pins
`cargo-dist-version = "0.25.1"` and lists the five release targets. The
`release.yml` workflow runs three layered jobs:

1. `dist-plan` runs `cargo dist plan --output-format=json` and emits the
   target matrix as a GitHub Actions output.
2. `dist-build` fans out across that matrix; each runner installs
   `cargo-dist`, the matching Rust target, and the linux cross
   toolchain when needed (`gcc-aarch64-linux-gnu`, `musl-tools`).
   Apple targets run on `macos-13`. `cargo dist build --artifacts=local`
   produces `target/distrib/*.tar.xz` + `.sha256`.
3. `publish-release` collects every archive, signs it with minisign,
   builds the CycloneDX SBOM, and hands the lot to
   `softprops/action-gh-release`.

The previous hand-rolled `build-binaries` matrix (only linux gnu) is
gone; macOS + musl are now part of every tag.

#### minisign key lifecycle

Key generation is out-of-band — running `minisign -G` inside CI would
leak the seedphrase into the runner. The release maintainer runs once,
on a trusted workstation:

```bash
minisign -G -p surch.pub -s surch.key
```

Then:

- commit `surch.pub` at the repository root (already done);
- store `surch.key` base64-encoded in the GitHub Actions secret
  `MINISIGN_PRIVATE_KEY` (`base64 -w0 < surch.key | gh secret set
  MINISIGN_PRIVATE_KEY`);
- store the passphrase in `MINISIGN_PASSWORD`;
- keep the on-disk `surch.key` in an offline password manager; never
  push it to git.

`publish-release` decodes the key into a private tmpfs path
(`/tmp/minisign.key`, `umask 077`), signs every archive
(`minisign -Sm <archive> -s /tmp/minisign.key`), uploads the
resulting `.minisig` files alongside the archives on the GitHub
release, and shreds the key on job exit (`shred -u`).

Downstream verification command (also in `README.md`):

```bash
minisign -Vm surch-api-<ver>-<triple>.tar.xz -p surch.pub
```

### 2. Versioning

- **SemVer strict** on the Surch binary and on the REST surface that is **not** OS-compatible.
- Separate `opensearch_compat_version` field exposed in the `GET /` root response (mirrors Quickwit's `quickwit_version` + `elasticsearch_version` split).
- Surch stays in `0.x.y` until the internal API is stable; breaking changes on minor are allowed during alpha (state explicitly in `CONTRIBUTING.md`).
- `opensearch_compat_version` is already wired and the live conductor
  plan tracks `2.17.1` as the current compatibility target.
- Git tag `v0.2.0` style; do not publish individual crates on crates.io until someone needs them as a library.
- **Effort**: 0.5 day (root endpoint change + policy doc).

### 3. Docker image

- **Base**: `gcr.io/distroless/cc-debian12:nonroot`. Provides `/etc/ssl/certs` for S3 TLS and `/etc/passwd` for the `nonroot` user. Pure `scratch` is rejected for those reasons.
- **Build pipeline**: current repo state is a plain multi-stage Docker build locally plus multi-arch publication from `release.yml` / `docker-build.yml`. The earlier `cargo-chef` / `docker-bake.hcl` sketch has not landed.
- **Publication**: primary `ghcr.io/rhanka/surch`; Docker Hub mirror remains future work.
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

Status on 2026-05-18: this section is no longer purely prospective.
The repo already ships `_snapshot` / `_slm` routes, filesystem
take/get/delete/restore coverage, S3 repository registration, and the
background SLM scheduler. The open items are persistent registries,
real S3 e2e, retention enforcement and richer restore fencing.

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

Status on 2026-05-18:

- Phase A is partially landed: release workflow, Dockerfile, local
  docker smoke path, Helm chart and compat version split all exist.
- Phase B is partially landed: `/_prometheus_metrics`, `_snapshot`
  and `_slm` exist, but snapshot persistence / S3 e2e / retention are
  not finished.
- Phases C and D remain open.

1. **Phase A — week 1**
   - D1–D2: `cargo-dist` + GitHub Actions `release.yml` (tag-triggered, 5 targets, sha256, minisign)
   - D3: `Dockerfile` (current repo state: multi-stage + distroless), push ghcr.io + cosign keyless
   - D4: minimal Helm chart in `charts/surch/` (current repo state: `Deployment` + Service + probes); chart publication still open
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
