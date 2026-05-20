//! B1 phase 3 — Surch ↔ Elasticsearch 8.6.1 oracle cross-check driver.
//!
//! Replays the curated matchID manifest at
//! `tests/matchid_compat/replays/deces_v1.json` against BOTH engines (a
//! running Surch HTTP API and a running Elasticsearch 8.6.1 cluster),
//! captures four diff axes per request, and emits a
//! `surch.bench.b1_oracle.v1` envelope under
//! `target/bench-reports/b1-oracle.json`.
//!
//! The four diff axes covered today:
//!   * `hits.total.value` — value field of total.
//!   * `hits.total.relation` — `eq` / `gte` (post-7.0 contract).
//!   * `hits.hits[0]._id` — top-hit id; left `None` for filter-only
//!     or scroll-only requests where the manifest omits the
//!     `hits.hits[0]._id` expectation.
//!   * `aggregations.<name>` — full sub-tree per agg name when the
//!     request body declares `aggs` / `aggregations`.
//!
//! Pipeline:
//!   1. Bootstrap each engine (DELETE /<index>, PUT mapping, cluster
//!      health, bulk-load the gunzipped slice, refresh). Skippable via
//!      `--skip-bootstrap` when both clusters already host the data.
//!   2. Load the replay manifest via `matchid_replay::load_replay`.
//!   3. For every non-skipped entry, POST the body to BOTH engines
//!      (honouring the `?scroll=` query param when present).
//!   4. Capture the four axes and append a divergence record whenever
//!      the Surch and ES projections disagree, unless the request name
//!      is in `KNOWN_PARTIAL_NAMES`. When either engine answers with a
//!      non-2xx status the projection step is skipped and the driver
//!      instead records `http.status` (+ `http.body` when only one side
//!      failed) divergences on that entry.
//!   5. Emit `surch.bench.b1_oracle.v1` JSON to `--out`, regardless of
//!      the divergence count — the report path is always populated.
//!   6. Exit 0 iff the post-`KNOWN_PARTIAL_NAMES` divergence count is
//!      zero, else 1 with stderr summary.
//!
//! CLI:
//! ```text
//! b1_oracle --url-surch <URL> --url-es <URL>
//!           [--mapping <PATH>] [--slice <PATH>] [--replay <PATH>]
//!           [--out <PATH>] [--skip-bootstrap]
//! ```
//!
//! The K8s manifest that actually wires up the two endpoints is a
//! separate lot — this binary is verified locally by build / clippy /
//! fmt + the in-file unit tests (no network I/O).

use std::{
    collections::BTreeMap,
    env,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
    time::Duration,
};

use chrono::{SecondsFormat, Utc};
use flate2::read::GzDecoder;
use matchid_replay::{load_replay, Replay, Request as ReplayRequest};
use reqwest::{Client, StatusCode};
use serde_json::{json, Value};

/// On-disk envelope schema. Bumped whenever the JSON shape changes.
const SCHEMA: &str = "surch.bench.b1_oracle.v1";

/// Default index used by the deces fixture (the slice bakes
/// `_index: "deces"` into every bulk action header).
const DEFAULT_INDEX: &str = "deces";

/// Names of replay requests whose Surch implementation is intentionally
/// partial vs Elasticsearch 8.6.1 today. Divergences on these entries are recorded
/// in the envelope but excluded from `divergence_count_unexpected` so
/// the exit gate stays meaningful while the underlying gaps remain open.
///
/// Buckets:
///   * gap-A2  — `geo_bounding_box` (no geo type in Surch today)
///   * gap-A5  — non-date `function_score` decay functions
///   * gap-A10 — sort on a `text`-mapped field. Surch's phase-1 alias
///     transparently falls back to the term dictionary; Elasticsearch 8.6.1 strictly
///     rejects the shape with `illegal_argument_exception` ("Text fields
///     are not optimised for operations that require per-document field
///     data like aggregations and sorting…"). The request that exercises
///     this in `deces_v1.json` is `sort_nom_desc` (verbatim
///     `sort: [{"NOM":"desc"}]` against the `text`-typed NOM field).
///   * gap-A12 — composite aggregation with date_histogram source
///
/// Cross-check rule: every entry listed here MUST be the `name` of a
/// real request in `tests/matchid_compat/replays/deces_v1.json`. The
/// `known_partial_names_subset_of_replay` unit test enforces this and
/// fails CI if a stale entry remains.
///
/// TODO(b1-phase-3): track the live gap list in
/// `docs/wp-d-matchid/B1-phase-3-plan.md` and keep this const in sync.
const KNOWN_PARTIAL_NAMES: &[&str] = &["sort_nom_desc"];

