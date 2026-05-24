# Release verification

Status: Track C Lot 4 closure. The helper `scripts/verify-release.sh`
reproduces every signed/attested artefact from the CI release pipeline
(`.github/workflows/release.yml` + `.github/workflows/docker-build.yml`)
so any consumer can audit a published Surch tag end to end from public
artefacts only.

This page documents:

1. What the release pipeline publishes for every `v*.*.*` tag.
2. How to run `scripts/verify-release.sh` (full mode + legacy image mode).
3. The expected console output and exit codes.
4. Troubleshooting and inspection paths for a failing release run.

## What the release pipeline publishes

For every git tag matching `v[0-9]+.[0-9]+.[0-9]+` (and `v*-prerelease`),
two workflows run in parallel:

- `release.yml` (cargo-dist driven)
  - `dist-build`: builds five archive targets
    (`x86_64-unknown-linux-{gnu,musl}`, `aarch64-unknown-linux-gnu`,
    `{x86_64,aarch64}-apple-darwin`) into `target/distrib/*.tar.xz`
    with sidecar `*.sha256`.
  - `publish-release`: signs every archive with minisign (Ed25519,
    repo public key at `surch.pub`), regenerates a top-level
    `SHA256SUMS`, builds a CycloneDX SBOM
    (`surch-sbom-<tag>.cdx.json`), and uploads the lot to the GitHub
    Release via `softprops/action-gh-release`.
  - `publish-image`: pushes the multi-arch runtime image to
    `ghcr.io/<owner>/surch:<semver>` (and `:sha-<full>`, `:major`,
    `:major.minor`), cosign-signs the immutable digest with keyless
    OIDC, and attests the CycloneDX SBOM against the same digest.
- `docker-build.yml`
  - Pushes the runtime image again under the same tags (fallback path
    independent from cargo-dist) plus the bench driver image
    (`ghcr.io/<owner>/surch:bench-<semver>`,
    `:bench-sha-<full>`, …). The bench image is **not** cosign-signed
    today — this is intentional, it is a debug/bench driver, not a
    runtime artefact.

The minisign public key (`surch.pub`) is committed at the repository
root and re-uploaded as a release asset for every tag, so downstream
consumers can pin either the in-repo copy or the per-release copy.

## Running the verifier

The script lives at `scripts/verify-release.sh` and has two modes:

### Full release verification (Lot 4)

```bash
scripts/verify-release.sh v0.1.0
```

This downloads every GitHub Release asset for `v0.1.0` into a temp
workdir and performs, in order:

1. **Release inventory**: `gh release view v0.1.0` and assert the
   expected asset families are present (`*.tar.xz`, `*.minisig`,
   `SHA256SUMS`, `surch-sbom-*.cdx.json`, `surch.pub`).
2. **Download**: `gh release download v0.1.0 --clobber` into the
   workdir. Re-runs are idempotent.
3. **sha256**: `sha256sum -c SHA256SUMS` replayed locally over the
   downloaded files (`--ignore-missing` so a partial download still
   yields a useful tally).
4. **minisign**: `minisign -Vm <archive> -p surch.pub` for every
   `*.tar.xz` / `*.zip` archive paired with a `*.minisig`.
5. **SBOM (release asset)**: parse `surch-sbom-<tag>.cdx.json` with
   `jq` and assert it is CycloneDX 1.x with at least one component.
6. **OCI image**:
   - `cosign verify --certificate-identity-regexp … --certificate-oidc-issuer …
     ghcr.io/<owner>/surch:<version>` (drops the leading `v` to match
     the `{{version}}` tag emitted by `docker/metadata-action`).
   - `cosign verify-attestation --type cyclonedx …` on the same image.
   - `docker manifest inspect ghcr.io/<owner>/surch:bench-<version>`
     (pull check only — bench image is unsigned by design, reported
     as `SKIP` in the cosign column).

### Image-only verification (legacy)

Kept for back-compat with `docs/ops/packaging-plan.md`:

```bash
scripts/verify-release.sh ghcr.io/rhanka/surch:0.1.0
```

Runs only steps 6a and 6b (cosign verify + verify-attestation) and
additionally prints the top 5 SBOM components by encoded size,
extracted from the verified attestation payload.

### Dependencies

| Mode        | Required CLI binaries                                            |
|-------------|------------------------------------------------------------------|
| Full        | `gh`, `minisign`, `cosign` (>= v2.4), `docker`, `jq`, `sha256sum`, `base64`, `tar` |
| Image-only  | `cosign` (>= v2.4), `jq`                                         |

Install hints:

- `cosign`: download from
  <https://github.com/sigstore/cosign/releases> (v2.4.1 pinned in CI).
