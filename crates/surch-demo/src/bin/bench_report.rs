//! Aggregates JSON envelopes under `target/bench-reports/<sha>/` into a
//! single Markdown summary, checks the matchID v1 SLOs, and (optionally)
//! diff's the latest run against a baseline directory.
//!
//! Recognised schemas:
//!   * `surch.bench.artillery.v1` — produced by `artillery_bench`
//!   * `surch.bench.ban_http.v1`  — produced by `surch-demo ban-http-bench`
//!   * `surch.bench.rss.v1`       — produced by `scripts/bench/rss-sample.sh`
//!   * `surch.bench.pair.v1`      — produced by `scripts/bench/run-pair.sh`
//!   * BEIR `.out` text reports    — produced by `scripts/bench/*-ndcg.sh`
//!
//! CLI:
//!   bench_report --dir target/bench-reports/<sha>
//!                [--baseline target/bench-reports/<other_sha>]
//!                [--output target/bench-reports/<sha>/summary.md]
//!                [--promote-dir docs/ops/bench-reports/<date>-<context>]
//!                [--rss-peak-mb 1024]
//!
//! Output:
//!   * Markdown summary at `--output` (or `<DIR>/summary.md`)
//!   * Promoted human report at `<promote-dir>/README.md`
//!   * Stable JSON summary next to it (`summary.json`)
//!
//! Exit code:
//!   0 → all SLOs pass *and* no regression beyond thresholds vs baseline
//!   1 → at least one SLO failed or a regression breached the threshold
//!
//! SLO thresholds (matchID v1):
//!   * Surch artillery p95 ≤ 200 ms (Surch engine only; the JVM
//!     reference engine's latency is reported but does not gate)
//!   * Surch artillery max ≤ 500 ms (Surch engine only)
//!   * artillery error rate ≤ 1 % (both engines — a non-zero rate
//!     invalidates the comparison)
//!   * Surch RSS peak ≤ `--rss-peak-mb` MB on the artillery workload
//!     (default 1024 for INSEE 10k; raise for large-corpus jobs whose
//!     resident set scales with the corpus. Surch engine only — the JVM
//!     reference engine is exempt)
//!
//! Regression thresholds vs `--baseline`:
//!   * p95 (artillery) regressed by more than 15 %
//!   * RSS peak regressed by more than 25 %

use std::{
    env, fmt, fs,
    path::{Path, PathBuf},
    process::ExitCode,
};

use chrono::{SecondsFormat, Utc};
use serde_json::Value;

const SCHEMA_ARTILLERY: &str = "surch.bench.artillery.v1";
const SCHEMA_BAN_HTTP: &str = "surch.bench.ban_http.v1";
const SCHEMA_RSS: &str = "surch.bench.rss.v1";
const SCHEMA_PAIR: &str = "surch.bench.pair.v1";
const SCHEMA_SUMMARY: &str = "surch.bench.summary.v1";

const SLO_ARTILLERY_P95_MS: f64 = 200.0;
const SLO_ARTILLERY_MAX_MS: f64 = 500.0;
const SLO_ARTILLERY_ERROR_RATE_PCT: f64 = 1.0;
const SLO_RSS_PEAK_MB: f64 = 1024.0;
const SLO_SCIFACT_NDCG_10: f64 = 0.65;
const SLO_TREC_COVID_NDCG_10: f64 = 0.55;

const REGRESSION_P95_PCT: f64 = 15.0;
const REGRESSION_RSS_PCT: f64 = 25.0;

fn main() -> ExitCode {
    match run() {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::from(1),
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}

fn run() -> Result<bool, CliError> {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_help();
        return Ok(true);
    }

    let plan = parse_args(args)?;
    let current = load_directory(&plan.dir)?;
    let baseline = match &plan.baseline {
        Some(path) => Some(load_directory(path)?),
        None => None,
    };

    let now = Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true);
    let sha = infer_sha(&plan.dir);
    let baseline_sha = plan.baseline.as_ref().map(|p| infer_sha(p));

    let slo_results = evaluate_slo(&current, plan.rss_peak_mb);
    let regression_results = match &baseline {
        Some(base) => evaluate_regressions(&current, base),
        None => Vec::new(),
    };

    let markdown = render_markdown(
        &sha,
        baseline_sha.as_deref(),
        &now,
        &current,
        baseline.as_ref(),
        &slo_results,
        &regression_results,
    );

    let output_path = plan.output_path();
    let json_output_path = plan.json_output_path();
    let slo_ok = slo_results.iter().all(|check| check.passed);
    let regression_ok = regression_results.iter().all(|check| check.passed);
    write_output(&output_path, &markdown)?;
    println!("wrote {}", output_path.display());
    let json_summary = render_json_summary(
        &sha,
        baseline_sha.as_deref(),
        &now,
        &current,
        &slo_results,
        &regression_results,
        slo_ok && regression_ok,
    );
    write_output(&json_output_path, &json_summary)?;
    println!("wrote {}", json_output_path.display());
    Ok(slo_ok && regression_ok)
}

#[derive(Debug, Clone)]
struct Plan {
    dir: PathBuf,
    baseline: Option<PathBuf>,
    output: Option<PathBuf>,
    promote_dir: Option<PathBuf>,
    rss_peak_mb: f64,
}

impl Plan {
    fn output_path(&self) -> PathBuf {
        match (&self.output, &self.promote_dir) {
            (Some(path), None) => path.clone(),
            (None, Some(dir)) => dir.join("README.md"),
            (None, None) => self.dir.join("summary.md"),
            (Some(_), Some(_)) => unreachable!("parse_args rejects output/promote_dir conflict"),
        }
    }

    fn json_output_path(&self) -> PathBuf {
        match (&self.output, &self.promote_dir) {
            (Some(path), None) => path.with_extension("json"),
            (None, Some(dir)) => dir.join("summary.json"),
            (None, None) => self.dir.join("summary.json"),
            (Some(_), Some(_)) => unreachable!("parse_args rejects output/promote_dir conflict"),
        }
    }
}