/// Default paths — all relative to the workspace root. Match the
/// canonical layout under `tests/matchid_compat/`.
const DEFAULT_MAPPING: &str = "tests/matchid_compat/deces/mapping.json";
const DEFAULT_SLICE: &str = "tests/matchid_compat/deces/slice-10000.ndjson.gz";
const DEFAULT_REPLAY: &str = "tests/matchid_compat/replays/deces_v1.json";
const DEFAULT_OUT: &str = "target/bench-reports/b1-oracle.json";

/// HTTP timeouts (kept conservative — the K8s smoke job runs both
/// engines on the same node, so 5s per search is plenty and 30s on
/// the one-shot bulk covers a cold Elasticsearch 8.6.1 JVM warmup).
const BULK_TIMEOUT: Duration = Duration::from_secs(30);
const SEARCH_TIMEOUT: Duration = Duration::from_secs(5);
const HEALTH_TIMEOUT: Duration = Duration::from_secs(30);

/// Parsed CLI arguments.
struct Cli {
    url_surch: String,
    url_es: String,
    mapping: PathBuf,
    slice: PathBuf,
    replay: PathBuf,
    out: PathBuf,
    skip_bootstrap: bool,
}

impl Cli {
    fn parse() -> Result<Self, String> {
        let mut url_surch: Option<String> = None;
        let mut url_es: Option<String> = None;
        let mut mapping = PathBuf::from(DEFAULT_MAPPING);
        let mut slice = PathBuf::from(DEFAULT_SLICE);
        let mut replay = PathBuf::from(DEFAULT_REPLAY);
        let mut out = PathBuf::from(DEFAULT_OUT);
        let mut skip_bootstrap = false;

        let mut args = env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--url-surch" => {
                    url_surch = Some(args.next().ok_or("--url-surch expects a value")?);
                }
                "--url-es" => {
                    url_es = Some(args.next().ok_or("--url-es expects a value")?);
                }
                "--mapping" => {
                    mapping = PathBuf::from(args.next().ok_or("--mapping expects a value")?);
                }
                "--slice" => {
                    slice = PathBuf::from(args.next().ok_or("--slice expects a value")?);
                }
                "--replay" => {
                    replay = PathBuf::from(args.next().ok_or("--replay expects a value")?);
                }
                "--out" => {
                    out = PathBuf::from(args.next().ok_or("--out expects a value")?);
                }
                "--skip-bootstrap" => skip_bootstrap = true,
                "-h" | "--help" => {
                    print_usage();
                    std::process::exit(0);
                }
                other => return Err(format!("unknown argument: {other}")),
            }
        }

        Ok(Cli {
            url_surch: url_surch.ok_or("--url-surch is required")?,
            url_es: url_es.ok_or("--url-es is required")?,
            mapping,
            slice,
            replay,
            out,
            skip_bootstrap,
        })
    }
}

fn print_usage() {
    eprintln!(
        "usage: b1_oracle --url-surch <URL> --url-es <URL> \
         [--mapping <PATH>] [--slice <PATH>] [--replay <PATH>] \
         [--out <PATH>] [--skip-bootstrap]"
    );
}

