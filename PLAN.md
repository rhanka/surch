# Surch - Rust OpenSearch/Lucene Clone

## Vision

**Surch** est un moteur de recherche 100% Rust reproduisant les fonctionnalités core d'indexation et de recherche d'OpenSearch/Elasticsearch, sans le module analytique (mais avec les fondations). L'objectif MVP est d'atteindre une compatibilité fonctionnelle avec l'API REST d'OpenSearch pour les opérations d'indexation et de search, avec l'algorithme Damerau-Levenshtein (distance ≤ 2) comme signature distinctive de Lucène.

---

## 1. Features & Roadmap

### Phase 1: Foundations (Jour 1)

| Feature | Description | Priorité |
|---------|-------------|----------|
| **F1.1** | Architecture core - Storage layer (segment-based, Write-Ahead Log) | Critical |
| **F1.2** | Inverted index implementation | Critical |
| **F1.3** | Document DSL (JSON parsing, mapping) | Critical |
| **F1.4** | HTTP server (REST API compatible OpenSearch) | Critical |
| **F1.5** | Basic authentication & authorization | High |

### Phase 2: Indexation (Jour 2)

| Feature | Description | Priorité |
|---------|-------------|----------|
| **F2.1** | Index creation/deletion API | Critical |
| **F2.2** | Document indexing (single/bulk) | Critical |
| **F2.3** | Field types: text, keyword, integer, long, float, double, boolean, date | Critical |
| **F2.4** | Analyzer pipeline (standard, simple, whitespace, stop, keyword) | Critical |
| **F2.5** | Segment merging (basic) | High |
| **F2.6** | Refresh & Flush API | High |

### Phase 3: Recherche (Jour 3)

| Feature | Description | Priorité |
|---------|-------------|----------|
| **F3.1** | Query DSL (match, term, range, bool, exists, missing) | Critical |
| **F3.2** | Full-text search (match, match_phrase, multi_match) | Critical |
| **F3.3** | Sorting & Pagination | Critical |
| **F3.4** | Aggregations foundation (terms, avg, sum, min, max) | Medium |
| **F3.5** | Search templates | Medium |
| **F3.6** | Highlighting | Medium |

### Phase 4: Lucene Signature (Jour 4 - FINAL)

| Feature | Description | Priorité |
|---------|-------------|----------|
| **F4.1** | Fuzzy search (Damerau-Levenshtein distance ≤ 2) | Critical |
| **F4.2** | Prefix queries | Critical |
| **F4.3** | Wildcard & Regexp queries | Critical |
| **F4.4** | Suggesters (Term, Phrase, Completion) | High |
| **F4.5** | Percolate (match reverse) | Medium |

### Post-MVP (Future)

- Index aliases
- Index templates
- Reindex API
- Snapshot/Restore
- Cross-cluster replication
- Plugin system

---

## 2. Stratégie Branches

### Branch Strategy: Gitflow Adapté

```
main (protected)
    │
    ├── develop (integration)
    │   │
    │   ├── feature/F1.1-storage-layer
    │   ├── feature/F1.2-inverted-index
    │   ├── feature/F2.x-indexation
    │   ├── feature/F3.x-search
    │   └── feature/F4.x-lucene-signature
    │
    ├── bugfix/*
    ├── hotfix/*
    └── release/v0.1.0-mvp
```

### Règles

- **main**: production-ready, tagué sémantiquement
- **develop**: integration continue,tests d'intégration
- **feature/F#.#-description**: une feature par branche, PR vers develop
- **bugfix/#-description**: fix vers develop
- **hotfix/#-description**: fix urgent vers main + merge develop
- **release/vX.Y.Z**: freeze API, release candidate

### Convention Commits

```
<type>(<scope>): <description>

Types: feat, fix, refactor, test, docs, chore, perf, security, api
Scopes: storage, indexer, search, api, auth, analyzer, fuzzy, aggregation
```

---

## 3. Directives Développement Agentique

### Rôle: Conductor (ce document)

Le Conductor orchestre l'ensemble du projet et gère les interactions avec les subagents.

**Responsabilités:**
1. Maintenir la vision produit et la roadmap
2. Allouer les tâches aux subagents
3. Valider les livrables avant merge
4. Gérer les feedback loops
5. Assurer la cohérence architecturale
6. Décider des compromis techniques

