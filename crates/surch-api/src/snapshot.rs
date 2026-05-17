//! Raw-index snapshot export / import.
//!
//! **Deprecated since 0.2.0** — these `/_surch/snapshot/{export,import}`
//! routes remain available for the matchID `deces-backend` boot path
//! that already targets them, but the canonical ES-parity surface
//! lives under `/_snapshot/{repository}/{snapshot}` (see
//! `crate::snapshot_es` and `docs/ops/snapshot-es-api.md`). The
//! `_surch/*` shape will be removed in `0.4.0` after one matchID
//! release cycle.
//!
//! Surch is single-node and in-memory today, so the "raw index" is not
//! a directory of FST + postings files but the logical content that
//! lets a fresh node reproduce the same searches bit-for-bit: the
//! mapping, the settings, the alias definitions, and the source
//! documents. That is exactly the shape matchID's `deces-backend`
//! already ships between hosts (tarball + boot-time re-ingest), so we
//! expose it under a single pair of routes:
//!
//! ```text
//! POST /_surch/snapshot/export?index=<name>   → tar.gz body
//! POST /_surch/snapshot/import?index=<name>   ← tar.gz body
//! ```
//!
//! Intentionally simpler than the ES `_snapshot` / SLM surface — no
//! repository registration, no scheduling. The packaging plan tracks
//! the full S3 / cron variant separately; this module is the
//! "single-tarball" half of that plan, scoped to the matchID use
//! case (cf. `docs/wp-d-matchid/SPEC.md` — "Out-of-scope reminders").
//!
//! Format (`surch_snapshot_format_version: 1`):
//!
//! ```text
//! manifest.json          { format_version: 1, surch_version, index, doc_count }
//! mapping.json           IndexMapping properties object
//! settings.json          Surch settings
//! aliases.json           BTreeMap<alias_name, alias_definition>
//! documents.ndjson       one JSON object per line: { "_id": str, "_source": value }
//! ```
//!
//! All paths inside the archive are flat (no directories). Import
//! rejects every entry whose path is not in this whitelist; this is
//! the defense against path-traversal entries (`../../etc/passwd`)
//! that the tar format leaves to the consumer.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use axum::{
    body::{Body, Bytes},
    extract::{Query, State},
    http::{header, StatusCode},
    response::IntoResponse,
};
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use surch_index::mapping::IndexMapping;
use tar::{Builder, Header};

use crate::{
    index::validate_index_name,
    state::{AppState, StoredDocument},
    OpenSearchError,
};

/// Bump on any breaking format change. `import` rejects unknown versions.
pub const SNAPSHOT_FORMAT_VERSION: u32 = 1;

const MANIFEST_ENTRY: &str = "manifest.json";
const MAPPING_ENTRY: &str = "mapping.json";
const SETTINGS_ENTRY: &str = "settings.json";
const ALIASES_ENTRY: &str = "aliases.json";
const DOCUMENTS_ENTRY: &str = "documents.ndjson";

const ALLOWED_ENTRIES: &[&str] = &[
    MANIFEST_ENTRY,
    MAPPING_ENTRY,
    SETTINGS_ENTRY,
    ALIASES_ENTRY,
    DOCUMENTS_ENTRY,
];

