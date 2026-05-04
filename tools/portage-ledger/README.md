# Portage Ledger

The portage ledger tracks function-level OpenSearch and Lucene parity tickets.

Tickets live under `docs/portage/ledger/tickets/` as JSON files.

Each ticket must include:

- upstream repository, commit, file, class, and method or REST spec
- owner subagent
- dependencies
- allowed paths and forbidden paths
- golden tests
- verification gates

Validate the ledger:

```bash
cargo run -p portage-ledger -- validate docs/portage/ledger/tickets
```

Check the repository language policy:

```bash
cargo run -p portage-ledger -- language-policy .
```
