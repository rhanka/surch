# Surch

Surch is being rebuilt as a Rust port of OpenSearch plus Lucene behavior.

The previous prototype has been archived under `archive/legacy-prototype/`. The active workspace is intentionally blank and will grow through upstream-traceable parity tickets.

## Current Status

- Planning baseline: `PLAN.md`
- Integral portage spec: `spec/SPEC_INTEGRAL_OPENSEARCH_LUCENE_PORTAGE.md`
- Autonomous execution plan: `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
- Upstream references and graphify reports: `docs/portage/`

Compatibility is not claimed until golden tests prove it against the pinned OpenSearch and Lucene references.

## Workspace

```text
crates/
  surch-types/
  surch-analysis/
  surch-codec/
  surch-store/
  surch-index/
  surch-search/
  surch-api/
```

## Development

```bash
cargo test --workspace
cargo fmt --all
cargo clippy --workspace --all-targets --all-features
```

## Portage Rule

Every feature starts from an upstream reference and a golden parity test:

- upstream repository, commit, file, class, method or REST spec
- owner subagent
- allowed and forbidden paths
- failing Surch test before implementation
- passing oracle against upstream or recorded fixture
- verification gates

## Verifying releases

The Surch OCI image published to `ghcr.io/rhanka/surch` is signed via
[cosign](https://github.com/sigstore/cosign) keyless OIDC from the
`release.yml` GitHub Actions workflow. No static keys are involved; the
signature is anchored to the image digest in the Sigstore transparency log.

Verify a published tag with cosign 2.x:

```bash
cosign verify ghcr.io/rhanka/surch:<tag> \
  --certificate-identity-regexp 'https://github.com/rhanka/surch/.github/workflows/release\.yml@.*' \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com
```

`<tag>` can be any tag produced by the release workflow (semver
`X.Y.Z`, short `X.Y`, major `X`, or `sha-<short>`). All tags resolve to
the same signed manifest digest.

## License

Apache-2.0