/// Raw HTTP response captured from one engine for one replay entry.
/// We keep both the parsed JSON body (when parseable) and the raw text
/// so the report can surface the exact Elasticsearch error envelope on non-2xx
/// responses without losing fidelity.
#[derive(Debug, Clone)]
struct EngineResponse {
    /// Numeric HTTP status (e.g. 200, 400). Stored as `u16` so the
    /// helpers can be exercised without going through `reqwest`.
    status: u16,
    /// Parsed JSON body, or `Value::Null` when the body was empty or
    /// not valid JSON (in which case `raw_text` holds the original).
    body: Value,
    /// Verbatim response text — used for `http.body` divergences when
    /// the engines disagree on a non-2xx error envelope.
    raw_text: String,
}

impl EngineResponse {
    fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// JSON-friendly snapshot of the response, embedded into divergence
    /// records. Prefers the parsed `body` so the envelope stays valid
    /// JSON; falls back to the raw text on parse failures.
    fn snapshot(&self) -> Value {
        if self.body.is_null() && !self.raw_text.is_empty() {
            json!({ "_raw": self.raw_text })
        } else {
            self.body.clone()
        }
    }
}

/// One captured projection from a `_search` response (per engine).
#[derive(Debug, Clone, PartialEq)]
struct SearchProjection {
    total_value: Option<Value>,
    total_relation: Option<Value>,
    top_id: Option<Value>,
    /// `aggregations` sub-tree keyed by agg name. `BTreeMap` keeps the
    /// JSON order deterministic on disk.
    aggregations: BTreeMap<String, Value>,
}

impl SearchProjection {
    fn from_response(body: &Value) -> Self {
        let hits = body.get("hits");
        let total = hits.and_then(|h| h.get("total"));
        let total_value = total.and_then(|t| t.get("value")).cloned();
        let total_relation = total.and_then(|t| t.get("relation")).cloned();
        let top_id = hits
            .and_then(|h| h.get("hits"))
            .and_then(|h| h.as_array())
            .and_then(|arr| arr.first())
            .and_then(|hit| hit.get("_id"))
            .cloned();

        let mut aggregations = BTreeMap::new();
        if let Some(aggs) = body.get("aggregations").and_then(|v| v.as_object()) {
            for (k, v) in aggs {
                aggregations.insert(k.clone(), v.clone());
            }
        }

        SearchProjection {
            total_value,
            total_relation,
            top_id,
            aggregations,
        }
    }
}

/// Single divergence record — one per (request_name, axis) pair.
#[derive(Debug)]
struct Divergence {
    request_name: String,
    axis: String,
    surch_value: Value,
    es_value: Value,
}

impl Divergence {
    fn to_json(&self) -> Value {
        json!({
            "request_name": self.request_name,
            "axis": self.axis,
            "surch_value": self.surch_value.clone(),
            "es_value": self.es_value.clone(),
        })
    }
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => code,
        Err(err) => {
            eprintln!("b1_oracle: {err}");
            ExitCode::from(1)
        }
    }
}