### Sous-Agents

| Agent | Rôle | Numéro |
|-------|------|--------|
| **StorageEngine** | Persistence, WAL, segments, index files | #1 |
| **Indexer** | Analyse, tokenization, indexing pipeline | #2 |
| **SearchEngine** | Query parsing, execution, scoring | #3 |
| **APIServer** | HTTP REST, OpenSearch compatibility | #4 |

### Directive pour Subagents

```
=== DEBUT DIRECTIVE ===
Tu es [AGENT_NAME], un subagent de Surch.
Tu as pour mission de développer [FEATURE_DESCRIPTION].

## Contexte
- Surch: Moteur de recherche 100% Rust (clone OpenSearch/Lucene)
- MVP: Indexation + Search avec Damerau-Levenshtein (distance ≤ 2)
- Compatibilité: API REST OpenSearch/Elasticsearch
- Sécurité: Zero-trust, input validation stricte

## Contraintes
1.Tout code DOIT être en Rust (edition 2021+)
2.Respecter les standards de code du projet (voir CODE_STYLE.md)
3.Tests unitaires avec >80% coverage
4.Tests d'intégration pour chaque feature
5.Pas de dépendances unsafe sauf justification documentée
6.Privilégier les crates maintenues et sécurisées

## Livrables Attendus
- Code source dans src/[domain]/
- Tests unitaires dans tests/unit/
- Tests d'intégration dans tests/integration/
- Documentation API dans docs/
- Mise à jour du CHANGELOG.md

## Processus
1.Lire la spec OpenSearch/Elasticsearch correspondante
2.Implémenter la feature selon le plan
3.Écrire les tests
4.Run: cargo test + cargo clippy + cargo fmt
5.Créer PR vers develop avec description détaillée

=== FIN DIRECTIVE ===
```

### Communication Flow

```
User → Conductor → SubAgent(s) → Review → Conductor → Merge
                 ↑              ↓
                 ←←←←←←← Feedback Loop ←←←←←←
```

### Feedback Loop Types

1. **Code Review**: Le subagent soumet une PR, le Conductor review
2. **Integration Test Fail**: Les tests d'intégration révèlent un problème inter-modules
3. **Spec Change**: Une nouvelle version d'OpenSearch modifie le comportement attendu
4. **Security Issue**: Une vulnérabilité est découverte

---

## 4. Architecture Technique

### Stack Technique

| Composant | Choice | Version |
|-----------|--------|---------|
| Language | Rust | 1.75+ |
| Async Runtime | Tokio | 1.x |
| HTTP Server | Axum | 0.7.x |
| Serialization | Serde + serde_json | 1.x |
| Logging | Tracing | 0.1.x |
| Testing | Tokio + Assertions | - |
| Fuzzy Logic | Custom (Damerau-Levenshtein) | - |

### Module Structure

```
src/
├── main.rs                 # Entry point
├── lib.rs                  # Library root
├── config.rs               # Configuration
├── error.rs                # Error types
│
├── api/                    # REST API Layer
│   ├── mod.rs
│   ├── routes/
│   │   ├── mod.rs
│   │   ├── index.rs
│   │   ├── document.rs
│   │   └── search.rs
│   └── middleware/
│       ├── mod.rs
│       ├── auth.rs
│       └── tracing.rs
│
├── storage/               # Storage Engine
│   ├── mod.rs
│   ├── wal.rs            # Write-Ahead Log
│   ├── segment.rs        # Segment management
│   ├── index_reader.rs   # Index reader
│   └── index_writer.rs   # Index writer
│
├── indexer/               # Indexation Pipeline
│   ├── mod.rs
│   ├── document.rs       # Document handling
│   ├── mapping.rs        # Index mapping
│   ├── analyzer/
│   │   ├── mod.rs
│   │   ├── standard.rs
│   │   ├── simple.rs
│   │   └── stop.rs
│   └── pipeline.rs       # Indexing pipeline
│
├── search/                # Search Engine
│   ├── mod.rs
│   ├── query/
│   │   ├── mod.rs
│   │   ├── match.rs
│   │   ├── term.rs
│   │   ├── range.rs
│   │   ├── bool.rs
│   │   └── fuzzy.rs      # Damerau-Levenshtein
│   ├── scorer.rs          # TF-IDF, BM25
│   ├── collector.rs       # Result collection
│   └── aggregator.rs      # Aggregations foundation
│
└── common/                # Shared utilities
    ├── mod.rs
    ├── document.rs
    ├── field.rs
    └── types.rs
```

