#!/usr/bin/env bash
# Verify a published Surch image: cosign signature + CycloneDX SBOM
# attestation. Prints the top 5 dependencies of the SBOM by package size.
#
# Usage:
#   scripts/verify-release.sh ghcr.io/rhanka/surch:0.1.0
#
# Requires: cosign (>= v2.4), jq.

set -euo pipefail

IMAGE="${1:-}"
if [ -z "${IMAGE}" ]; then
  echo "usage: $0 <image-ref>" >&2
  echo "example: $0 ghcr.io/rhanka/surch:0.1.0" >&2
  exit 2
fi

# Identity regex of the keyless-signing workflow. Override via env if the
# repo is forked under a different org.
CERT_IDENTITY_REGEX="${COSIGN_CERT_IDENTITY_REGEX:-https://github.com/rhanka/surch/.github/workflows/release\\.yml@.*}"
CERT_OIDC_ISSUER="${COSIGN_CERT_OIDC_ISSUER:-https://token.actions.githubusercontent.com}"

for bin in cosign jq; do
  if ! command -v "${bin}" >/dev/null 2>&1; then
    echo "error: ${bin} not found in PATH" >&2
    exit 3
  fi
done

echo "==> image: ${IMAGE}"

# ---------------------------------------------------------------------------
# 1) Signature
# ---------------------------------------------------------------------------
echo "==> cosign verify (signature)"
SIG_OK=0
if cosign verify \
    --certificate-identity-regexp "${CERT_IDENTITY_REGEX}" \
    --certificate-oidc-issuer "${CERT_OIDC_ISSUER}" \
    "${IMAGE}" >/dev/null 2>&1; then
  echo "    signature: OK"
  SIG_OK=1
else
  echo "    signature: FAIL"
fi

# ---------------------------------------------------------------------------
# 2) SBOM attestation
# ---------------------------------------------------------------------------
echo "==> cosign verify-attestation (CycloneDX SBOM)"
ATT_JSON="$(mktemp)"
trap 'rm -f "${ATT_JSON}"' EXIT

SBOM_OK=0
if cosign verify-attestation \
    --type cyclonedx \
    --certificate-identity-regexp "${CERT_IDENTITY_REGEX}" \
    --certificate-oidc-issuer "${CERT_OIDC_ISSUER}" \
    "${IMAGE}" >"${ATT_JSON}" 2>/dev/null; then
  echo "    SBOM attestation: OK"
  SBOM_OK=1
else
  echo "    SBOM attestation: FAIL"
fi

# ---------------------------------------------------------------------------
# 3) Top 5 dependencies by size (from the decoded SBOM)
# ---------------------------------------------------------------------------
if [ "${SBOM_OK}" -eq 1 ]; then
  echo "==> top 5 dependencies (by encoded predicate size)"
  # verify-attestation emits one or more DSSE envelopes (JSON Lines). Each
  # envelope's .payload is base64-encoded; the inner predicate is the
  # CycloneDX document. We sort components by the byte length of their
  # JSON encoding as a stable, dependency-free proxy for "size".
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
  ' "${ATT_JSON}" || echo "    (could not parse SBOM payload)"
fi

# ---------------------------------------------------------------------------
# Verdict
# ---------------------------------------------------------------------------
echo "==> verdict"
echo "    signature       : $( [ "${SIG_OK}"  -eq 1 ] && echo OK || echo FAIL )"
echo "    SBOM attestation: $( [ "${SBOM_OK}" -eq 1 ] && echo OK || echo FAIL )"

if [ "${SIG_OK}" -eq 1 ] && [ "${SBOM_OK}" -eq 1 ]; then
  exit 0
fi
exit 1