- `minisign`: `apt install minisign` on Debian/Ubuntu, `brew install
  minisign` on macOS.
- `gh`: <https://cli.github.com/>; run `gh auth login` (or set
  `GH_TOKEN`) before the first run.

### Environment overrides

| Variable                       | Default                                                                  | Purpose                                                      |
|--------------------------------|--------------------------------------------------------------------------|--------------------------------------------------------------|
| `SURCH_REPO`                   | `rhanka/surch`                                                           | GitHub repo slug (fork support).                             |
| `SURCH_IMAGE`                  | `ghcr.io/<owner>/surch`                                                  | OCI image base (mirror or fork support).                     |
| `SURCH_PUBKEY`                 | `<repo>/surch.pub`                                                       | Path to the minisign verification key.                       |
| `SURCH_WORKDIR`                | `$(mktemp -d)`                                                           | Download / verification scratch dir. Auto-deleted unless set. |
| `SURCH_KEEP_WORKDIR=1`         | unset                                                                    | Keep the workdir after exit for inspection.                  |
| `COSIGN_CERT_IDENTITY_REGEX`   | `https://github.com/<repo>/.github/workflows/release\.yml@.*`            | Override cosign certificate identity regex (fork support).   |
| `COSIGN_CERT_OIDC_ISSUER`      | `https://token.actions.githubusercontent.com`                            | Override OIDC issuer (typically not needed).                 |

## Expected output

A clean run prints one `==>` section per step and a final verdict
table. Truncated example for `v0.1.0`:

```
==> full release verification
    tag       : v0.1.0
    version   : 0.1.0
    repo      : rhanka/surch
    image     : ghcr.io/rhanka/surch:0.1.0
    bench img : ghcr.io/rhanka/surch:bench-0.1.0
    workdir   : /tmp/surch-verify-XXXXXX
    pubkey    : /…/surch.pub
==> 1/6 release inventory (gh release view)
    found 17 asset(s)
      - SHA256SUMS
      - surch-api-x86_64-unknown-linux-gnu.tar.xz
      - surch-api-x86_64-unknown-linux-gnu.tar.xz.minisig
      - … (one pair per target)
      - surch-sbom-v0.1.0.cdx.json
      - surch.pub
    OK    : tar.xz archives (\.tar\.xz$)
    OK    : minisign signatures (\.minisig$)
    OK    : SHA256SUMS manifest (^SHA256SUMS$)
    OK    : CycloneDX SBOM (surch-sbom-.*\.cdx\.json$)
    OK    : minisign public key asset (^surch\.pub$)
==> 2/6 download assets (gh release download)
    downloaded into /tmp/surch-verify-XXXXXX/dist
==> 3/6 sha256 checksums (SHA256SUMS)
    OK: 10 file(s) match SHA256SUMS
==> 4/6 minisign signatures (binaries + zip archives)
    OK   : surch-api-x86_64-unknown-linux-gnu.tar.xz
    OK   : surch-api-x86_64-unknown-linux-musl.tar.xz
    OK   : surch-api-aarch64-unknown-linux-gnu.tar.xz
    OK   : surch-api-x86_64-apple-darwin.tar.xz
    OK   : surch-api-aarch64-apple-darwin.tar.xz
    summary: 5 OK, 0 FAIL, 0 missing-sig
==> 5/6 SBOM asset (CycloneDX JSON)
    OK: surch-sbom-v0.1.0.cdx.json (314 components)
==> 6/6 image checks
    runtime: ghcr.io/rhanka/surch:0.1.0
    bench  : ghcr.io/rhanka/surch:bench-0.1.0
    OK    : cosign signature on ghcr.io/rhanka/surch:0.1.0
    OK    : cosign CycloneDX attestation on ghcr.io/rhanka/surch:0.1.0
    OK    : bench image manifest reachable
    NOTE  : bench image is intentionally not cosign-signed today
            (built by docker-build.yml, separate workflow)
==> verdict
    release inventory                        : OK
    download assets                          : OK
    sha256 checksums                         : OK
    minisign signatures                      : OK
    SBOM asset (release)                     : OK
    image cosign signature                   : OK
    image SBOM attestation                   : OK
    bench image pullable                     : OK
    bench image cosign                       : SKIP
```

### Exit codes

| Exit | Meaning                                                                |
|------|------------------------------------------------------------------------|
| 0    | All verifications passed (`SKIP` lines do not fail the run).           |
| 1    | At least one verification failed (fail-closed).                        |
| 2    | Bad usage (missing arg, tag does not match `v<semver>`).               |
| 3    | Missing dependency (`gh`, `cosign`, `minisign`, `docker`, `jq`, …).    |