async fn run() -> Result<ExitCode, String> {
    let cli = Cli::parse().inspect_err(|_| {
        print_usage();
    })?;

    let search_client = Client::builder()
        .timeout(SEARCH_TIMEOUT)
        .build()
        .map_err(|e| format!("build search client: {e}"))?;
    let bulk_client = Client::builder()
        .timeout(BULK_TIMEOUT)
        .build()
        .map_err(|e| format!("build bulk client: {e}"))?;

    if !cli.skip_bootstrap {
        bootstrap(&bulk_client, &cli.url_surch, &cli.mapping, &cli.slice).await?;
        bootstrap(&bulk_client, &cli.url_es, &cli.mapping, &cli.slice).await?;
    }

    let replay = load_replay(
        cli.replay
            .to_str()
            .ok_or_else(|| format!("non-UTF-8 replay path: {:?}", cli.replay))?,
    )
    .map_err(|e| format!("load replay {:?}: {e}", cli.replay))?;

    let (total, skipped, divergences) =
        run_replay(&search_client, &cli.url_surch, &cli.url_es, &replay).await?;

    // The report is always emitted: gate verdict aside, downstream
    // tooling (K8s artefact collector, dashboard ingest) relies on the
    // file being present after every run.
    let report = build_report(&cli, total, skipped, &divergences);
    write_report(&cli.out, &report)?;

    let exit_code = if divergences.is_empty() {
        0
    } else {
        eprintln!(
            "b1_oracle: {} unexpected divergence(s) over {} request(s); see {}",
            divergences.len(),
            total - skipped,
            cli.out.display()
        );
        for div in divergences.iter().take(5) {
            eprintln!(
                "  - {} [{}] surch={} es={}",
                div.request_name, div.axis, div.surch_value, div.es_value
            );
        }
        1
    };
    Ok(ExitCode::from(exit_code))
}

/// Bootstrap one engine: DELETE the index (best-effort), PUT the
/// mapping, wait for cluster health, POST /_bulk with the gunzipped
/// slice, then refresh.
async fn bootstrap(
    client: &Client,
    base_url: &str,
    mapping_path: &Path,
    slice_path: &Path,
) -> Result<(), String> {
    let mapping_body = fs::read_to_string(mapping_path)
        .map_err(|e| format!("read mapping {:?}: {e}", mapping_path))?;
    let mapping_json: Value = serde_json::from_str(&mapping_body)
        .map_err(|e| format!("parse mapping {:?}: {e}", mapping_path))?;

    let index_url = format!("{}/{}", base_url.trim_end_matches('/'), DEFAULT_INDEX);
    let health_url = format!(
        "{}/_cluster/health?wait_for_status=yellow&timeout=30s",
        base_url.trim_end_matches('/')
    );
    let bulk_url = format!("{}/_bulk", base_url.trim_end_matches('/'));
    let refresh_url = format!(
        "{}/{}/_refresh",
        base_url.trim_end_matches('/'),
        DEFAULT_INDEX
    );

    // DELETE — swallow 404 and "index_not_found" errors.
    let resp = client.delete(&index_url).send().await;
    if let Ok(resp) = resp {
        let status = resp.status();
        if !status.is_success() && status != StatusCode::NOT_FOUND {
            // Read the body to confirm it's an index_not_found before
            // accepting the failure; otherwise surface it loudly.
            let body = resp.text().await.unwrap_or_default();
            if !body.contains("index_not_found_exception") {
                return Err(format!(
                    "DELETE {} returned {} ({})",
                    index_url, status, body
                ));
            }
        }
    }

    // PUT mapping.
    let resp = client
        .put(&index_url)
        .json(&mapping_json)
        .send()
        .await
        .map_err(|e| format!("PUT {}: {e}", index_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("PUT {} returned {} ({})", index_url, status, body));
    }

    // Cluster health.
    let resp = client
        .get(&health_url)
        .timeout(HEALTH_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("GET {}: {e}", health_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("GET {} returned {} ({})", health_url, status, body));
    }

    // Bulk-load — gunzip in memory; the slice is ~2 MB so one shot is
    // fine and avoids chunked-Transfer-Encoding gotchas in Elasticsearch.
    let bulk_body = read_gzipped(slice_path)?;
    let resp = client
        .post(&bulk_url)
        .header("Content-Type", "application/x-ndjson")
        .body(bulk_body)
        .send()
        .await
        .map_err(|e| format!("POST {}: {e}", bulk_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("POST {} returned {} ({})", bulk_url, status, body));
    }
    let bulk_json: Value = resp
        .json()
        .await
        .map_err(|e| format!("parse bulk response: {e}"))?;
    if bulk_json.get("errors").and_then(|v| v.as_bool()) == Some(true) {
        return Err(format!("bulk reported errors against {}", base_url));
    }

    // Refresh.
    let resp = client
        .post(&refresh_url)
        .send()
        .await
        .map_err(|e| format!("POST {}: {e}", refresh_url))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!(
            "POST {} returned {} ({})",
            refresh_url, status, body
        ));
    }

    Ok(())
}