fn parse_args(args: Vec<String>) -> Result<Plan, CliError> {
    let mut dir: Option<PathBuf> = None;
    let mut baseline: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;
    let mut promote_dir: Option<PathBuf> = None;
    let mut rss_peak_mb: f64 = SLO_RSS_PEAK_MB;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(required(&mut iter, "--dir")?)),
            "--baseline" => baseline = Some(PathBuf::from(required(&mut iter, "--baseline")?)),
            "--output" => output = Some(PathBuf::from(required(&mut iter, "--output")?)),
            "--promote-dir" => {
                promote_dir = Some(PathBuf::from(required(&mut iter, "--promote-dir")?));
            }
            "--rss-peak-mb" => {
                let raw = required(&mut iter, "--rss-peak-mb")?;
                rss_peak_mb = raw
                    .parse::<f64>()
                    .map_err(|_| CliError::Usage(format!("invalid --rss-peak-mb value `{raw}`")))?;
            }
            other => {
                return Err(CliError::Usage(format!("unknown option `{other}`")));
            }
        }
    }

    if output.is_some() && promote_dir.is_some() {
        return Err(CliError::Usage(
            "--output and --promote-dir are mutually exclusive".to_owned(),
        ));
    }

    let dir = dir.ok_or_else(|| CliError::Usage("missing required --dir".to_owned()))?;
    Ok(Plan {
        dir,
        baseline,
        output,
        promote_dir,
        rss_peak_mb,
    })
}

fn required<I: Iterator<Item = String>>(
    iter: &mut I,
    option: &'static str,
) -> Result<String, CliError> {
    iter.next()
        .ok_or_else(|| CliError::Usage(format!("missing value after {option}")))
}

// ---------------------------------------------------------------------------
// Loading + parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Default, Clone)]
struct Aggregate {
    artillery: Vec<ArtilleryRow>,
    ban_http: Vec<BanHttpRow>,
    beir: Vec<BeirRow>,
    rss: Vec<RssRow>,
    pair: Vec<PairRow>,
    unknown_files: Vec<String>,
}

#[derive(Debug, Clone)]
struct ArtilleryRow {
    label: String,
    engine: String,
    workload: String,
    p50_ms: f64,
    p95_ms: f64,
    p99_ms: f64,
    max_ms: f64,
    issued: u64,
    errors: u64,
}

#[derive(Debug, Clone)]
struct BanHttpRow {
    engine: String,
    operation: String,
    status: u64,
    iterations: u64,
    p50_us: u64,
    p95_us: u64,
    p99_us: u64,
    max_us: u64,
    errors: u64,
    docs_per_second: Option<f64>,
    bytes_per_second: Option<f64>,
}

#[derive(Debug, Clone)]
struct BeirRow {
    engine: String,
    workload: String,
    ndcg_10: f64,
    recall_10: f64,
    queries_processed: u64,
    total_queries: u64,
    bulk_ms: f64,
    lucene_baseline_ndcg_10: Option<f64>,
}

#[derive(Debug, Clone)]
struct RssRow {
    label: String,
    engine: String,
    workload: String,
    peak_mb: f64,
    final_mb: f64,
}

#[derive(Debug, Clone)]
struct PairRow {
    workload: String,
    surch_out: String,
    os_out: String,
}

fn load_directory(dir: &Path) -> Result<Aggregate, CliError> {
    let mut agg = Aggregate::default();
    if !dir.exists() {
        return Err(CliError::Io(format!(
            "directory does not exist: {}",
            dir.display()
        )));
    }
    if !dir.is_dir() {
        return Err(CliError::Io(format!("not a directory: {}", dir.display())));
    }

    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| CliError::Io(format!("read_dir {}: {e}", dir.display())))?
        .filter_map(|res| res.ok().map(|entry| entry.path()))
        .filter(|path| path.is_file())
        .filter(|path| {
            matches!(
                path.extension().and_then(|s| s.to_str()),
                Some("json" | "out")
            )
        })
        .collect();
    entries.sort();

    for path in entries {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_owned();
        let raw = match fs::read_to_string(&path) {
            Ok(value) => value,
            Err(error) => {
                agg.unknown_files
                    .push(format!("{}: io error {error}", path.display()));
                continue;
            }
        };
        if matches!(path.extension().and_then(|s| s.to_str()), Some("out")) {
            if let Some(row) = parse_beir_text_output(&raw, &stem) {
                agg.beir.push(row);
            } else {
                agg.unknown_files
                    .push(format!("{}: unrecognised text report", path.display()));
            }
            continue;
        }
        let value: Value = match serde_json::from_str(&raw) {
            Ok(v) => v,
            Err(_) => {
                agg.unknown_files
                    .push(format!("{}: not valid JSON", path.display()));
                continue;
            }
        };
        let schema = value.get("schema").and_then(|s| s.as_str()).unwrap_or("");
        match schema {
            SCHEMA_ARTILLERY => {
                if let Some(row) = parse_artillery(&value, &stem) {
                    agg.artillery.push(row);
                } else {
                    agg.unknown_files
                        .push(format!("{}: malformed artillery report", path.display()));
                }
            }
            SCHEMA_BAN_HTTP => {
                if let Some(rows) = parse_ban_http(&value) {
                    agg.ban_http.extend(rows);
                } else {
                    agg.unknown_files
                        .push(format!("{}: malformed BAN HTTP report", path.display()));
                }
            }
            SCHEMA_RSS => {
                if let Some(row) = parse_rss(&value, &stem) {
                    agg.rss.push(row);
                } else {
                    agg.unknown_files
                        .push(format!("{}: malformed rss report", path.display()));
                }
            }
            SCHEMA_PAIR => {
                if let Some(row) = parse_pair(&value) {
                    agg.pair.push(row);
                } else {
                    agg.unknown_files
                        .push(format!("{}: malformed pair report", path.display()));
                }
            }
            "" => {
                agg.unknown_files
                    .push(format!("{}: no schema field", path.display()));
            }
            other => {
                agg.unknown_files
                    .push(format!("{}: unrecognised schema `{other}`", path.display()));
            }
        }
    }

    // Stable order: artillery rows by engine then workload.
    agg.artillery
        .sort_by(|a, b| a.engine.cmp(&b.engine).then(a.workload.cmp(&b.workload)));
    agg.ban_http
        .sort_by(|a, b| a.engine.cmp(&b.engine).then(a.operation.cmp(&b.operation)));
    agg.beir
        .sort_by(|a, b| a.engine.cmp(&b.engine).then(a.workload.cmp(&b.workload)));
    agg.rss
        .sort_by(|a, b| a.engine.cmp(&b.engine).then(a.workload.cmp(&b.workload)));
    agg.pair.sort_by(|a, b| a.workload.cmp(&b.workload));

    Ok(agg)
}