## Troubleshooting

### `release not found` / `gh: HTTP 404`

The tag does not exist as a published GitHub Release yet, or `gh` is
not authenticated. Check:

```bash
gh auth status
gh release view v0.1.0 --repo rhanka/surch
```

If the release has not been cut yet but the workflow has produced
draft artefacts you want to inspect, download them directly from the
workflow run instead, then point the verifier at the resulting dir:

```bash
gh run list --workflow release.yml --limit 5
gh run download <run-id> -D /tmp/surch-verify-manual
SURCH_WORKDIR=/tmp/surch-verify-manual scripts/verify-release.sh v0.1.0
```

(The script still expects an actual GitHub Release for the inventory
step; if you only have run artefacts, skip steps 1–2 and run
`sha256sum -c`, `minisign -Vm`, `cosign verify` manually using the
commands documented above.)

### `cosign: signature not verified`

CI pins `cosign-installer@v3` with `cosign-release: v2.4.1`. Older
local cosign (<= 2.2) does not recognise the rekor entry layout used
by the workflow and will spuriously fail. Upgrade:

```bash
cosign version
# if < 2.4: install from https://github.com/sigstore/cosign/releases
```

For a fork that rewrites the workflow path, override the identity
regex:

```bash
COSIGN_CERT_IDENTITY_REGEX='https://github.com/my-fork/surch/.github/workflows/release\.yml@.*' \
  scripts/verify-release.sh v0.1.0
```

### `minisign: Signature verification failed`

The repo-root `surch.pub` is the canonical key for signed releases.
Three causes, in order of likelihood:

1. The key was rotated and the local checkout is stale — run
   `git pull` (or pass `SURCH_PUBKEY=/path/to/new-surch.pub`).
2. The asset was tampered with after publication — the same run will
   also fail the `sha256` step; cross-check.
3. The release was signed under a different key in a fork —
   download `surch.pub` from the release assets:
   `SURCH_PUBKEY=$(mktemp); gh release download v0.1.0 --pattern surch.pub -O "${SURCH_PUBKEY}"`.

### `bench image … not reachable`

`ghcr.io/<owner>/surch:bench-<version>` is published by
`docker-build.yml`. If `docker-build.yml` failed or has not run for
this tag yet, the bench image will be absent. Inspect:

```bash
gh run list --workflow docker-build.yml --limit 5
```

Note: if the repository is private (or you exhausted GHCR anonymous
quotas), `docker login ghcr.io -u <user> -p <pat-with-read:packages>`
is required even for `docker manifest inspect`.

### Inspecting a failing release run

When the verifier reports a FAIL, the failing run on GitHub Actions
is the first thing to inspect:

```bash
# Latest 5 runs of the release workflow:
gh run list --workflow release.yml --limit 5

# Detail a specific run (logs grouped by job):
gh run view <run-id> --log

# Just the failing job:
gh run view <run-id> --log-failed

# Download the per-job artefacts (dist-plan, dist-<target>, sbom-cyclonedx):
gh run download <run-id> -D /tmp/surch-run-<run-id>
```

The `dist-plan` artefact contains the cargo-dist plan JSON used to
derive the build matrix; comparing it against `target/distrib/` of a
local `dist build` reproduces the same set of archives offline.

## Reproducing the release locally (offline)

The verifier checks published artefacts; reproducing them locally is
useful when chasing a divergence. The same commands run in CI:

```bash
# Plan + per-target build (same as dist-plan + dist-build):
cargo install --locked cargo-dist@0.25.1
dist plan --output-format=json | tee dist-plan.json
dist build --target x86_64-unknown-linux-gnu --artifacts=local
ls target/distrib/

# SBOM (same as publish-release):
cargo install --locked cargo-cyclonedx
cargo cyclonedx --format json --all --target-in-filename
# -> bom.json at workspace root (rename to surch-sbom-<tag>.cdx.json)

# Minisign signing requires the private key; locally you would use a
# scratch key:
#   minisign -G -p /tmp/scratch.pub -s /tmp/scratch.key
#   minisign -Sm target/distrib/*.tar.xz -s /tmp/scratch.key
```

The `make sbom` shortcut wraps the cargo-cyclonedx call for the SBOM
asset.

## See also

- `plan/wp-c-ops.md` — Track C ops plan (Lot 4 closure).
- `docs/ops/packaging-plan.md` — full packaging design (cargo-dist,
  minisign, cosign, CycloneDX).
- `.github/workflows/release.yml` — authoritative release pipeline.
- `.github/workflows/docker-build.yml` — runtime + bench image
  publication.
- `README.md` — end-user `minisign -Vm` and `cosign verify` snippets.