fn read_gzipped(path: &Path) -> Result<Vec<u8>, String> {
    let file = File::open(path).map_err(|e| format!("open {:?}: {e}", path))?;
    let mut decoder = GzDecoder::new(file);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| format!("gunzip {:?}: {e}", path))?;
    Ok(out)
}

/// Iterate the replay, POSTing each request to both engines. Returns
/// `(total_requests, skipped_count, divergences)`.
async fn run_replay(
    client: &Client,
    surch_url: &str,
    es_url: &str,
    replay: &Replay,
) -> Result<(usize, usize, Vec<Divergence>), String> {
    let mut skipped = 0usize;
    let mut divergences = Vec::new();

    for entry in &replay.requests {
        if entry.skip.is_some() {
            skipped += 1;
            continue;
        }
        let entry_divs = compare_entry(client, surch_url, es_url, entry).await?;
        divergences.extend(entry_divs);
    }

    Ok((replay.requests.len(), skipped, divergences))
}

/// Hit one entry against both engines and emit zero-or-more
/// `Divergence` records. Pure HTTP plumbing — the diff itself is in
/// [`diff_responses`] so it can be unit-tested without a network.
async fn compare_entry(
    client: &Client,
    surch_url: &str,
    es_url: &str,
    entry: &ReplayRequest,
) -> Result<Vec<Divergence>, String> {
    let path = &entry.request.path;
    let surch_target = format!("{}{}", surch_url.trim_end_matches('/'), path);
    let es_target = format!("{}{}", es_url.trim_end_matches('/'), path);

    let surch = post_search(client, &surch_target, &entry.request).await?;
    let es = post_search(client, &es_target, &entry.request).await?;

    Ok(diff_responses(entry, &surch, &es))
}

