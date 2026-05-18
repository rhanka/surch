# Surch

Surch is being rebuilt as a Rust port of OpenSearch plus Lucene behavior.

The previous prototype has been archived under `archive/legacy-prototype/`. The active workspace is intentionally blank and will grow through upstream-traceable parity tickets.

## Current Status

- Agentic governance: `AGENTS.md`
- Planning baseline: `PLAN.md`
- Branch/lane plans: `plan/*.md`
- Historical portage execution plan:
  `plan/00_AUTONOMOUS_PORTAGE_EXECUTION.md`
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

### Static binaries (minisign)

The static binaries produced by `cargo-dist` (linux gnu/musl x86_64 +
aarch64, darwin x86_64 + aarch64) are signed with
[minisign](https://jedisct1.github.io/minisign/). Each archive is
published next to its `.minisig` signature on the GitHub release page.
The public verification key lives at the repo root in `surch.pub` and
is also attached to every release as an asset.

Verify a downloaded archive:

```bash
# Extract the archive you want to verify, then:
minisign -Vm surch-api-<ver>-<triple>.tar.xz -p surch.pub
```

`minisign -Vm` exits 0 only if the signature is valid for the file
content and was produced by the holder of the matching private key
(stored as a GitHub Actions secret, never present in this repository).

## License

Apache-2.0