/// 1 GiB cap on incoming tarball size. Surch is single-node in-memory;
/// anything past this is almost certainly an accident, not a real
/// matchID corpus (the INSEE 25M-record bulk is ~3 GiB raw NDJSON but
/// matchID ships it through `_bulk`, not `snapshot/import`).
const IMPORT_BODY_CAP_BYTES: usize = 1024 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct IndexParam {
    pub index: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct Manifest {
    format_version: u32,
    surch_version: String,
    index: String,
    doc_count: u64,
}

/// Build a single-index tarball for the ES-parity snapshot path.
///
/// Surch is in-memory: the on-repository "shard payload" promised by
/// the Elasticsearch `BlobStoreRepository` layout is a logical
/// equivalent — the same `build_tarball` output the legacy
/// `_surch/snapshot/export` route returns, packaged inside the
/// ES-style `__snap-{uuid}.dat` blob. This wrapper keeps the byte
/// shape identical so an operator can `scp` a single payload out of
/// the repository and feed it to the legacy `_surch/snapshot/import`
/// route without translation.
pub fn build_index_tarball_for_es_snapshot(
    index: &str,
    metadata: &crate::state::IndexMetadata,
    documents: &[StoredDocument],
) -> Result<Vec<u8>, String> {
    build_tarball(
        index,
        &metadata.mapping,
        &metadata.settings,
        &metadata.aliases,
        documents,
    )
}

/// Restore a single index from the tarball produced by
/// [`build_index_tarball_for_es_snapshot`].
///
/// Refuses to overwrite an existing index (same contract as the
/// legacy `_surch/snapshot/import` route — the caller deletes first
/// if they want to overwrite, matching ES `restore` defaults).
pub fn restore_index_tarball_for_es_snapshot(
    state: &AppState,
    index: &str,
    body: &[u8],
) -> Result<u64, String> {
    if state.index_exists(index) {
        return Err(format!("index [{index}] already exists"));
    }
    let parsed = parse_tarball(body)?;
    if parsed.manifest.format_version != SNAPSHOT_FORMAT_VERSION {
        return Err(format!(
            "unsupported snapshot format_version {} (expected {})",
            parsed.manifest.format_version, SNAPSHOT_FORMAT_VERSION
        ));
    }
    state.create_index(
        index,
        Some(parsed.mapping),
        parsed.settings,
        parsed.aliases,
    );
    for (id, source) in parsed.documents {
        state.index_document(index, &id, source);
    }
    Ok(parsed.manifest.doc_count)
}

/// `POST /_surch/snapshot/export?index=<name>` → tar.gz body.
///
/// Deprecated: use `PUT /_snapshot/<repo>/<snap>` instead. Will be
/// removed in 0.4.0.
///
/// Returns 404 when the index is unknown, 400 when the name is
/// invalid, 200 + `application/gzip` otherwise.
pub async fn export_handler(
    State(state): State<AppState>,
    Query(params): Query<IndexParam>,
) -> impl IntoResponse {
    let index = params.index;
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    if !state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{index}] missing"),
        )
        .into_response();
    }

    let metadata = match state.index_metadata(&index) {
        Some(metadata) => metadata,
        None => {
            return OpenSearchError::new(
                StatusCode::NOT_FOUND,
                "index_not_found_exception",
                format!("index [{index}] missing"),
            )
            .into_response();
        }
    };
    let documents = state.documents(&index);

    let bytes = match build_tarball(
        &index,
        &metadata.mapping,
        &metadata.settings,
        &metadata.aliases,
        &documents,
    ) {
        Ok(bytes) => bytes,
        Err(reason) => {
            return OpenSearchError::new(
                StatusCode::INTERNAL_SERVER_ERROR,
                "snapshot_export_exception",
                reason,
            )
            .into_response();
        }
    };

    axum::http::Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, "application/gzip")
        .header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"surch-snapshot-{index}.tar.gz\""),
        )
        .header("x-surch-snapshot-format-version", SNAPSHOT_FORMAT_VERSION)
        .body(Body::from(bytes))
        .expect("snapshot export response should build")
        .into_response()
}

/// `POST /_surch/snapshot/import?index=<name>` ← tar.gz body.
///
/// Deprecated: use `POST /_snapshot/<repo>/<snap>/_restore` instead.
/// Will be removed in 0.4.0.
///
/// `index` becomes the new physical index name (free choice, lets the
/// caller restore the same snapshot under a different alias without
/// editing the archive). Refuses to overwrite an existing index — the
/// caller deletes first if that is what they want.
pub async fn import_handler(
    State(state): State<AppState>,
    Query(params): Query<IndexParam>,
    body: Bytes,
) -> impl IntoResponse {
    let index = params.index;
    if let Err(error) = validate_index_name(&index) {
        return error.into_response();
    }
    if state.index_exists(&index) {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "resource_already_exists_exception",
            format!("index [{index}] already exists"),
        )
        .into_response();
    }
    if body.len() > IMPORT_BODY_CAP_BYTES {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "snapshot_import_exception",
            format!(
                "snapshot body exceeds the {IMPORT_BODY_CAP_BYTES}-byte cap (got {} bytes)",
                body.len()
            ),
        )
        .into_response();
    }

    let parsed = match parse_tarball(&body) {
        Ok(parsed) => parsed,
        Err(reason) => {
            return OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "snapshot_import_exception",
                reason,
            )
            .into_response();
        }
    };

    if parsed.manifest.format_version != SNAPSHOT_FORMAT_VERSION {
        return OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "snapshot_import_exception",
            format!(
                "unsupported snapshot format_version {} (expected {})",
                parsed.manifest.format_version, SNAPSHOT_FORMAT_VERSION
            ),
        )
        .into_response();
    }

    state.create_index(
        &index,
        Some(parsed.mapping),
        parsed.settings,
        parsed.aliases,
    );
    for (id, source) in parsed.documents {
        state.index_document(&index, &id, source);
    }

    let response_body = json!({
        "acknowledged": true,
        "index": index,
        "documents": parsed.manifest.doc_count,
        "format_version": parsed.manifest.format_version,
        "source_index": parsed.manifest.index,
    });
    (StatusCode::OK, axum::Json(response_body)).into_response()
}