/// Pure projection + diff routine. Returns the divergence records for
/// one replay entry against the two engine responses. Behaviour:
///   * `_msearch` paths are always treated as a no-op (the matchid
///     compat harness owns msearch assertions); the engines are still
///     hit upstream so state stays consistent.
///   * If either response is non-2xx, projection is skipped and the
///     divergence list captures `http.status` (+ `http.body` when the
///     status mismatch is one-sided OR the bodies differ).
///   * Entries listed in `KNOWN_PARTIAL_NAMES` always return an empty
///     Vec — divergences are suppressed from the gate.
fn diff_responses(
    entry: &ReplayRequest,
    surch: &EngineResponse,
    es: &EngineResponse,
) -> Vec<Divergence> {
    if entry.request.path.starts_with("/_msearch") {
        return Vec::new();
    }

    if KNOWN_PARTIAL_NAMES.contains(&entry.name.as_str()) {
        return Vec::new();
    }

    // Non-2xx path: don't attempt the four-axis projection (the body
    // shape is an error envelope, not a `_search` response). Surface
    // status/body divergences instead so the report stays meaningful.
    if !surch.is_success() || !es.is_success() {
        let mut out = Vec::new();
        if surch.status != es.status {
            out.push(Divergence {
                request_name: entry.name.clone(),
                axis: "http.status".into(),
                surch_value: json!(surch.status),
                es_value: json!(es.status),
            });
        }
        let surch_snap = surch.snapshot();
        let es_snap = es.snapshot();
        if surch_snap != es_snap {
            out.push(Divergence {
                request_name: entry.name.clone(),
                axis: "http.body".into(),
                surch_value: surch_snap,
                es_value: es_snap,
            });
        }
        return out;
    }

    let surch_proj = SearchProjection::from_response(&surch.body);
    let es_proj = SearchProjection::from_response(&es.body);

    let mut out = Vec::new();
    let want_top_id = entry
        .expected
        .as_ref()
        .map(|e| e.top_id.is_some())
        .unwrap_or(false);

    if surch_proj.total_value != es_proj.total_value {
        out.push(Divergence {
            request_name: entry.name.clone(),
            axis: "hits.total.value".into(),
            surch_value: surch_proj.total_value.clone().unwrap_or(Value::Null),
            es_value: es_proj.total_value.clone().unwrap_or(Value::Null),
        });
    }
    if surch_proj.total_relation != es_proj.total_relation {
        out.push(Divergence {
            request_name: entry.name.clone(),
            axis: "hits.total.relation".into(),
            surch_value: surch_proj.total_relation.clone().unwrap_or(Value::Null),
            es_value: es_proj.total_relation.clone().unwrap_or(Value::Null),
        });
    }
    if want_top_id && surch_proj.top_id != es_proj.top_id {
        out.push(Divergence {
            request_name: entry.name.clone(),
            axis: "hits.hits[0]._id".into(),
            surch_value: surch_proj.top_id.clone().unwrap_or(Value::Null),
            es_value: es_proj.top_id.clone().unwrap_or(Value::Null),
        });
    }

    // aggregations — keyed comparison. Missing on one side counts as
    // Null so the divergence is visible in the report.
    let mut agg_names: Vec<&String> = surch_proj
        .aggregations
        .keys()
        .chain(es_proj.aggregations.keys())
        .collect();
    agg_names.sort();
    agg_names.dedup();
    for name in agg_names {
        let s_val = surch_proj
            .aggregations
            .get(name)
            .cloned()
            .unwrap_or(Value::Null);
        let e_val = es_proj
            .aggregations
            .get(name)
            .cloned()
            .unwrap_or(Value::Null);
        if s_val != e_val {
            out.push(Divergence {
                request_name: entry.name.clone(),
                axis: format!("aggregations.{name}"),
                surch_value: s_val,
                es_value: e_val,
            });
        }
    }

    out
}