### API Compatibility Layer

OpenSearch/Elasticsearch compatibility endpoint structure:

```
# Index Management
PUT /{index}
DELETE /{index}
GET /{index}/_mapping

# Document Operations
POST /{index}/_doc/{id}
PUT /{index}/_doc/{id}
GET /{index}/_doc/{id}
DELETE /{index}/_doc/{id}
POST /{index}/_bulk

# Search
POST /{index}/_search
GET /{index}/_search

# Refresh/Flush
POST /{index}/_refresh
POST /{index}/_flush
```

---

## 5. Tests Strategy

### Unit Tests

- **Coverage Target**: 80% minimum
- **Naming**: `#[cfg(test)] mod tests { ... }` dans chaque crate
- **Tools**: `cargo test`, `cargo tarpaulin` pour coverage

### Integration Tests

- **Location**: `tests/integration/`
- **Framework**: Rust built-in + custom test harness
- **API Tests**: Exhaustive sur les endpoints REST

### Test Categories

| Category | Location | Run |
|----------|----------|-----|
| Unit | src/**/tests | On PR |
| Integration | tests/integration/ | On PR + nightly |
| Benchmark | benches/ | Manual |
| Fuzz | fuzz/ | CI weekly |

---

## 6. Sécurité

### Principes

1. **Zero Trust**: Toute entrée est non-fiable
2. **Input Validation**: Validate all inputs at API boundary
3. **Sandboxing**: Limiter les privilèges du processus
4. **Audit**: Logging de toutes les opérations sensibles

### Points de Contrôle

- [ ] Authentication JWT
- [ ] Authorization (RBAC basique)
- [ ] Input sanitization (injection attacks)
- [ ] Rate limiting
- [ ] TLS support
- [ ] Secrets management

---

## 7. OpenSearch Spec References

### Prioritaire (pour MVP)

1. [Search API](https://opensearch.org/docs/latest/api-reference/search/)
2. [Document API](https://opensearch.org/docs/latest/api-reference/document-api/)
3. [Index API](https://opensearch.org/docs/latest/api-reference/index-api/)
4. [Query DSL](https://opensearch.org/docs/latest/query-dsl/)
5. [Aggregations](https://opensearch.org/docs/latest/aggregations/)
6. [Fuzzy Query](https://opensearch.org/docs/latest/query-dsl/term/fuzzy/)

### Specification OpenSearch Sources

- Repo: https://github.com/opensearch-project/OpenSearch
- Specs: https://opensearch.org/docs/latest/

---

## 8. Définitions Done/MVP

### Done Criteria

- [ ] Code compile sans warning (`cargo clippy`)
- [ ] Code formaté (`cargo fmt`)
- [ ] Tests unitaires passent (>80% coverage)
- [ ] Tests d'intégration passent
- [ ] Documentation API complète
- [ ] CHANGELOG mis à jour

### MVP Success Metrics

1. **Indexation**: Peut indexer 10k documents/sec sur laptop standard
2. **Search**: Latence <50ms pour queries simples
3. **Fuzzy**: Damerau-Levenshtein distance ≤ 2 fonctionnel
4. **API Compatible**: Requêtes OpenSearch retournent les mêmes résultats

---

## 9. Timeline Jour par Jour

### Jour 1: Foundations
- [x] Repo setup, Cargo.toml, workspace
- [ ] Storage layer (WAL + segments)
- [ ] HTTP server basic
- [ ] Document DSL

### Jour 2: Indexation
- [ ] Index CRUD API
- [ ] Bulk indexing
- [ ] Analyzers
- [ ] Field types

### Jour 3: Recherche
- [ ] Query DSL
- [ ] Full-text search
- [ ] Sorting/Pagination
- [ ] Aggregations foundation

### Jour 4: Lucene Signature
- [ ] Fuzzy search (Damerau-Levenshtein)
- [ ] Wildcard/Regex
- [ ] Suggesters
- [ ] Final integration tests
- [ ] MVP release

---

*Document généré par Conductor - Version 0.1.0*
