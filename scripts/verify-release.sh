#!/usr/bin/env bash
# Verify a published Surch release end to end from CI artefacts.
#
# Two invocation modes:
#
#   1. Full release verification (Lot 4):
#        scripts/verify-release.sh v0.1.0
#
#      Verifies the entire release surface produced by
#      .github/workflows/release.yml and .github/workflows/docker-build.yml:
#        - downloads every GitHub Release asset for the tag
#        - replays SHA256SUMS via sha256sum -c
#        - verifies the minisign signature of every .tar.xz / .zip archive
#          against the surch.pub committed at the repo root
#        - sanity-checks the CycloneDX SBOM asset (presence + parseable JSON)
#        - verifies the cosign keyless signature of the runtime image
#          ghcr.io/<org>/surch:<version>
#        - verifies the cosign CycloneDX attestation on the same image
#        - confirms the bench driver image
#          ghcr.io/<org>/surch:bench-<version> is pullable (not signed —
#          docker-build.yml does not cosign-sign the bench image today)
#
#   2. Image-only verification (legacy, kept for back-compat with the
#      docs/ops/packaging-plan.md call site):
#        scripts/verify-release.sh ghcr.io/rhanka/surch:0.1.0
#
#      Runs only the cosign signature + SBOM attestation checks and prints
#      the top 5 SBOM components by encoded size.
#
# Exit codes:
#   0  all verifications passed
#   1  at least one verification failed (fail-closed)
#   2  bad usage
#   3  missing dependency
#
# Requires (full mode):
#   gh, minisign, cosign (>= v2.4), docker, jq, sha256sum, base64, tar
# Requires (image-only mode):
#   cosign, jq
#
# Environment overrides:
#   SURCH_REPO                  GitHub repo slug (default: rhanka/surch)
#   SURCH_IMAGE                 OCI image base   (default: ghcr.io/<owner>/surch)
#   SURCH_PUBKEY                minisign pub key (default: <repo>/surch.pub)
#   SURCH_WORKDIR               download directory (default: mktemp -d)
#   SURCH_KEEP_WORKDIR=1        do not delete SURCH_WORKDIR on exit
#   COSIGN_CERT_IDENTITY_REGEX  cosign identity regex override
#   COSIGN_CERT_OIDC_ISSUER     cosign OIDC issuer override
#
# Troubleshooting:
#   - "release not found": check `gh auth status` and that the tag exists at
#     https://github.com/<repo>/releases. For an unreleased run, use
#     `gh run download <run-id> -D <dir>` manually and re-point SURCH_WORKDIR.
#   - "cosign: signature not verified": cosign-installer in CI pins v2.4.1;
#     older local cosign (<= 2.2) may fail on the same signature. Upgrade.
#   - "minisign: Signature verification failed": surch.pub at the repo root
#     is the source of truth — if it has been rotated, pull the new key.
#   - The bench driver image is intentionally unsigned today (separate
#     workflow). The script reports a WARN, not a FAIL, when it is missing
#     a cosign signature.

set -euo pipefail

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
log()  { printf '%s\n' "$*"; }
note() { printf '    %s\n' "$*"; }
hdr()  { printf '==> %s\n' "$*"; }

require_bin() {
  local missing=0
  for bin in "$@"; do
    if ! command -v "${bin}" >/dev/null 2>&1; then
      printf 'error: %s not found in PATH\n' "${bin}" >&2
      missing=1
    fi
  done
  [ "${missing}" -eq 0 ] || exit 3
}

# Verdict table: name -> OK / FAIL / WARN / SKIP
declare -A VERDICT=()
declare -a VERDICT_ORDER=()

record() {
  local name="$1" status="$2"
  if [ -z "${VERDICT[${name}]+x}" ]; then
    VERDICT_ORDER+=("${name}")
  fi
  VERDICT[${name}]="${status}"
}

print_verdict() {
  hdr "verdict"
  local any_fail=0
  local k
  for k in "${VERDICT_ORDER[@]}"; do
    local v="${VERDICT[${k}]}"
    printf '    %-40s : %s\n' "${k}" "${v}"
    if [ "${v}" = "FAIL" ]; then any_fail=1; fi
  done
  return "${any_fail}"
}

# ---------------------------------------------------------------------------
# Argument parsing
# ---------------------------------------------------------------------------
ARG="${1:-}"
if [ -z "${ARG}" ]; then
  cat >&2 <<EOF
usage: $0 <tag>            full release verification (Lot 4)
       $0 <image-ref>      image-only signature + SBOM check