fn parse_artillery(value: &Value, stem: &str) -> Option<ArtilleryRow> {
    let global = value.get("global")?;
    let p50_ms = global.get("p50_ms")?.as_f64()?;
    let p95_ms = global.get("p95_ms")?.as_f64()?;
    let p99_ms = global.get("p99_ms")?.as_f64()?;
    let max_ms = global.get("max_ms")?.as_f64()?;
    let issued = global.get("issued")?.as_u64()?;
    let errors = global.get("errors")?.as_u64()?;

    let url = value.get("url").and_then(|v| v.as_str()).unwrap_or("");
    let index = value.get("index").and_then(|v| v.as_str()).unwrap_or("");
    let engine = engine_from_url_or_stem(url, stem);
    let workload = if index.is_empty() {
        workload_from_stem(stem)
    } else {
        index.to_owned()
    };

    Some(ArtilleryRow {
        label: stem.to_owned(),
        engine,
        workload,
        p50_ms,
        p95_ms,
        p99_ms,
        max_ms,
        issued,
        errors,
    })
}

fn parse_ban_http(value: &Value) -> Option<Vec<BanHttpRow>> {
    let engines = value.get("engines")?.as_array()?;
    let mut rows = Vec::new();
    for engine in engines {
        let engine_name = normalize_engine_name(engine.get("name")?.as_str()?);
        let metrics = engine.get("metrics")?.as_array()?;
        for metric in metrics {
            let latency = metric.get("latency_us")?;
            rows.push(BanHttpRow {
                engine: engine_name.clone(),
                operation: metric.get("operation")?.as_str()?.to_owned(),
                status: metric.get("status")?.as_u64()?,
                iterations: metric.get("iterations")?.as_u64()?,
                p50_us: latency.get("p50")?.as_u64()?,
                p95_us: latency.get("p95")?.as_u64()?,
                p99_us: latency.get("p99")?.as_u64()?,
                max_us: latency.get("max")?.as_u64()?,
                errors: metric.get("error_count")?.as_u64()?,
                docs_per_second: metric.get("docs_per_second").and_then(Value::as_f64),
                bytes_per_second: metric.get("bytes_per_second").and_then(Value::as_f64),
            });
        }
    }
    Some(rows)
}

