//! OpenSearch-compatible `_cat` admin endpoints.

use axum::{
    extract::{Path, Query, State},
    http::{header::CONTENT_TYPE, HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

const CLUSTER_NAME: &str = "surch-cluster";

/// One `_cat/indices` column: ES 8.6.1 `RestIndicesAction.getTableWithHeader`
/// (RestIndicesAction.java:254-270) name, aliases, default visibility and
/// alignment, restricted to the columns Surch can fill.
struct CatColumn {
    name: &'static str,
    aliases: &'static [&'static str],
    default: bool,
    right_aligned: bool,
}

const CAT_INDICES_COLUMNS: &[CatColumn] = &[
    CatColumn {
        name: "health",
        aliases: &["h"],
        default: true,
        right_aligned: false,
    },
    CatColumn {
        name: "status",
        aliases: &["s"],
        default: true,
        right_aligned: false,
    },
    CatColumn {
        name: "index",
        aliases: &["i", "idx"],
        default: true,
        right_aligned: false,
    },
    CatColumn {
        name: "uuid",
        aliases: &["id", "uuid"],
        default: true,
        right_aligned: false,
    },
    CatColumn {
        name: "pri",
        aliases: &["p", "shards.primary", "shardsPrimary"],
        default: true,
        right_aligned: true,
    },
    CatColumn {
        name: "rep",
        aliases: &["r", "shards.replica", "shardsReplica"],
        default: true,
        right_aligned: true,
    },
    CatColumn {
        name: "docs.count",
        aliases: &["dc", "docsCount"],
        default: true,
        right_aligned: true,
    },
    CatColumn {
        name: "docs.deleted",
        aliases: &["dd", "docsDeleted"],
        default: true,
        right_aligned: true,
    },
    CatColumn {
        name: "creation.date",
        aliases: &["cd"],
        default: false,
        right_aligned: false,
    },
    CatColumn {
        name: "creation.date.string",
        aliases: &["cds"],
        default: false,
        right_aligned: false,
    },
    CatColumn {
        name: "store.size",
        aliases: &["ss", "storeSize"],
        default: true,
        right_aligned: true,
    },
    CatColumn {
        name: "pri.store.size",
        aliases: &[],
        default: true,
        right_aligned: true,
    },
];

/// A requested column: its table name and the name it is displayed under
/// (ES `RestTable.DisplayHeader`: the alias when the request used one).
struct DisplayColumn {
    column: &'static CatColumn,
    display: String,
}

/// Axum handler for `GET /_cat/indices`.
pub async fn cat_indices_handler(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> axum::response::Response {
    cat_indices(&state, None, &params, &headers)
}

/// Axum handler for `GET /_cat/indices/{index}` (comma list, wildcards,
/// aliases).
pub async fn cat_indices_target_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
    Query(params): Query<HashMap<String, String>>,
    headers: HeaderMap,
) -> axum::response::Response {
    cat_indices(&state, Some(&target), &params, &headers)
}

/// MVP L2b: `_cat/indices` with ES 8.6.1 table semantics:
///
/// - target: comma-separated names, `*` wildcards and aliases; a missing
///   concrete name is `404 index_not_found_exception`
///   (`IndicesOptions.strictExpand`, RestIndicesAction.java:84), a wildcard
///   matching nothing yields no row;
/// - `h`: requested columns in request order, names or aliases, `*`
///   wildcards; unknown names are skipped (`RestTable.buildDisplayHeaders`,
///   RestTable.java:148-196); without `h`, the default columns;
/// - `creation.date` = epoch millis, `creation.date.string` = the
///   `strict_date_time` print in UTC (`2026-11-15T10:12:13.123Z`,
///   RestIndicesAction.java:678-680);
/// - JSON (`format=json`, or a JSON `Accept`): an array of objects whose
///   values are all strings (`RestTable.buildXContentBuilder` +
///   `renderValue`, RestTable.java:59-75, 360-410); text otherwise
///   (`buildTextPlainResponse`, RestTable.java:77-113): cells padded to the
///   column width, right-aligned columns left-padded, one space between
///   cells, no padding after the last cell, `\n` after each row, a header
///   row with `v=true`.
///
/// Surch deviation: a request with neither `format` nor `Accept` gets
/// JSON (ES: text), the historical Surch default kept for existing
/// clients; `s` (row sort) is not implemented (rows by index name).
fn cat_indices(
    state: &AppState,
    target: Option<&str>,
    params: &HashMap<String, String>,
    headers: &HeaderMap,
) -> axum::response::Response {
    let indices = match resolve_cat_indices(state, target) {
        Ok(indices) => indices,
        Err(error) => return error.into_response(),
    };
    let columns = display_columns(params.get("h").map(String::as_str));
    let rows: Vec<Vec<Option<String>>> = indices
        .iter()
        .map(|index| {
            let creation = state
                .index_metadata(index)
                .and_then(|metadata| crate::index::creation_date_millis(&metadata.settings));
            columns
                .iter()
                .map(|display| cat_indices_cell(state, index, creation, display.column.name))
                .collect()
        })
        .collect();

    if cat_wants_json(params, headers) {
        let body: Vec<Value> = rows
            .iter()
            .map(|row| {
                let mut object = serde_json::Map::with_capacity(columns.len());
                for (display, cell) in columns.iter().zip(row) {
                    object.insert(
                        display.display.clone(),
                        cell.clone().map_or(Value::Null, Value::String),
                    );
                }
                Value::Object(object)
            })
            .collect();
        return cat_response(body);
    }

    let verbose = params.get("v").is_some_and(|v| v.is_empty() || v == "true");
    let mut widths: Vec<usize> = columns
        .iter()
        .map(|display| {
            if verbose {
                display.display.chars().count()
            } else {
                0
            }
        })
        .collect();
    for row in &rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.as_deref().map_or(0, |c| c.chars().count()));
        }
    }
    let mut text = String::new();
    let push_row = |cells: Vec<Option<&str>>, text: &mut String| {
        let last = cells.len().saturating_sub(1);
        for (col, cell) in cells.into_iter().enumerate() {
            let value = cell.unwrap_or("");
            let pad = widths[col].saturating_sub(value.chars().count());
            if pad > 0 && columns[col].column.right_aligned {
                text.push_str(&" ".repeat(pad));
                text.push_str(value);
            } else {
                text.push_str(value);
                if col != last {
                    text.push_str(&" ".repeat(pad));
                }
            }
            if col != last {
                text.push(' ');
            }
        }
        text.push('\n');
    };
    if verbose {
        push_row(
            columns.iter().map(|d| Some(d.display.as_str())).collect(),
            &mut text,
        );
    }
    for row in &rows {
        push_row(row.iter().map(Option::as_deref).collect(), &mut text);
    }
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "text/plain; charset=UTF-8")],
        text,
    )
        .into_response()
}