fn build_tarball(
    index: &str,
    mapping: &Value,
    settings: &Value,
    aliases: &BTreeMap<String, Value>,
    documents: &[StoredDocument],
) -> Result<Vec<u8>, String> {
    let manifest = Manifest {
        format_version: SNAPSHOT_FORMAT_VERSION,
        surch_version: env!("CARGO_PKG_VERSION").to_owned(),
        index: index.to_owned(),
        doc_count: documents.len() as u64,
    };
    let manifest_bytes =
        serde_json::to_vec_pretty(&manifest).map_err(|e| format!("manifest serialisation: {e}"))?;
    let mapping_bytes =
        serde_json::to_vec_pretty(mapping).map_err(|e| format!("mapping serialisation: {e}"))?;
    let settings_bytes =
        serde_json::to_vec_pretty(settings).map_err(|e| format!("settings serialisation: {e}"))?;
    let aliases_value = aliases_to_value(aliases);
    let aliases_bytes = serde_json::to_vec_pretty(&aliases_value)
        .map_err(|e| format!("aliases serialisation: {e}"))?;

    let mut documents_bytes = Vec::new();
    for doc in documents {
        let entry = json!({ "_id": doc.id, "_source": doc.source.as_ref() });
        let line =
            serde_json::to_vec(&entry).map_err(|e| format!("document serialisation: {e}"))?;
        documents_bytes.extend_from_slice(&line);
        documents_bytes.push(b'\n');
    }

    let buf = Vec::with_capacity(
        manifest_bytes.len()
            + mapping_bytes.len()
            + settings_bytes.len()
            + aliases_bytes.len()
            + documents_bytes.len()
            + 4096,
    );
    let gz = GzEncoder::new(buf, Compression::default());
    let mut tar = Builder::new(gz);

    append_entry(&mut tar, MANIFEST_ENTRY, &manifest_bytes)?;
    append_entry(&mut tar, MAPPING_ENTRY, &mapping_bytes)?;
    append_entry(&mut tar, SETTINGS_ENTRY, &settings_bytes)?;
    append_entry(&mut tar, ALIASES_ENTRY, &aliases_bytes)?;
    append_entry(&mut tar, DOCUMENTS_ENTRY, &documents_bytes)?;

    let gz = tar
        .into_inner()
        .map_err(|e| format!("tar finalisation: {e}"))?;
    let bytes = gz.finish().map_err(|e| format!("gzip finalisation: {e}"))?;
    Ok(bytes)
}

fn append_entry<W: Write>(tar: &mut Builder<W>, name: &str, data: &[u8]) -> Result<(), String> {
    let mut header = Header::new_gnu();
    header
        .set_path(name)
        .map_err(|e| format!("tar header path `{name}`: {e}"))?;
    header.set_size(data.len() as u64);
    header.set_mode(0o644);
    header.set_cksum();
    tar.append(&header, data)
        .map_err(|e| format!("tar append `{name}`: {e}"))?;
    Ok(())
}

struct ParsedSnapshot {
    manifest: Manifest,
    mapping: IndexMapping,
    settings: Value,
    aliases: BTreeMap<String, Value>,
    documents: Vec<(String, Value)>,
}