fn parse_beir_text_output(raw: &str, stem: &str) -> Option<BeirRow> {
    let mut engine: Option<String> = None;
    let mut workload: Option<String> = None;
    let mut bulk_ms: Option<f64> = None;
    let mut queries_processed: Option<u64> = None;
    let mut total_queries: Option<u64> = None;
    let mut ndcg_10: Option<f64> = None;
    let mut recall_10: Option<f64> = None;
    let mut lucene_baseline_ndcg_10: Option<f64> = None;

    for line in raw.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("## ") {
            let mut tokens = rest.split_whitespace();
            if let Some(name) = tokens.next() {
                if let Some(base) = name.strip_suffix("-ndcg") {
                    workload = Some(base.to_owned());
                }
            }
            for token in tokens {
                if let Some(label) = token.strip_prefix("label=") {
                    engine = Some(normalize_engine_name(label));
                }
            }
            continue;
        }

        if line.starts_with("url=") {
            if let Some(value) = key_value_token(line, "bulk_ms") {
                bulk_ms = value.parse().ok();
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("queries_processed=") {
            let mut parts = rest.split_whitespace();
            queries_processed = parts.next().and_then(|value| value.parse().ok());
            if let Some((_, after)) = line.split_once("(out of ") {
                total_queries = after
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok());
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("NDCG@10 = ") {
            ndcg_10 = rest.split_whitespace().next().and_then(|v| v.parse().ok());
            if let Some((_, after)) = rest.split_once("baseline:") {
                lucene_baseline_ndcg_10 = after
                    .trim_end_matches(')')
                    .split_whitespace()
                    .next()
                    .and_then(|value| value.parse().ok());
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("Recall@10 = ") {
            recall_10 = rest.split_whitespace().next().and_then(|v| v.parse().ok());
        }
    }

    Some(BeirRow {
        engine: engine.unwrap_or_else(|| normalize_engine_name(&engine_from_stem(stem))),
        workload: workload.unwrap_or_else(|| workload_from_stem(stem)),
        ndcg_10: ndcg_10?,
        recall_10: recall_10?,
        queries_processed: queries_processed?,
        total_queries: total_queries?,
        bulk_ms: bulk_ms?,
        lucene_baseline_ndcg_10,
    })
}

fn parse_rss(value: &Value, stem: &str) -> Option<RssRow> {
    let peak_mb = value.get("peak_mb").and_then(|v| v.as_f64())?;
    let final_mb = value.get("final_mb").and_then(|v| v.as_f64())?;
    let engine = engine_from_stem(stem);
    let workload = workload_from_stem(stem);
    Some(RssRow {
        label: stem.to_owned(),
        engine,
        workload,
        peak_mb,
        final_mb,
    })
}

fn parse_pair(value: &Value) -> Option<PairRow> {
    let workload = value.get("workload")?.as_str()?.to_owned();
    let surch_out = value
        .get("surch_out")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    let os_out = value
        .get("os_out")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_owned();
    Some(PairRow {
        workload,
        surch_out,
        os_out,
    })
}

fn normalize_engine_name(name: &str) -> String {
    match name.to_ascii_lowercase().as_str() {
        "es" | "os" | "opensearch" => "elasticsearch".to_owned(),
        other => other.to_owned(),
    }
}

fn key_value_token<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let prefix = format!("{key}=");
    line.split_whitespace()
        .find_map(|token| token.strip_prefix(&prefix))
}

/// Best-effort engine detection from URL (port 7700 = surch, 9200 = Elasticsearch),
/// falling back to the filename stem.
fn engine_from_url_or_stem(url: &str, stem: &str) -> String {
    if url.contains(":7700") {
        return "surch".to_owned();
    }
    if url.contains(":9200") {
        return "elasticsearch".to_owned();
    }
    normalize_engine_name(&engine_from_stem(stem))
}

fn engine_from_stem(stem: &str) -> String {
    let lower = stem.to_ascii_lowercase();
    // Look for an "-os" or "-surch" suffix or substring.
    if lower.contains("opensearch") {
        return "opensearch".to_owned();
    }
    if lower.contains("-os") || lower.ends_with("os") || lower.contains("-os-") {
        // Be cautious — only if the token "os" is preceded by '-' or end.
        for token in lower.split(['-', '_', '.']) {
            if token == "os" || token == "opensearch" {
                return "opensearch".to_owned();
            }
        }
    }
    for token in lower.split(['-', '_', '.']) {
        if token == "surch" {
            return "surch".to_owned();
        }
    }
    "unknown".to_owned()
}

/// Strips engine tokens from a filename stem to produce a workload label.
fn workload_from_stem(stem: &str) -> String {
    let tokens: Vec<&str> = stem
        .split(['-', '_'])
        .filter(|t| {
            let lower = t.to_ascii_lowercase();
            !matches!(lower.as_str(), "surch" | "os" | "opensearch" | "rss")
        })
        .collect();
    if tokens.is_empty() {
        stem.to_owned()
    } else {
        tokens.join("-")
    }
}

// ---------------------------------------------------------------------------
// SLO + regression checks
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct SloCheck {
    name: String,
    detail: String,
    passed: bool,
}

fn evaluate_slo(agg: &Aggregate, rss_peak_mb: f64) -> Vec<SloCheck> {
    let mut checks = Vec::new();
    for row in &agg.artillery {
        // The p95 / max latency SLOs are a *Surch* performance budget.
        // The reference engine (OpenSearch/Elasticsearch) is reported
        // for comparison but must not gate the Job: on a large corpus
        // (e.g. trec-covid-latency on 171k) the JVM engine legitimately
        // exceeds the matchID-tuned 200 ms / 500 ms thresholds, which
        // would otherwise fail the run. Gate latency on `surch` only.
        if row.engine == "surch" {
            let passed = row.p95_ms <= SLO_ARTILLERY_P95_MS;
            checks.push(SloCheck {
                name: format!(
                    "Surch artillery p95 ≤ {} ms [{}]",
                    SLO_ARTILLERY_P95_MS, row.label
                ),
                detail: format!("observed p95 = {:.1} ms", row.p95_ms),
                passed,
            });

            let passed = row.max_ms <= SLO_ARTILLERY_MAX_MS;
            checks.push(SloCheck {
                name: format!(
                    "Surch artillery max ≤ {} ms [{}]",
                    SLO_ARTILLERY_MAX_MS, row.label
                ),
                detail: format!("observed max = {:.1} ms", row.max_ms),
                passed,
            });
        }

        // Error rate gates BOTH engines: a non-zero error rate means the
        // benchmark run is invalid (the latency comparison is moot), so
        // it must fail closed regardless of engine.
        let error_rate = if row.issued == 0 {
            0.0
        } else {
            (row.errors as f64) / (row.issued as f64) * 100.0
        };
        let passed = error_rate <= SLO_ARTILLERY_ERROR_RATE_PCT;
        checks.push(SloCheck {
            name: format!(
                "artillery error rate ≤ {} % [{}]",
                SLO_ARTILLERY_ERROR_RATE_PCT, row.label
            ),
            detail: format!(
                "observed = {:.3} % ({} errors / {} issued)",
                error_rate, row.errors, row.issued
            ),
            passed,
        });
    }
    // RSS peak SLO is a *Surch* memory target on the artillery
    // workload. It must not gate the reference engine: OpenSearch /
    // Elasticsearch run a JVM with `-Xmx1g`, so their RSS legitimately
    // exceeds 1 GiB and is irrelevant to Surch's footprint budget.
    // Reference-engine RSS rows are still rendered in the human report
    // for comparison, but only the `surch` engine gates the SLO. The
    // budget `rss_peak_mb` is per-workload (passed via `--rss-peak-mb`):
    // the INSEE 10k job uses the default 1024 MB, while the 171k
    // trec-covid-latency job declares a corpus-appropriate budget, since
    // Surch's resident set scales with the indexed corpus and ~2 GiB is
    // expected there (not a regression).
    for row in &agg.rss {
        let is_artillery = row.workload.contains("insee") || row.label.contains("art");
        if is_artillery && row.engine == "surch" {
            let passed = row.peak_mb <= rss_peak_mb;
            checks.push(SloCheck {
                name: format!(
                    "Surch RSS peak ≤ {} MB (artillery) [{}]",
                    rss_peak_mb, row.label
                ),
                detail: format!("observed peak = {:.1} MB", row.peak_mb),
                passed,
            });
        }
    }
    for row in &agg.beir {
        if let Some(target) = beir_ndcg_target(&row.workload) {
            let passed = row.ndcg_10 >= target;
            checks.push(SloCheck {
                name: format!(
                    "BEIR NDCG@10 ≥ {:.2} [{} / {}]",
                    target, row.engine, row.workload
                ),
                detail: format!(
                    "observed NDCG@10 = {:.4}, Recall@10 = {:.4}",
                    row.ndcg_10, row.recall_10
                ),
                passed,
            });
        }
    }
    checks
}

fn beir_ndcg_target(workload: &str) -> Option<f64> {
    match workload {
        "scifact" => Some(SLO_SCIFACT_NDCG_10),
        "trec-covid" => Some(SLO_TREC_COVID_NDCG_10),
        _ => None,
    }
}

#[derive(Debug, Clone)]
struct RegressionCheck {
    name: String,
    detail: String,
    passed: bool,
}

fn evaluate_regressions(current: &Aggregate, baseline: &Aggregate) -> Vec<RegressionCheck> {
    let mut checks = Vec::new();
    // Pair artillery rows by (engine, workload).
    for cur in &current.artillery {
        let Some(base) = baseline
            .artillery
            .iter()
            .find(|b| b.engine == cur.engine && b.workload == cur.workload)
        else {
            continue;
        };
        if base.p95_ms <= 0.0 {
            continue;
        }
        let delta_pct = (cur.p95_ms - base.p95_ms) / base.p95_ms * 100.0;
        let passed = delta_pct <= REGRESSION_P95_PCT;
        checks.push(RegressionCheck {
            name: format!(
                "p95 regression ≤ {} % [{} / {}]",
                REGRESSION_P95_PCT, cur.engine, cur.workload
            ),
            detail: format!(
                "baseline p95 = {:.1} ms, current p95 = {:.1} ms, delta = {:+.1} %",
                base.p95_ms, cur.p95_ms, delta_pct
            ),
            passed,
        });
    }
    for cur in &current.rss {
        let Some(base) = baseline
            .rss
            .iter()
            .find(|b| b.engine == cur.engine && b.workload == cur.workload)
        else {
            continue;
        };
        if base.peak_mb <= 0.0 {
            continue;
        }
        let delta_pct = (cur.peak_mb - base.peak_mb) / base.peak_mb * 100.0;
        let passed = delta_pct <= REGRESSION_RSS_PCT;
        checks.push(RegressionCheck {
            name: format!(
                "RSS regression ≤ {} % [{} / {}]",
                REGRESSION_RSS_PCT, cur.engine, cur.workload
            ),
            detail: format!(
                "baseline peak = {:.1} MB, current peak = {:.1} MB, delta = {:+.1} %",
                base.peak_mb, cur.peak_mb, delta_pct
            ),
            passed,
        });
    }
    checks
}

// ---------------------------------------------------------------------------
// Markdown rendering
// ---------------------------------------------------------------------------

fn render_markdown(
    sha: &str,
    baseline_sha: Option<&str>,
    generated_at: &str,
    current: &Aggregate,
    baseline: Option<&Aggregate>,
    slo: &[SloCheck],
    regressions: &[RegressionCheck],
) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Surch bench summary {sha}\n\n"));
    out.push_str(&format!("Generated {generated_at}.\n\n"));
    if let Some(base) = baseline_sha {
        out.push_str(&format!("Baseline: {base}.\n\n"));
    }

    // Artillery section.
    out.push_str(&format!("## Artillery results ({SCHEMA_ARTILLERY})\n\n"));
    out.push_str("| Engine | Workload | p50 ms | p95 ms | p99 ms | max ms | issued | errors |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---:|---:|\n");
    if current.artillery.is_empty() {
        out.push_str("| _no data_ |  |  |  |  |  |  |  |\n");
    } else {
        for row in &current.artillery {
            out.push_str(&format!(
                "| {} | {} | {:.1} | {:.1} | {:.1} | {:.1} | {} | {} |\n",
                row.engine,
                row.workload,
                row.p50_ms,
                row.p95_ms,
                row.p99_ms,
                row.max_ms,
                row.issued,
                row.errors,
            ));
        }
    }
    out.push('\n');

    // BAN HTTP section.
    out.push_str(&format!("## BAN HTTP results ({SCHEMA_BAN_HTTP})\n\n"));
    out.push_str("| Engine | Operation | status | iterations | p50 us | p95 us | p99 us | max us | errors | docs/s | bytes/s |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|\n");
    if current.ban_http.is_empty() {
        out.push_str("| _no data_ |  |  |  |  |  |  |  |  |  |  |\n");
    } else {
        for row in &current.ban_http {
            out.push_str(&format!(
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
                row.engine,
                row.operation,
                row.status,
                row.iterations,
                row.p50_us,
                row.p95_us,
                row.p99_us,
                row.max_us,
                row.errors,
                format_optional_rate(row.docs_per_second),
                format_optional_rate(row.bytes_per_second),
            ));
        }
    }
    out.push('\n');

    // BEIR section.
    out.push_str("## BEIR retrieval results\n\n");
    out.push_str("| Engine | Workload | NDCG@10 | Recall@10 | processed | total | bulk ms | Lucene baseline NDCG@10 |\n");
    out.push_str("|---|---|---:|---:|---:|---:|---:|---:|\n");
    if current.beir.is_empty() {
        out.push_str("| _no data_ |  |  |  |  |  |  |  |\n");
    } else {
        for row in &current.beir {
            out.push_str(&format!(
                "| {} | {} | {:.4} | {:.4} | {} | {} | {:.1} | {} |\n",
                row.engine,
                row.workload,
                row.ndcg_10,
                row.recall_10,
                row.queries_processed,
                row.total_queries,
                row.bulk_ms,
                row.lucene_baseline_ndcg_10
                    .map_or_else(|| "-".to_owned(), |value| format!("{value:.4}")),
            ));
        }
    }
    out.push('\n');

    // RSS section.
    out.push_str(&format!("## RSS samples ({SCHEMA_RSS})\n\n"));
    out.push_str("| Engine | Workload | peak MB | final MB |\n");
    out.push_str("|---|---|---:|---:|\n");
    if current.rss.is_empty() {
        out.push_str("| _no data_ |  |  |  |\n");
    } else {
        for row in &current.rss {
            out.push_str(&format!(
                "| {} | {} | {:.1} | {:.1} |\n",
                row.engine, row.workload, row.peak_mb, row.final_mb
            ));
        }
    }
    out.push('\n');

    // Pair section (optional, informational only).
    if !current.pair.is_empty() {
        out.push_str(&format!("## Pair envelopes ({SCHEMA_PAIR})\n\n"));
        out.push_str("| Workload | Surch output | Elasticsearch output |\n");
        out.push_str("|---|---|---|\n");
        for row in &current.pair {
            out.push_str(&format!(
                "| {} | {} | {} |\n",
                row.workload, row.surch_out, row.os_out
            ));
        }
        out.push('\n');
    }

    // SLO section.
    out.push_str("## SLO checks\n\n");
    if slo.is_empty() {
        out.push_str("- _no SLO-applicable data_\n");
    } else {
        for check in slo {
            let badge = if check.passed { "PASS" } else { "FAIL" };
            out.push_str(&format!(
                "- {} : {} ({})\n",
                check.name, badge, check.detail
            ));
        }
    }
    out.push('\n');

    // Regression section.
    if baseline.is_some() {
        out.push_str("## Regression vs baseline\n\n");
        if regressions.is_empty() {
            out.push_str("- _no comparable rows between current and baseline_\n");
        } else {
            for check in regressions {
                let badge = if check.passed { "PASS" } else { "FAIL" };
                out.push_str(&format!(
                    "- {} : {} ({})\n",
                    check.name, badge, check.detail
                ));
            }
        }
        out.push('\n');
    }

    if !current.unknown_files.is_empty() {
        out.push_str("## Skipped files\n\n");
        for entry in &current.unknown_files {
            out.push_str(&format!("- {entry}\n"));
        }
        out.push('\n');
    }

    out
}

fn render_json_summary(
    sha: &str,
    baseline_sha: Option<&str>,
    generated_at: &str,
    current: &Aggregate,
    slo: &[SloCheck],
    regressions: &[RegressionCheck],
    exit_ok: bool,
) -> String {
    let artillery: Vec<Value> = current
        .artillery
        .iter()
        .map(|row| {
            serde_json::json!({
                "label": row.label,
                "engine": row.engine,
                "workload": row.workload,
                "latency_ms": {
                    "p50": row.p50_ms,
                    "p95": row.p95_ms,
                    "p99": row.p99_ms,
                    "max": row.max_ms,
                },
                "issued": row.issued,
                "errors": row.errors,
            })
        })
        .collect();
    let rss: Vec<Value> = current
        .rss
        .iter()
        .map(|row| {
            serde_json::json!({
                "label": row.label,
                "engine": row.engine,
                "workload": row.workload,
                "peak_mb": row.peak_mb,
                "final_mb": row.final_mb,
            })
        })
        .collect();
    let ban_http: Vec<Value> = current
        .ban_http
        .iter()
        .map(|row| {
            serde_json::json!({
                "engine": row.engine,
                "operation": row.operation,
                "status": row.status,
                "iterations": row.iterations,
                "latency_us": {
                    "p50": row.p50_us,
                    "p95": row.p95_us,
                    "p99": row.p99_us,
                    "max": row.max_us,
                },
                "errors": row.errors,
                "docs_per_second": row.docs_per_second,
                "bytes_per_second": row.bytes_per_second,
            })
        })
        .collect();
    let beir: Vec<Value> = current
        .beir
        .iter()
        .map(|row| {
            serde_json::json!({
                "engine": row.engine,
                "workload": row.workload,
                "ndcg_10": row.ndcg_10,
                "recall_10": row.recall_10,
                "queries_processed": row.queries_processed,
                "total_queries": row.total_queries,
                "bulk_ms": row.bulk_ms,
                "lucene_baseline_ndcg_10": row.lucene_baseline_ndcg_10,
            })
        })
        .collect();
    let pair: Vec<Value> = current
        .pair
        .iter()
        .map(|row| {
            serde_json::json!({
                "workload": row.workload,
                "surch_out": row.surch_out,
                "os_out": row.os_out,
            })
        })
        .collect();
    let slo_checks: Vec<Value> = slo
        .iter()
        .map(|check| {
            serde_json::json!({
                "name": check.name,
                "detail": check.detail,
                "passed": check.passed,
            })
        })
        .collect();
    let regression_checks: Vec<Value> = regressions
        .iter()
        .map(|check| {
            serde_json::json!({
                "name": check.name,
                "detail": check.detail,
                "passed": check.passed,
            })
        })
        .collect();

    serde_json::to_string_pretty(&serde_json::json!({
        "schema": SCHEMA_SUMMARY,
        "sha": sha,
        "baseline_sha": baseline_sha,
        "generated_at": generated_at,
        "artillery": artillery,
        "ban_http": ban_http,
        "beir": beir,
        "rss": rss,
        "pair": pair,
        "slo_checks": slo_checks,
        "regression_checks": regression_checks,
        "unknown_files": current.unknown_files,
        "verdict": {
            "slo_ok": slo.iter().all(|check| check.passed),
            "regression_ok": regressions.iter().all(|check| check.passed),
            "exit_ok": exit_ok,
        }
    }))
    .expect("summary json serialization should not fail")
}

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

fn format_optional_rate(value: Option<f64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| format!("{value:.1}"))
}

fn infer_sha(dir: &Path) -> String {
    dir.file_name()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn write_output(path: &Path, content: &str) -> Result<(), CliError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent).map_err(|e| {
                CliError::Io(format!(
                    "failed to create output directory `{}`: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    fs::write(path, content)
        .map_err(|e| CliError::Io(format!("failed to write `{}`: {e}", path.display())))?;
    Ok(())
}

fn print_help() {
    println!("surch-demo bench_report");
    println!(
        "Aggregates JSON envelopes under target/bench-reports/<sha>/ into Markdown + JSON summaries."
    );
    println!();
    println!("USAGE:");
    println!("  bench_report --dir <DIR> [--baseline <DIR>] [--output <FILE>]");
    println!("  bench_report --dir <DIR> [--baseline <DIR>] --promote-dir <DIR>");
    println!();
    println!("OPTIONS:");
    println!("  --dir <DIR>         directory containing *.json reports to aggregate (required)");
    println!("  --baseline <DIR>    optional baseline directory for regression detection");
    println!("  --output <FILE>     output Markdown path (default: <DIR>/summary.md)");
    println!("  --promote-dir <DIR> write promoted README.md and summary.json under <DIR>");
    println!(
        "  --rss-peak-mb <MB>  Surch RSS peak SLO budget in MB (default {SLO_RSS_PEAK_MB}; raise for large-corpus jobs)"
    );
    println!("  -h, --help          print this help");
    println!();
    println!("REPORT CONTRACT:");
    println!("  Human readers use summary.md or promoted README.md.");
    println!("  Agents and CI validate summary.json.");
    println!();
    println!("RECOGNISED SCHEMAS:");
    println!("  {SCHEMA_ARTILLERY}");
    println!("  {SCHEMA_BAN_HTTP}");
    println!("  {SCHEMA_RSS}");
    println!("  {SCHEMA_PAIR}");
    println!("  BEIR .out text reports from scripts/bench/*-ndcg.sh");
    println!("  {SCHEMA_SUMMARY} (written as summary.json)");
    println!();
    println!("SLO THRESHOLDS:");
    println!("  Surch artillery p95 ≤ {SLO_ARTILLERY_P95_MS} ms (Surch engine only)");
    println!("  Surch artillery max ≤ {SLO_ARTILLERY_MAX_MS} ms (Surch engine only)");
    println!("  artillery error rate ≤ {SLO_ARTILLERY_ERROR_RATE_PCT} %");
    println!(
        "  Surch RSS peak ≤ {SLO_RSS_PEAK_MB} MB (artillery, Surch only; per-workload via --rss-peak-mb)"
    );
    println!("  SciFact NDCG@10 ≥ {SLO_SCIFACT_NDCG_10}");
    println!("  TREC-COVID NDCG@10 ≥ {SLO_TREC_COVID_NDCG_10}");
    println!();
    println!("REGRESSION THRESHOLDS vs --baseline:");
    println!("  p95 +{REGRESSION_P95_PCT} %");
    println!("  RSS +{REGRESSION_RSS_PCT} %");
    println!();
    println!("Exit code 0 iff every SLO passes and no regression breaches its threshold.");
}

#[derive(Debug)]
enum CliError {
    Usage(String),
    Io(String),
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Usage(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

// ---------------------------------------------------------------------------
// Unit tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_from_url_detects_surch_and_elasticsearch() {
        assert_eq!(
            engine_from_url_or_stem("http://127.0.0.1:7700", "art"),
            "surch"
        );
        assert_eq!(
            engine_from_url_or_stem("http://127.0.0.1:9200", "art"),
            "elasticsearch"
        );
    }

    #[test]
    fn engine_from_stem_handles_common_naming() {
        assert_eq!(engine_from_stem("art-surch"), "surch");
        assert_eq!(engine_from_stem("art-os"), "opensearch");
        assert_eq!(engine_from_stem("art-opensearch"), "opensearch");
        assert_eq!(engine_from_stem("custom-bench"), "unknown");
    }

    #[test]
    fn normalize_engine_name_maps_legacy_os_labels_to_elasticsearch() {
        assert_eq!(normalize_engine_name("opensearch"), "elasticsearch");
        assert_eq!(normalize_engine_name("os"), "elasticsearch");
        assert_eq!(normalize_engine_name("surch"), "surch");
    }

    #[test]
    fn workload_from_stem_strips_engine_tokens() {
        assert_eq!(workload_from_stem("art-surch"), "art");
        assert_eq!(workload_from_stem("insee25k-os"), "insee25k");
        assert_eq!(workload_from_stem("rss-art-surch"), "art");
    }

    #[test]
    fn parse_artillery_extracts_global_summary() {
        let value: Value = serde_json::from_str(
            r#"{
                "schema": "surch.bench.artillery.v1",
                "url": "http://127.0.0.1:7700",
                "index": "deces_25k",
                "global": {
                    "issued": 100, "errors": 0,
                    "p50_ms": 4.0, "p95_ms": 20.0, "p99_ms": 30.0, "max_ms": 42.0
                }
            }"#,
        )
        .unwrap();
        let row = parse_artillery(&value, "art-surch").expect("artillery should parse");
        assert_eq!(row.engine, "surch");
        assert_eq!(row.workload, "deces_25k");
        assert_eq!(row.p95_ms, 20.0);
        assert_eq!(row.issued, 100);
    }

    #[test]
    fn parse_ban_http_extracts_engine_operation_rows() {
        let value: Value = serde_json::from_str(
            r#"{
                "schema": "surch.bench.ban_http.v1",
                "engines": [
                    {
                        "name": "opensearch",
                        "metrics": [
                            {
                                "operation": "bulk_ingest",
                                "status": 200,
                                "iterations": 1,
                                "docs_per_second": 2000.0,
                                "bytes_per_second": 600000.0,
                                "error_count": 0,
                                "latency_us": {
                                    "p50": 300,
                                    "p95": 500,
                                    "p99": 500,
                                    "max": 500
                                }
                            }
                        ]
                    }
                ]
            }"#,
        )
        .unwrap();
        let rows = parse_ban_http(&value).expect("BAN HTTP should parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].engine, "elasticsearch");
        assert_eq!(rows[0].operation, "bulk_ingest");
        assert_eq!(rows[0].p95_us, 500);
        assert_eq!(rows[0].docs_per_second, Some(2000.0));
    }

    #[test]
    fn parse_beir_text_output_extracts_quality_metrics() {
        let row = parse_beir_text_output(
            r#"## scifact-ndcg label=surch  2026-05-18T12:00:00Z
url=http://127.0.0.1:7700 bulk_ms=1234.5
queries_processed=300 (out of 300 unique test qids)
NDCG@10 = 0.6576 (Lucene/Anserini baseline: 0.688)
Recall@10 = 0.8100
"#,
            "scifact-surch",
        )
        .expect("BEIR text output should parse");
        assert_eq!(row.engine, "surch");
        assert_eq!(row.workload, "scifact");
        assert_eq!(row.queries_processed, 300);
        assert_eq!(row.total_queries, 300);
        assert_eq!(row.bulk_ms, 1234.5);
        assert_eq!(row.ndcg_10, 0.6576);
        assert_eq!(row.recall_10, 0.8100);
        assert_eq!(row.lucene_baseline_ndcg_10, Some(0.688));
    }

    #[test]
    fn evaluate_slo_rss_gates_surch_only_not_reference_engine() {
        // Regression guard: once the RSS sampler was wired into
        // insee-bench, bench_report started seeing rss-art-os.json for
        // the JVM reference engine, whose `-Xmx1g` heap legitimately
        // exceeds the 1024 MB Surch SLO. The SLO must gate the `surch`
        // engine only, otherwise the Job fails closed at teardown.
        let agg = Aggregate {
            rss: vec![
                RssRow {
                    label: "rss-art-surch".into(),
                    engine: "surch".into(),
                    workload: "art".into(),
                    peak_mb: 300.0, // well under the 1024 MB SLO
                    final_mb: 280.0,
                },
                RssRow {
                    label: "rss-art-os".into(),
                    engine: "opensearch".into(),
                    workload: "art".into(),
                    peak_mb: 1466.0, // JVM heap > 1024 MB, must NOT gate
                    final_mb: 1466.0,
                },
            ],
            ..Aggregate::default()
        };
        let checks = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        let rss_checks: Vec<_> = checks
            .iter()
            .filter(|c| c.name.contains("RSS peak"))
            .collect();
        assert_eq!(
            rss_checks.len(),
            1,
            "only the surch RSS row should produce an SLO check"
        );
        assert!(
            rss_checks[0].name.contains("rss-art-surch"),
            "the single RSS check must be the surch one"
        );
        assert!(rss_checks[0].passed, "surch peak 300 MB is within SLO");
    }

    #[test]
    fn evaluate_slo_flags_surch_rss_over_budget() {
        let agg = Aggregate {
            rss: vec![RssRow {
                label: "rss-art-surch".into(),
                engine: "surch".into(),
                workload: "art".into(),
                peak_mb: 1500.0, // breaches the 1024 MB Surch SLO
                final_mb: 1400.0,
            }],
            ..Aggregate::default()
        };
        let checks = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        let rss = checks
            .iter()
            .find(|c| c.name.contains("RSS peak"))
            .expect("surch RSS check should exist");
        assert!(!rss.passed, "surch peak 1500 MB must fail the SLO");
    }

    #[test]
    fn evaluate_slo_rss_budget_is_per_workload() {
        // Regression guard for trec-covid-latency: on the 171k corpus
        // Surch's resident set legitimately reaches ~2.1 GiB, far above
        // the INSEE 10k default of 1024 MB. The large-corpus job passes a
        // higher `--rss-peak-mb`; the same peak must pass under that
        // budget and fail under the default.
        let agg = Aggregate {
            rss: vec![RssRow {
                label: "rss-art-surch".into(),
                engine: "surch".into(),
                workload: "art".into(),
                peak_mb: 2168.0,
                final_mb: 1400.0,
            }],
            ..Aggregate::default()
        };
        let under_default = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        assert!(
            !under_default
                .iter()
                .find(|c| c.name.contains("RSS peak"))
                .expect("surch RSS check should exist")
                .passed,
            "2168 MB must fail the default 1024 MB budget"
        );
        let under_raised = evaluate_slo(&agg, 2560.0);
        let raised = under_raised
            .iter()
            .find(|c| c.name.contains("RSS peak"))
            .expect("surch RSS check should exist");
        assert!(raised.passed, "2168 MB must pass a 2560 MB budget");
        assert!(
            raised.name.contains("2560"),
            "the check name must reflect the active budget"
        );
    }

    #[test]
    fn evaluate_slo_artillery_latency_gates_surch_only() {
        // Regression guard for trec-covid-latency: on a large corpus the
        // JVM reference engine legitimately exceeds the 200/500 ms
        // matchID-tuned thresholds. Its latency must NOT produce an SLO
        // check (would fail the Job); only Surch's latency gates.
        let agg = Aggregate {
            artillery: vec![
                ArtilleryRow {
                    label: "art-surch".into(),
                    engine: "surch".into(),
                    workload: "trec-covid".into(),
                    p50_ms: 0.6,
                    p95_ms: 1.7,
                    p99_ms: 7.4,
                    max_ms: 324.8,
                    issued: 13170,
                    errors: 0,
                },
                ArtilleryRow {
                    label: "art-os".into(),
                    engine: "elasticsearch".into(),
                    workload: "trec-covid".into(),
                    p50_ms: 207.0,
                    p95_ms: 592.0, // > 200 ms SLO, must NOT gate
                    p99_ms: 807.0,
                    max_ms: 1595.0, // > 500 ms SLO, must NOT gate
                    issued: 13170,
                    errors: 0,
                },
            ],
            ..Aggregate::default()
        };
        let checks = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        let latency_checks: Vec<_> = checks
            .iter()
            .filter(|c| c.name.contains("artillery p95") || c.name.contains("artillery max"))
            .collect();
        // Exactly the two Surch latency checks; none for the reference.
        assert_eq!(latency_checks.len(), 2, "only surch latency gates");
        assert!(
            latency_checks
                .iter()
                .all(|c| c.name.contains("[art-surch]") && c.passed),
            "surch latency within SLO, reference engine not gated"
        );
        // Error-rate checks still apply to BOTH engines.
        let err_checks = checks
            .iter()
            .filter(|c| c.name.contains("error rate"))
            .count();
        assert_eq!(err_checks, 2, "error-rate SLO gates both engines");
    }

    #[test]
    fn evaluate_slo_flags_high_p95() {
        let agg = Aggregate {
            artillery: vec![ArtilleryRow {
                label: "art-surch".into(),
                engine: "surch".into(),
                workload: "deces_25k".into(),
                p50_ms: 5.0,
                p95_ms: 250.0, // breaches the 200 ms SLO
                p99_ms: 400.0,
                max_ms: 450.0,
                issued: 1000,
                errors: 0,
            }],
            ..Aggregate::default()
        };
        let checks = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        let p95 = checks
            .iter()
            .find(|c| c.name.starts_with("Surch artillery p95"))
            .expect("p95 check should exist");
        assert!(!p95.passed, "p95=250 should fail");
        let max = checks
            .iter()
            .find(|c| c.name.starts_with("Surch artillery max"))
            .expect("max check should exist");
        assert!(max.passed, "max=450 within 500 ms SLO");
    }

    #[test]
    fn evaluate_slo_flags_beir_ndcg_regression() {
        let agg = Aggregate {
            beir: vec![BeirRow {
                engine: "surch".into(),
                workload: "scifact".into(),
                ndcg_10: 0.60,
                recall_10: 0.80,
                queries_processed: 300,
                total_queries: 300,
                bulk_ms: 1000.0,
                lucene_baseline_ndcg_10: Some(0.688),
            }],
            ..Aggregate::default()
        };
        let checks = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        let ndcg = checks
            .iter()
            .find(|c| c.name.starts_with("BEIR NDCG@10"))
            .expect("BEIR NDCG check should exist");
        assert!(!ndcg.passed, "SciFact NDCG below 0.65 should fail");
    }

    #[test]
    fn evaluate_regressions_detects_p95_regression() {
        let base = Aggregate {
            artillery: vec![ArtilleryRow {
                label: "art-surch".into(),
                engine: "surch".into(),
                workload: "deces_25k".into(),
                p50_ms: 5.0,
                p95_ms: 100.0,
                p99_ms: 150.0,
                max_ms: 200.0,
                issued: 1000,
                errors: 0,
            }],
            ..Aggregate::default()
        };
        let mut head = base.clone();
        head.artillery[0].p95_ms = 130.0; // +30 % regression
        let regressions = evaluate_regressions(&head, &base);
        assert_eq!(regressions.len(), 1);
        assert!(!regressions[0].passed, "+30 % should breach 15 % threshold");
    }

    #[test]
    fn evaluate_regressions_passes_when_within_threshold() {
        let base = Aggregate {
            artillery: vec![ArtilleryRow {
                label: "art-surch".into(),
                engine: "surch".into(),
                workload: "deces_25k".into(),
                p50_ms: 5.0,
                p95_ms: 100.0,
                p99_ms: 150.0,
                max_ms: 200.0,
                issued: 1000,
                errors: 0,
            }],
            ..Aggregate::default()
        };
        let mut head = base.clone();
        head.artillery[0].p95_ms = 110.0; // +10 %, within 15 % budget
        let regressions = evaluate_regressions(&head, &base);
        assert_eq!(regressions.len(), 1);
        assert!(regressions[0].passed);
    }

    #[test]
    fn render_markdown_contains_required_sections() {
        let agg = Aggregate {
            artillery: vec![ArtilleryRow {
                label: "art-surch".into(),
                engine: "surch".into(),
                workload: "deces_25k".into(),
                p50_ms: 5.0,
                p95_ms: 20.0,
                p99_ms: 30.0,
                max_ms: 40.0,
                issued: 100,
                errors: 0,
            }],
            rss: vec![RssRow {
                label: "rss-art-surch".into(),
                engine: "surch".into(),
                workload: "art".into(),
                peak_mb: 256.0,
                final_mb: 220.0,
            }],
            ban_http: vec![BanHttpRow {
                engine: "elasticsearch".into(),
                operation: "bulk_ingest".into(),
                status: 200,
                iterations: 1,
                p50_us: 300,
                p95_us: 500,
                p99_us: 500,
                max_us: 500,
                errors: 0,
                docs_per_second: Some(2000.0),
                bytes_per_second: Some(600000.0),
            }],
            ..Aggregate::default()
        };
        let slo = evaluate_slo(&agg, SLO_RSS_PEAK_MB);
        let md = render_markdown(
            "abc1234",
            None,
            "2026-05-15T10:00:00Z",
            &agg,
            None,
            &slo,
            &[],
        );
        assert!(md.contains("# Surch bench summary abc1234"));
        assert!(md.contains("## Artillery results"));
        assert!(md.contains("## BAN HTTP results"));
        assert!(md.contains("## RSS samples"));
        assert!(md.contains("| elasticsearch | bulk_ingest |"));
        assert!(md.contains("## SLO checks"));
        assert!(!md.contains("## Regression vs baseline"));
    }

    #[test]
    fn render_markdown_includes_regression_section_when_baseline_present() {
        let agg = Aggregate::default();
        let md = render_markdown(
            "head",
            Some("base"),
            "2026-05-15T10:00:00Z",
            &agg,
            Some(&agg),
            &[],
            &[],
        );
        assert!(md.contains("## Regression vs baseline"));
        assert!(md.contains("Baseline: base"));
    }
}
