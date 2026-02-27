# Directives de Développement Agentique - Surch

## Rôle du Conductor

Le Conductor (ce document) orchestre l'ensemble du projet Surch. Responsabilités principales:

1. **Planification**: Maintenir la roadmap et allouer les tâches aux subagents
2. **Feedback Loop**: Valider les livrables, gérer les aller-retour avec les subagents
3. **Architecture**: Assurer la cohérence technique entre les modules
4. **Qualité**: Valider tests, reviews, et critères de done
5. **Sécurité**: Point final sur tous les aspects security

## Structure des Subagents

| ID | Nom | Domain | Status |
|----|-----|--------|--------|
| #1 | **StorageEngine** | Persistence, WAL, segments | AVAILABLE |
| #2 | **Indexer** | Analyse, tokenization, indexing | AVAILABLE |
| #3 | **SearchEngine** | Query, execution, scoring, fuzzy | AVAILABLE |
| #4 | **APIServer** | REST API, OpenSearch compatibility | AVAILABLE |

## Directive Standard pour Subagent

```
=== DEBUT DIRECTIVE ===
Tu es [AGENT_NAME], subagent #N de Surch.

## Mission
[FEATURE_DESCRIPTION]

## Contexte Projet
- **Nom**: Surch - Moteur de recherche 100% Rust
- **Objectif**: Clone OpenSearch/Lucene (indexation + search, sans analytique)
- **Key Feature**: Damerau-Levenshtein distance ≤ 2
- **Compatibilité**: API REST OpenSearch/Elasticsearch

## Contraintes Obligatoires
1. Langage: Rust 1.75+ (edition 2021)
2. Style: Voir CODE_STYLE.md
3. Tests: >80% coverage unit, tests intégration
4. Sécurité: Zero-trust, input validation stricte
5. Pas de unsafe sauf dérogation documentée
6. Dépendances: Crates maintenues et sécurisées uniquement

## Livrables
- Code: src/[domain]/
- Tests unitaires: src/[domain]/tests/
- Tests intégration: tests/integration/
- Docs: docs/
- Update: CHANGELOG.md

## Processus
1. Lire spec OpenSearch correspondante
2. Implémenter feature
3. Écrire tests
4. Run: cargo test && cargo clippy && cargo fmt
5. Soumettre PR vers develop

=== FIN DIRECTIVE ===
```

## Règles de Communication

### Initiation de Task

```
Conductor → SubAgent: Task(agent_id, feature_id, description, priority)
```

### Feedback Loop Types

1. **CODE_REVIEW**: "Code Review - <feature>: Points à corriger"
2. **INTEGRATION_FAIL**: "Échec test intégration - reason: <détail>"
3. **SPEC_MISMATCH**: "Comportement différent de spec - expected: <X>, got: <Y>"
4. **SECURITY_ALERT**: "Vulnérabilité potentielle - <type>: <détail>"
5. **BLOCKED**: "Dépendance non résolue - <module> a besoin de <feature>"

### Format Retour Subagent

```
## Task: <feature_id> - <status>

### Résumé
<Bref résumé du travail>

### Changes
- <fichier>: <description>

### Tests
- Unit: <pass/fail>
- Integration: <pass/fail>

### Blocker?
<Oui/Non + reason si oui>

### Next Steps
<Prochaine action>
```

## Critères de Done par Phase

### Phase 1 (Foundations) - StorageEngine
- [ ] WAL fonctionnel (append, flush)
- [ ] Segment management (create, read, merge)
- [ ] Index file format stable

### Phase 2 (Indexation) - Indexer
- [ ] Index CRUD
- [ ] Document bulk indexing
- [ ] Analyzers pipeline
- [ ] Field type handling

### Phase 3 (Recherche) - SearchEngine
- [ ] Query DSL complet
- [ ] TF-IDF/BM25 scoring
- [ ] Pagination/sorting

### Phase 4 (Lucene Signature) - SearchEngine + StorageEngine
- [ ] Damerau-Levenshtein distance ≤ 2
- [ ] Fuzzy query type
- [ ] Suggesters

## Priorités d'Allocation

### Ordre de priorité pour les subagents:
1. **Critical**: Blocker le MVP
2. **High**: Feature principale
3. **Medium**: Feature secondaire
4. **Low**: Nice-to-have

## Commit Convention

```
<type>(<scope>): <description>

Types: feat, fix, refactor, test, docs, chore, perf, security, api
Scopes: storage, indexer, search, api, auth, analyzer, fuzzy, aggregation

Exemples:
feat(fuzzy): add damerau levenshtein algorithm
fix(storage): wal rotation issue
api(search): add match_phrase query support
security(auth): jwt validation hardening
```

## Security Checklist

Tout code soumis DOIT:
- [ ] Valider toutes les entrées (API boundary)
- [ ] Échapper les sorties (si applicable)
- [ ] Logger les opérations sensibles
- [ ] Ne pas exposer de secrets dans les logs
- [ ] Passer cargo clippy --security-focused

## Notes

- Maximum 4 subagents parallèles
- Un subagent = une feature à la fois
- Si bloqué >30min, escalader vers Conductor
- Toutes PRs passent par develop (jamais direct sur main)