examples:
  $0 v0.1.0
  $0 ghcr.io/rhanka/surch:0.1.0
EOF
  exit 2
fi

REPO="${SURCH_REPO:-rhanka/surch}"
REPO_OWNER="${REPO%%/*}"
IMAGE_BASE="${SURCH_IMAGE:-ghcr.io/${REPO_OWNER}/surch}"
CERT_IDENTITY_REGEX="${COSIGN_CERT_IDENTITY_REGEX:-https://github.com/${REPO}/.github/workflows/release\\.yml@.*}"
CERT_OIDC_ISSUER="${COSIGN_CERT_OIDC_ISSUER:-https://token.actions.githubusercontent.com}"

# ---------------------------------------------------------------------------
# Mode 2 — image-only verification (back-compat)
# ---------------------------------------------------------------------------
if [[ "${ARG}" == *"/"* ]]; then
  require_bin cosign jq
  IMAGE="${ARG}"
  hdr "image-only mode: ${IMAGE}"

  hdr "cosign verify (signature)"
  if cosign verify \
        --certificate-identity-regexp "${CERT_IDENTITY_REGEX}" \
        --certificate-oidc-issuer "${CERT_OIDC_ISSUER}" \
        "${IMAGE}" >/dev/null 2>&1; then
    note "signature: OK"
    record "image signature" OK
  else
    note "signature: FAIL"
    record "image signature" FAIL
  fi

  hdr "cosign verify-attestation (CycloneDX SBOM)"
  ATT_JSON="$(mktemp)"
  trap 'rm -f "${ATT_JSON}"' EXIT
  if cosign verify-attestation \
        --type cyclonedx \
        --certificate-identity-regexp "${CERT_IDENTITY_REGEX}" \
        --certificate-oidc-issuer "${CERT_OIDC_ISSUER}" \
        "${IMAGE}" >"${ATT_JSON}" 2>/dev/null; then
    note "SBOM attestation: OK"
    record "image SBOM attestation" OK

    hdr "top 5 SBOM components (by encoded size)"
    jq -rs '
      .[0].payload
      | @base64d
      | fromjson
      | .predicate.components // []
      | map({
          name: ((.group // "") + (if .group then "/" else "" end) + .name),
          version: (.version // ""),
          bytes: (. | tojson | length)
        })
      | sort_by(-.bytes)
      | .[0:5]
      | .[]
      | "    \(.bytes)\t\(.name)@\(.version)"
    ' "${ATT_JSON}" || note "(could not parse SBOM payload)"
  else
    note "SBOM attestation: FAIL"
    record "image SBOM attestation" FAIL
  fi

  if print_verdict; then
    exit 0
  else
    exit 1
  fi
fi

# ---------------------------------------------------------------------------
# Mode 1 — full release verification (Lot 4)
# ---------------------------------------------------------------------------
TAG="${ARG}"
case "${TAG}" in
  v[0-9]*) : ;;
  *) printf 'error: tag %q must look like v<semver> (e.g. v0.1.0)\n' "${TAG}" >&2
     exit 2 ;;
esac

# Strip leading "v" — both cargo-dist's binary asset versions and the
# docker/metadata-action {{version}} tag drop it.
VERSION="${TAG#v}"

require_bin gh minisign cosign docker jq sha256sum base64 tar

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
PUBKEY="${SURCH_PUBKEY:-${REPO_ROOT}/surch.pub}"

if [ ! -f "${PUBKEY}" ]; then
  printf 'error: minisign public key not found at %s\n' "${PUBKEY}" >&2
  printf '       point SURCH_PUBKEY at the rotated key if it has moved.\n' >&2
  exit 3
fi

# Workdir: caller-provided or mktemp. Idempotent re-runs are honoured by
# `gh release download --clobber` so an existing workdir with stale assets
# is safe.
if [ -n "${SURCH_WORKDIR:-}" ]; then
  WORKDIR="${SURCH_WORKDIR}"
  mkdir -p "${WORKDIR}"
else
  WORKDIR="$(mktemp -d -t surch-verify-XXXXXX)"
fi

cleanup() {
  if [ -z "${SURCH_KEEP_WORKDIR:-}" ] && [ -z "${SURCH_WORKDIR:-}" ]; then
    rm -rf "${WORKDIR}"
  fi
}
trap cleanup EXIT

hdr "full release verification"
note "tag       : ${TAG}"
note "version   : ${VERSION}"
note "repo      : ${REPO}"
note "image     : ${IMAGE_BASE}:${VERSION}"
note "bench img : ${IMAGE_BASE}:bench-${VERSION}"
note "workdir   : ${WORKDIR}"
note "pubkey    : ${PUBKEY}"

# ---------------------------------------------------------------------------
# 1) Inventory expected assets via the GitHub Release API
# ---------------------------------------------------------------------------
hdr "1/6 release inventory (gh release view)"
ASSETS_JSON="${WORKDIR}/assets.json"
if ! gh release view "${TAG}" --repo "${REPO}" --json assets >"${ASSETS_JSON}" 2>/dev/null; then
  note "FAIL: release ${TAG} not found on ${REPO} (or gh not authenticated)"
  note "      try: gh auth status"
  record "release inventory" FAIL
  print_verdict || true
  exit 1
fi

ASSET_NAMES="$(jq -r '.assets[].name' "${ASSETS_JSON}")"
ASSET_COUNT="$(printf '%s\n' "${ASSET_NAMES}" | grep -c . || true)"
note "found ${ASSET_COUNT} asset(s)"
printf '%s\n' "${ASSET_NAMES}" | sed 's/^/      - /'

# Sanity: the release workflow always uploads at least these envelopes.
expect_present() {
  local pattern="$1" label="$2"
  if printf '%s\n' "${ASSET_NAMES}" | grep -Eq "${pattern}"; then
    note "OK    : ${label} (${pattern})"
  else
    note "FAIL  : ${label} missing — no asset matches ${pattern}"
    record "expected: ${label}" FAIL
  fi
}

expect_present '\.tar\.xz$'                "tar.xz archives"
expect_present '\.minisig$'                "minisign signatures"
expect_present '^SHA256SUMS$'              "SHA256SUMS manifest"
expect_present 'surch-sbom-.*\.cdx\.json$' "CycloneDX SBOM"
expect_present '^surch\.pub$'              "minisign public key asset"

record "release inventory" OK

# ---------------------------------------------------------------------------
# 2) Download all assets
# ---------------------------------------------------------------------------
hdr "2/6 download assets (gh release download)"
DL_DIR="${WORKDIR}/dist"
mkdir -p "${DL_DIR}"
# --clobber makes this idempotent: re-running the script over an existing
# workdir overwrites stale files instead of failing.
if gh release download "${TAG}" --repo "${REPO}" --dir "${DL_DIR}" --clobber >/dev/null 2>&1; then
  note "downloaded into ${DL_DIR}"
  record "download assets" OK
else
  note "FAIL: gh release download exited non-zero"
  record "download assets" FAIL
fi

# ---------------------------------------------------------------------------
# 3) SHA256SUMS replay
# ---------------------------------------------------------------------------
hdr "3/6 sha256 checksums (SHA256SUMS)"
if [ -f "${DL_DIR}/SHA256SUMS" ]; then
  # sha256sum -c needs the manifest to live next to the files; cd into
  # the download dir. Use --ignore-missing so the script still reports a
  # useful tally if a single asset failed to download.
  CHECK_OUT="${WORKDIR}/sha256sum.out"
  if ( cd "${DL_DIR}" && sha256sum -c --ignore-missing SHA256SUMS ) \
        >"${CHECK_OUT}" 2>&1; then
    OK_COUNT="$(grep -c ': OK$' "${CHECK_OUT}" || true)"
    note "OK: ${OK_COUNT} file(s) match SHA256SUMS"
    record "sha256 checksums" OK
  else
    note "FAIL: at least one checksum mismatched — see ${CHECK_OUT}"
    sed 's/^/      /' "${CHECK_OUT}" || true
    record "sha256 checksums" FAIL
  fi
else
  note "SKIP: no SHA256SUMS in the release"
  record "sha256 checksums" FAIL
fi

# ---------------------------------------------------------------------------
# 4) Minisign signatures on every archive
# ---------------------------------------------------------------------------
hdr "4/6 minisign signatures (binaries + zip archives)"
MS_OK=0; MS_FAIL=0; MS_MISS=0
shopt -s nullglob
for archive in "${DL_DIR}"/*.tar.xz "${DL_DIR}"/*.zip; do
  base="$(basename "${archive}")"
  sig="${archive}.minisig"
  if [ ! -f "${sig}" ]; then
    note "MISS : ${base} (no ${base}.minisig)"
    MS_MISS=$((MS_MISS + 1))
    continue
  fi
  if minisign -Vm "${archive}" -p "${PUBKEY}" >/dev/null 2>&1; then
    note "OK   : ${base}"
    MS_OK=$((MS_OK + 1))
  else
    note "FAIL : ${base} did not verify against $(basename "${PUBKEY}")"
    MS_FAIL=$((MS_FAIL + 1))
  fi
done
shopt -u nullglob

note "summary: ${MS_OK} OK, ${MS_FAIL} FAIL, ${MS_MISS} missing-sig"
if [ "${MS_FAIL}" -eq 0 ] && [ "${MS_MISS}" -eq 0 ] && [ "${MS_OK}" -gt 0 ]; then
  record "minisign signatures" OK
else
  record "minisign signatures" FAIL
fi

# ---------------------------------------------------------------------------
# 5) SBOM sanity (presence + parseable CycloneDX JSON)
# ---------------------------------------------------------------------------
hdr "5/6 SBOM asset (CycloneDX JSON)"
SBOM_FILE="$(ls "${DL_DIR}"/surch-sbom-*.cdx.json 2>/dev/null | head -n 1 || true)"
if [ -z "${SBOM_FILE}" ]; then
  note "FAIL: no surch-sbom-*.cdx.json asset"
  record "SBOM asset (release)" FAIL
else
  if jq -e '
        .bomFormat == "CycloneDX"
        and (.specVersion // "" | test("^1\\."))
        and ((.components // []) | length) > 0
      ' "${SBOM_FILE}" >/dev/null 2>&1; then
    COMP_COUNT="$(jq '(.components // []) | length' "${SBOM_FILE}")"
    note "OK: $(basename "${SBOM_FILE}") (${COMP_COUNT} components)"
    record "SBOM asset (release)" OK
  else
    note "FAIL: $(basename "${SBOM_FILE}") is not a valid CycloneDX BOM"
    record "SBOM asset (release)" FAIL
  fi
fi

# ---------------------------------------------------------------------------
# 6) OCI image: cosign signature + SBOM attestation + bench pullability
# ---------------------------------------------------------------------------
RUNTIME_IMAGE="${IMAGE_BASE}:${VERSION}"
BENCH_IMAGE="${IMAGE_BASE}:bench-${VERSION}"

hdr "6/6 image checks"
note "runtime: ${RUNTIME_IMAGE}"
note "bench  : ${BENCH_IMAGE}"

# 6a) cosign verify (runtime image)
if cosign verify \
      --certificate-identity-regexp "${CERT_IDENTITY_REGEX}" \
      --certificate-oidc-issuer "${CERT_OIDC_ISSUER}" \
      "${RUNTIME_IMAGE}" >/dev/null 2>&1; then
  note "OK    : cosign signature on ${RUNTIME_IMAGE}"
  record "image cosign signature" OK
else
  note "FAIL  : cosign signature on ${RUNTIME_IMAGE}"
  record "image cosign signature" FAIL
fi

# 6b) cosign verify-attestation --type cyclonedx (runtime image)
ATT_OUT="${WORKDIR}/cosign-attest.json"
if cosign verify-attestation \
      --type cyclonedx \
      --certificate-identity-regexp "${CERT_IDENTITY_REGEX}" \
      --certificate-oidc-issuer "${CERT_OIDC_ISSUER}" \
      "${RUNTIME_IMAGE}" >"${ATT_OUT}" 2>/dev/null; then
  note "OK    : cosign CycloneDX attestation on ${RUNTIME_IMAGE}"
  record "image SBOM attestation" OK
else
  note "FAIL  : cosign CycloneDX attestation on ${RUNTIME_IMAGE}"
  record "image SBOM attestation" FAIL
fi

# 6c) bench image: pull check only (workflow does not cosign-sign it).
#     We use `docker manifest inspect` first (cheaper, no layer pull); fall
#     back to `docker pull` if manifest inspect is disabled (`experimental`).
if docker manifest inspect "${BENCH_IMAGE}" >/dev/null 2>&1; then
  note "OK    : bench image manifest reachable"
  record "bench image pullable" OK
elif docker pull --quiet "${BENCH_IMAGE}" >/dev/null 2>&1; then
  note "OK    : bench image pulled"
  record "bench image pullable" OK
else
  note "FAIL  : bench image ${BENCH_IMAGE} not reachable"
  note "        (docker login ghcr.io may be required for private repos)"
  record "bench image pullable" FAIL
fi
note "NOTE  : bench image is intentionally not cosign-signed today"
note "        (built by docker-build.yml, separate workflow)"
record "bench image cosign" SKIP

# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------
if print_verdict; then
  exit 0
else
  exit 1
fi