/// `format` wins (`json` / `yaml` / `cbor` / `smile` = structured, else
/// text); otherwise the `Accept` header (ES `RestTable.getResponseContentType`);
/// neither: JSON (Surch default, see [`cat_indices`]).
fn cat_wants_json(params: &HashMap<String, String>, headers: &HeaderMap) -> bool {
    if let Some(format) = params.get("format") {
        return !matches!(format.as_str(), "txt" | "text");
    }
    match headers
        .get(axum::http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
    {
        Some(accept) => accept.contains("json"),
        None => true,
    }
}

/// The physical indices `_cat/indices[/{target}]` reports, sorted by name.
fn resolve_cat_indices(
    state: &AppState,
    target: Option<&str>,
) -> Result<Vec<String>, OpenSearchError> {
    let all = state.index_names();
    let Some(target) = target.filter(|t| !t.is_empty() && *t != "_all") else {
        return Ok(all);
    };
    let mut selected: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for part in target.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        if part.contains('*') {
            for index in &all {
                if wildcard_match(part, index) {
                    selected.insert(index.clone());
                }
            }
            for (alias, indices) in state.all_aliases() {
                if wildcard_match(part, &alias) {
                    selected.extend(indices);
                }
            }
            continue;
        }
        let resolved = state.resolve_index(part);
        if resolved.is_empty() {
            return Err(OpenSearchError::new(
                StatusCode::NOT_FOUND,
                "index_not_found_exception",
                format!("no such index [{part}]"),
            ));
        }
        selected.extend(resolved);
    }
    Ok(selected.into_iter().collect())
}

/// ES `Regex.simpleMatch` for patterns made of `*` wildcards.
fn wildcard_match(pattern: &str, text: &str) -> bool {
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == text;
    }
    let mut rest = text;
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            let Some(stripped) = rest.strip_prefix(part) else {
                return false;
            };
            rest = stripped;
        } else if i == parts.len() - 1 {
            return rest.ends_with(part);
        } else if let Some(position) = rest.find(part) {
            rest = &rest[position + part.len()..];
        } else {
            return false;
        }
    }
    true
}