fn parse_tarball(body: &[u8]) -> Result<ParsedSnapshot, String> {
    let gz = GzDecoder::new(Cursor::new(body));
    let mut archive = tar::Archive::new(gz);
    let entries = archive.entries().map_err(|e| format!("tar entries: {e}"))?;

    let mut manifest: Option<Manifest> = None;
    let mut mapping_raw: Option<Value> = None;
    let mut settings_raw: Option<Value> = None;
    let mut aliases_raw: Option<Value> = None;
    let mut documents_ndjson: Option<Vec<u8>> = None;

    for entry in entries {
        let mut entry = entry.map_err(|e| format!("tar entry: {e}"))?;
        let path = entry
            .path()
            .map_err(|e| format!("tar entry path: {e}"))?
            .into_owned();
        let name = path
            .to_str()
            .ok_or_else(|| format!("tar entry path is not UTF-8: {}", path.display()))?
            .to_owned();
        // Path-traversal hardening: tar entries are flat, no `..`, no
        // absolute paths, no nested directories. Any other layout is
        // either a hostile archive or a format we don't grok.
        if !ALLOWED_ENTRIES.contains(&name.as_str()) {
            return Err(format!(
                "unexpected tar entry `{name}` (allowed: {})",
                ALLOWED_ENTRIES.join(", ")
            ));
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("tar entry read `{name}`: {e}"))?;
        match name.as_str() {
            MANIFEST_ENTRY => {
                manifest = Some(
                    serde_json::from_slice(&buf)
                        .map_err(|e| format!("manifest.json parse: {e}"))?,
                );
            }
            MAPPING_ENTRY => {
                mapping_raw = Some(
                    serde_json::from_slice(&buf).map_err(|e| format!("mapping.json parse: {e}"))?,
                );
            }
            SETTINGS_ENTRY => {
                settings_raw = Some(
                    serde_json::from_slice(&buf)
                        .map_err(|e| format!("settings.json parse: {e}"))?,
                );
            }
            ALIASES_ENTRY => {
                aliases_raw = Some(
                    serde_json::from_slice(&buf).map_err(|e| format!("aliases.json parse: {e}"))?,
                );
            }
            DOCUMENTS_ENTRY => {
                documents_ndjson = Some(buf);
            }
            _ => unreachable!("guarded by ALLOWED_ENTRIES contains check above"),
        }
    }

    let manifest = manifest.ok_or_else(|| "missing manifest.json".to_owned())?;
    let mapping_value = mapping_raw.ok_or_else(|| "missing mapping.json".to_owned())?;
    let settings = settings_raw.ok_or_else(|| "missing settings.json".to_owned())?;
    let aliases_value = aliases_raw.ok_or_else(|| "missing aliases.json".to_owned())?;
    let documents_bytes = documents_ndjson.ok_or_else(|| "missing documents.ndjson".to_owned())?;

    // mapping.json is the value returned by `IndexMapping::as_value()`:
    // `{ "properties": { … } }`. Re-hydrate via the same JSON path the
    // PUT /index handler uses, so on-the-wire mapping and snapshot
    // mapping stay symmetric.
    let properties = mapping_value
        .get("properties")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()));
    let mapping = IndexMapping::from_properties_value(&properties)
        .map_err(|e| format!("mapping.json invalid: {e}"))?;

    let aliases = parse_aliases_value(&aliases_value)?;
    let documents = parse_documents_ndjson(&documents_bytes)?;

    Ok(ParsedSnapshot {
        manifest,
        mapping,
        settings,
        aliases,
        documents,
    })
}

fn aliases_to_value(aliases: &BTreeMap<String, Value>) -> Value {
    let mut map = Map::new();
    for (name, definition) in aliases {
        map.insert(name.clone(), definition.clone());
    }
    Value::Object(map)
}

fn parse_aliases_value(value: &Value) -> Result<BTreeMap<String, Value>, String> {
    let object = value
        .as_object()
        .ok_or_else(|| "aliases.json must be a JSON object".to_owned())?;
    let mut out = BTreeMap::new();
    for (name, definition) in object {
        out.insert(name.clone(), definition.clone());
    }
    Ok(out)
}

fn parse_documents_ndjson(bytes: &[u8]) -> Result<Vec<(String, Value)>, String> {
    let text =
        std::str::from_utf8(bytes).map_err(|e| format!("documents.ndjson is not UTF-8: {e}"))?;
    let mut out = Vec::new();
    for (line_no, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .map_err(|e| format!("documents.ndjson line {} parse: {e}", line_no + 1))?;
        let id = value
            .get("_id")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("documents.ndjson line {}: missing `_id`", line_no + 1))?
            .to_owned();
        let source = value
            .get("_source")
            .cloned()
            .ok_or_else(|| format!("documents.ndjson line {}: missing `_source`", line_no + 1))?;
        out.push((id, source));
    }
    Ok(out)
}