/// POST one replay entry to a single engine and return the captured
/// [`EngineResponse`]. Handles both the `body` (single JSON document)
/// and `body_ndjson` (line-delimited; used by `_msearch`) forms. The
/// `?scroll=…` query param is part of `entry.request.path` already and
/// flows through unchanged.
///
/// Non-2xx responses are NOT errors — they are recorded as a normal
/// `EngineResponse` so [`diff_responses`] can surface them as divergence
/// records. Only transport-level failures (timeouts, DNS, body read)
/// bubble up as `Err`.
async fn post_search(
    client: &Client,
    url: &str,
    spec: &matchid_replay::RequestSpec,
) -> Result<EngineResponse, String> {
    let method = spec.method.to_uppercase();
    if method != "POST" && method != "GET" {
        return Err(format!("unsupported method: {}", spec.method));
    }

    let req = if let Some(body) = &spec.body {
        client
            .request(reqwest::Method::POST, url)
            .header("Content-Type", "application/json")
            .json(body)
    } else if let Some(ndjson) = &spec.body_ndjson {
        let mut buf = String::new();
        for line in ndjson {
            buf.push_str(&serde_json::to_string(line).map_err(|e| format!("ndjson encode: {e}"))?);
            buf.push('\n');
        }
        client
            .request(reqwest::Method::POST, url)
            .header("Content-Type", "application/x-ndjson")
            .body(buf)
    } else {
        client.request(reqwest::Method::POST, url)
    };

    let resp = req.send().await.map_err(|e| format!("POST {url}: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| format!("read body {url}: {e}"))?;
    // Best-effort parse: Elasticsearch error envelopes are valid JSON, so the
    // common case still gives a structured body for the report.
    let body = serde_json::from_str(&text).unwrap_or(Value::Null);
    Ok(EngineResponse {
        status,
        body,
        raw_text: text,
    })
}

/// Assemble the `surch.bench.b1_oracle.v1` envelope.
fn build_report(cli: &Cli, total: usize, skipped: usize, divergences: &[Divergence]) -> Value {
    let generated_at = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    json!({
        "schema": SCHEMA,
        "generated_at": generated_at,
        "surch_url": cli.url_surch,
        "es_url": cli.url_es,
        "total_requests": total,
        "skipped_count": skipped,
        "divergence_count": divergences.len(),
        "divergences": divergences.iter().map(Divergence::to_json).collect::<Vec<_>>(),
    })
}

fn write_report(path: &Path, report: &Value) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| format!("create_dir_all {:?}: {e}", parent))?;
        }
    }
    let serialized =
        serde_json::to_string_pretty(report).map_err(|e| format!("serialise report: {e}"))?;
    fs::write(path, serialized).map_err(|e| format!("write {:?}: {e}", path))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// `surch.bench.b1_oracle.v1` envelope must round-trip through
    /// `serde_json::Value` without loss — this guards the schema
    /// against accidental non-JSON insertions (e.g. NaN floats).
    #[test]
    fn envelope_round_trips() {
        let cli = Cli {
            url_surch: "http://surch:9200".into(),
            url_es: "http://es:9200".into(),
            mapping: PathBuf::from(DEFAULT_MAPPING),
            slice: PathBuf::from(DEFAULT_SLICE),
            replay: PathBuf::from(DEFAULT_REPLAY),
            out: PathBuf::from(DEFAULT_OUT),
            skip_bootstrap: true,
        };
        let divs = vec![Divergence {
            request_name: "adv_nom_prenoms_date".into(),
            axis: "hits.total.value".into(),
            surch_value: json!(1),
            es_value: json!(2),
        }];
        let report = build_report(&cli, 30, 0, &divs);
        let serialized = serde_json::to_string(&report).expect("envelope must serialise");
        let parsed: Value = serde_json::from_str(&serialized).expect("envelope must round-trip");
        assert_eq!(parsed["schema"], SCHEMA);
        assert_eq!(parsed["total_requests"], 30);
        assert_eq!(parsed["divergence_count"], 1);
        assert_eq!(
            parsed["divergences"][0]["request_name"],
            "adv_nom_prenoms_date"
        );
    }

    /// Every entry in `KNOWN_PARTIAL_NAMES` must reference a real
    /// request in `deces_v1.json`. See the const doc-comment for the
    /// rationale and the gap mapping.
    #[test]
    fn known_partial_names_subset_of_replay() {
        // Tests run with CWD = crate root (`crates/surch-demo`); the
        // replay manifest lives at the workspace root, so we walk up
        // two ancestors from `CARGO_MANIFEST_DIR` rather than relying
        // on a relative path that breaks under `cargo test -p`.
        let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let workspace_root = manifest_dir
            .ancestors()
            .nth(2)
            .expect("CARGO_MANIFEST_DIR must have a workspace root ancestor");
        let replay_path = workspace_root.join(DEFAULT_REPLAY);
        let raw = fs::read_to_string(&replay_path).unwrap_or_else(|e| {
            panic!("replay at {} must be readable: {e}", replay_path.display())
        });
        let parsed: Value = serde_json::from_str(&raw).expect("replay must be valid JSON");
        let names: HashSet<String> = parsed["requests"]
            .as_array()
            .expect("requests must be an array")
            .iter()
            .filter_map(|req| req["name"].as_str().map(str::to_string))
            .collect();

        assert!(
            !names.is_empty(),
            "deces_v1.json must declare at least one request"
        );
        for partial in KNOWN_PARTIAL_NAMES {
            assert!(
                names.contains(*partial),
                "KNOWN_PARTIAL_NAMES entry {partial:?} must match a request name in {DEFAULT_REPLAY}"
            );
        }
    }

    /// Helper: synthesise a `ReplayRequest` with the given name and a
    /// trivial `_search` POST body. Used by the non-2xx unit test.
    fn synth_entry(name: &str) -> ReplayRequest {
        let json_str = format!(
            r#"{{
                "name": "{name}",
                "request": {{
                    "method": "POST",
                    "path": "/deces/_search",
                    "body": {{ "query": {{ "match_all": {{}} }} }}
                }}
            }}"#
        );
        serde_json::from_str(&json_str).expect("synthetic replay entry must parse")
    }

    /// A non-2xx response from ES (with a 2xx from Surch) must be
    /// surfaced as `http.status` + `http.body` divergences rather than
    /// panicking. The report must still be emittable, and the gate
    /// remains green only when the entry is in `KNOWN_PARTIAL_NAMES`.
    #[test]
    fn non_2xx_response_emits_divergence_and_report() {
        let entry = synth_entry("sort_nom_desc_test_only");
        let surch_ok = EngineResponse {
            status: 200,
            body: json!({
                "hits": { "total": { "value": 1000, "relation": "eq" }, "hits": [] }
            }),
            raw_text: String::new(),
        };
        let es_400_body = json!({
            "error": {
                "type": "illegal_argument_exception",
                "reason": "Text fields are not optimised for sorting"
            },
            "status": 400
        });
        let es_err = EngineResponse {
            status: 400,
            body: es_400_body.clone(),
            raw_text: es_400_body.to_string(),
        };

        // Path 1: entry NOT in KNOWN_PARTIAL_NAMES — expect two
        // divergences (`http.status` + `http.body`).
        let divs = diff_responses(&entry, &surch_ok, &es_err);
        assert_eq!(divs.len(), 2, "expected status + body divergences");
        let axes: Vec<&str> = divs.iter().map(|d| d.axis.as_str()).collect();
        assert!(axes.contains(&"http.status"));
        assert!(axes.contains(&"http.body"));
        let status_div = divs.iter().find(|d| d.axis == "http.status").unwrap();
        assert_eq!(status_div.surch_value, json!(200));
        assert_eq!(status_div.es_value, json!(400));

        // Path 2: same shape but listed in KNOWN_PARTIAL_NAMES — the
        // existing `sort_nom_desc` matches today's const, so the
        // routine must suppress all divergences.
        let known_entry = synth_entry("sort_nom_desc");
        assert!(
            KNOWN_PARTIAL_NAMES.contains(&"sort_nom_desc"),
            "fixture relies on sort_nom_desc being in KNOWN_PARTIAL_NAMES"
        );
        let suppressed = diff_responses(&known_entry, &surch_ok, &es_err);
        assert!(
            suppressed.is_empty(),
            "KNOWN_PARTIAL_NAMES entry must suppress divergences, got {suppressed:?}"
        );

        // The report-emission contract: build + write must succeed
        // even when the divergence list is non-empty, and the JSON on
        // disk must round-trip with the right divergence count.
        let cli = Cli {
            url_surch: "http://surch:9200".into(),
            url_es: "http://es:9200".into(),
            mapping: PathBuf::from(DEFAULT_MAPPING),
            slice: PathBuf::from(DEFAULT_SLICE),
            replay: PathBuf::from(DEFAULT_REPLAY),
            out: PathBuf::from(DEFAULT_OUT),
            skip_bootstrap: true,
        };
        let report = build_report(&cli, 30, 0, &divs);
        assert_eq!(report["divergence_count"], 2);
        assert_eq!(report["schema"], SCHEMA);

        let tmp = std::env::temp_dir().join(format!(
            "b1_oracle_test_{}_{}.json",
            std::process::id(),
            // crude per-test salt so concurrent test runs do not clash
            line!()
        ));
        write_report(&tmp, &report).expect("report must write");
        let on_disk = fs::read_to_string(&tmp).expect("report must be readable");
        let parsed: Value = serde_json::from_str(&on_disk).expect("report must be valid JSON");
        assert_eq!(parsed["divergence_count"], 2);
        let _ = fs::remove_file(&tmp);
    }
}