/// The displayed columns for an `h` parameter (ES
/// `RestTable.expandHeadersFromRequest` + `buildDisplayHeaders`).
fn display_columns(h: Option<&str>) -> Vec<DisplayColumn> {
    let Some(h) = h else {
        return CAT_INDICES_COLUMNS
            .iter()
            .filter(|column| column.default)
            .map(|column| DisplayColumn {
                column,
                display: column.name.to_owned(),
            })
            .collect();
    };
    let mut requested: Vec<String> = Vec::new();
    for header in h.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if header.contains('*') {
            for column in CAT_INDICES_COLUMNS {
                let matched = wildcard_match(header, column.name)
                    || column
                        .aliases
                        .iter()
                        .any(|alias| wildcard_match(header, alias));
                if matched && !requested.iter().any(|r| r == column.name) {
                    requested.push(column.name.to_owned());
                }
            }
        } else if !requested.iter().any(|r| r == header) {
            requested.push(header.to_owned());
        }
    }
    requested
        .into_iter()
        .filter_map(|wanted| {
            if let Some(column) = CAT_INDICES_COLUMNS.iter().find(|c| c.name == wanted) {
                return Some(DisplayColumn {
                    column,
                    display: wanted,
                });
            }
            CAT_INDICES_COLUMNS
                .iter()
                .find(|c| c.aliases.contains(&wanted.as_str()))
                .map(|column| DisplayColumn {
                    column,
                    display: wanted,
                })
        })
        .collect()
}

fn cat_indices_cell(
    state: &AppState,
    index: &str,
    creation: Option<i64>,
    column: &str,
) -> Option<String> {
    Some(match column {
        "health" => "green".to_owned(),
        "status" => "open".to_owned(),
        "index" => index.to_owned(),
        "uuid" => "_na_".to_owned(),
        "pri" => "1".to_owned(),
        "rep" => "0".to_owned(),
        "docs.count" => state.count(index).to_string(),
        "docs.deleted" => "0".to_owned(),
        "creation.date" => creation?.to_string(),
        "creation.date.string" => crate::es_json::format_es_date(creation?, "strict_date_time"),
        "store.size" | "pri.store.size" => "0b".to_owned(),
        _ => return None,
    })
}

/// Axum handler for `GET /_cat/health`.
pub async fn cat_health_handler(State(state): State<AppState>) -> impl IntoResponse {
    let shard_count = state.index_names().len() as u64;
    let row = json!({
        "epoch": current_epoch_secs().to_string(),
        "timestamp": current_timestamp_hms(),
        "cluster": CLUSTER_NAME,
        "status": "green",
        "node.total": "1",
        "node.data": "1",
        "discovered_master": "true",
        "shards": shard_count.to_string(),
        "pri": shard_count.to_string(),
        "relo": "0",
        "init": "0",
        "unassign": "0",
        "pending_tasks": "0",
        "max_task_wait_time": "-",
        "active_shards_percent": "100.0%",
    });
    cat_response(vec![row])
}

/// Axum handler for `GET /_cat/aliases`.
pub async fn cat_aliases_handler(State(state): State<AppState>) -> impl IntoResponse {
    let rows = collect_alias_rows(&state, None);
    cat_response(rows)
}

/// Axum handler for `GET /_cat/aliases/{name}`.
pub async fn cat_aliases_by_name_handler(
    State(state): State<AppState>,
    Path(alias): Path<String>,
) -> impl IntoResponse {
    if !state.alias_exists(&alias) {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "aliases_not_found_exception",
            format!("alias [{alias}] missing"),
        )
        .into_response();
    }
    let rows = collect_alias_rows(&state, Some(alias.as_str()));
    cat_response(rows)
}

/// Axum handler for `GET /_cat/count`.
pub async fn cat_count_handler(State(state): State<AppState>) -> impl IntoResponse {
    let total: u64 = state
        .index_names()
        .iter()
        .map(|index| state.count(index))
        .sum();
    cat_count_response(total)
}

/// Axum handler for `GET /_cat/count/{index}`.
pub async fn cat_count_index_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    let indices = state.resolve_index(&target);
    if indices.is_empty() {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{target}] missing"),
        )
        .into_response();
    }
    let total: u64 = indices.iter().map(|index| state.count(index)).sum();
    cat_count_response(total)
}

fn cat_count_response(total: u64) -> axum::response::Response {
    let row = json!({
        "epoch": current_epoch_secs().to_string(),
        "timestamp": current_timestamp_hms(),
        "count": total.to_string(),
    });
    cat_response(vec![row])
}

fn collect_alias_rows(state: &AppState, filter: Option<&str>) -> Vec<Value> {
    state
        .all_aliases()
        .into_iter()
        .filter(|(alias, _)| filter.is_none_or(|name| alias == name))
        .flat_map(|(alias, indices)| {
            indices.into_iter().map(move |index| {
                json!({
                    "alias": alias,
                    "index": index,
                    "filter": "-",
                    "routing.index": "-",
                    "routing.search": "-",
                    "is_write_index": "-",
                })
            })
        })
        .collect()
}

fn cat_response(rows: Vec<Value>) -> axum::response::Response {
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/json")],
        Json(rows),
    )
        .into_response()
}

fn current_epoch_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn current_timestamp_hms() -> String {
    let secs = current_epoch_secs();
    let seconds = secs % 60;
    let minutes = (secs / 60) % 60;
    let hours = (secs / 3600) % 24;
    format!("{hours:02}:{minutes:02}:{seconds:02}")
}
