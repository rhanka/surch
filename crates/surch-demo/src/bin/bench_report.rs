//! Aggregates JSON envelopes under `target/bench-reports/<sha>/` into a
//! single Markdown summary, checks the matchID v1 SLOs, and (optionally)
//! diff's the latest run against a baseline directory.
//!
//! Recognised schemas:
//!   * `surch.bench.artillery.v1` — produced by `artillery_bench`
//!   * `surch.bench.rss.v1`       — produced by `scripts/bench/rss-sample.sh`
//!   * `surch.bench.pair.v1`      — produced by `scripts/bench/run-pair.sh`
//!
//! CLI:
//!   bench_report --dir target/bench-reports/<sha>
//!                [--baseline target/bench-reports/<other_sha>]
//!                [--output target/bench-reports/<sha>/summary.md]
//!
//! Exit code:
//!   0 → all SLOs pass *and* no regression beyond thresholds vs baseline
//!   1 → at least one SLO failed or a regression breached the threshold
//!
//! SLO thresholds (matchID v1):
//!   * artillery p95 ≤ 200 ms
//!   * artillery max ≤ 500 ms
//!   * artillery error rate ≤ 1 %
//!   * RSS peak ≤ 1024 MB on the artillery INSEE 25k workload
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
const SCHEMA_RSS: &str = "surch.bench.rss.v1";
const SCHEMA_PAIR: &str = "surch.bench.pair.v1";

const SLO_ARTILLERY_P95_MS: f64 = 200.0;
const SLO_ARTILLERY_MAX_MS: f64 = 500.0;
const SLO_ARTILLERY_ERROR_RATE_PCT: f64 = 1.0;
const SLO_RSS_PEAK_MB: f64 = 1024.0;

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

    let slo_results = evaluate_slo(&current);
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

    let output_path = plan
        .output
        .clone()
        .unwrap_or_else(|| plan.dir.join("summary.md"));
    write_output(&output_path, &markdown)?;
    println!("wrote {}", output_path.display());

    let slo_ok = slo_results.iter().all(|check| check.passed);
    let regression_ok = regression_results.iter().all(|check| check.passed);
    Ok(slo_ok && regression_ok)
}

#[derive(Debug, Clone)]
struct Plan {
    dir: PathBuf,
    baseline: Option<PathBuf>,
    output: Option<PathBuf>,
}

fn parse_args(args: Vec<String>) -> Result<Plan, CliError> {
    let mut dir: Option<PathBuf> = None;
    let mut baseline: Option<PathBuf> = None;
    let mut output: Option<PathBuf> = None;

    let mut iter = args.into_iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--dir" => dir = Some(PathBuf::from(required(&mut iter, "--dir")?)),
            "--baseline" => baseline = Some(PathBuf::from(required(&mut iter, "--baseline")?)),
            "--output" => output = Some(PathBuf::from(required(&mut iter, "--output")?)),
            other => {
                return Err(CliError::Usage(format!("unknown option `{other}`")));
            }
        }
    }

    let dir = dir.ok_or_else(|| CliError::Usage("missing required --dir".to_owned()))?;
    Ok(Plan {
        dir,
        baseline,
        output,
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
        .filter(|path| matches!(path.extension().and_then(|s| s.to_str()), Some("json")))
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

/// Best-effort engine detection from URL (port 7700 = surch, 9200 = OS),
/// falling back to the filename stem.
fn engine_from_url_or_stem(url: &str, stem: &str) -> String {
    if url.contains(":7700") {
        return "surch".to_owned();
    }
    if url.contains(":9200") {
        return "opensearch".to_owned();
    }
    engine_from_stem(stem)
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

fn evaluate_slo(agg: &Aggregate) -> Vec<SloCheck> {
    let mut checks = Vec::new();
    for row in &agg.artillery {
        let passed = row.p95_ms <= SLO_ARTILLERY_P95_MS;
        checks.push(SloCheck {
            name: format!(
                "artillery p95 ≤ {} ms [{}]",
                SLO_ARTILLERY_P95_MS, row.label
            ),
            detail: format!("observed p95 = {:.1} ms", row.p95_ms),
            passed,
        });

        let passed = row.max_ms <= SLO_ARTILLERY_MAX_MS;
        checks.push(SloCheck {
            name: format!(
                "artillery max ≤ {} ms [{}]",
                SLO_ARTILLERY_MAX_MS, row.label
            ),
            detail: format!("observed max = {:.1} ms", row.max_ms),
            passed,
        });

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
    // RSS peak SLO applies to the INSEE 25k artillery workload if present.
    for row in &agg.rss {
        if row.workload.contains("insee") || row.label.contains("art") {
            let passed = row.peak_mb <= SLO_RSS_PEAK_MB;
            checks.push(SloCheck {
                name: format!(
                    "RSS peak ≤ {} MB (artillery on INSEE 25k) [{}]",
                    SLO_RSS_PEAK_MB, row.label
                ),
                detail: format!("observed peak = {:.1} MB", row.peak_mb),
                passed,
            });
        }
    }
    checks
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
        out.push_str("| Workload | Surch output | OS output |\n");
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

// ---------------------------------------------------------------------------
// Misc helpers
// ---------------------------------------------------------------------------

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
        "Aggregates JSON envelopes under target/bench-reports/<sha>/ into a Markdown summary."
    );
    println!();
    println!("USAGE:");
    println!("  bench_report --dir <DIR> [--baseline <DIR>] [--output <FILE>]");
    println!();
    println!("OPTIONS:");
    println!("  --dir <DIR>         directory containing *.json reports to aggregate (required)");
    println!("  --baseline <DIR>    optional baseline directory for regression detection");
    println!("  --output <FILE>     output Markdown path (default: <DIR>/summary.md)");
    println!("  -h, --help          print this help");
    println!();
    println!("RECOGNISED SCHEMAS:");
    println!("  {SCHEMA_ARTILLERY}");
    println!("  {SCHEMA_RSS}");
    println!("  {SCHEMA_PAIR}");
    println!();
    println!("SLO THRESHOLDS:");
    println!("  artillery p95 ≤ {SLO_ARTILLERY_P95_MS} ms");
    println!("  artillery max ≤ {SLO_ARTILLERY_MAX_MS} ms");
    println!("  artillery error rate ≤ {SLO_ARTILLERY_ERROR_RATE_PCT} %");
    println!("  RSS peak ≤ {SLO_RSS_PEAK_MB} MB (artillery INSEE 25k)");
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
    fn engine_from_url_detects_surch_and_opensearch() {
        assert_eq!(
            engine_from_url_or_stem("http://127.0.0.1:7700", "art"),
            "surch"
        );
        assert_eq!(
            engine_from_url_or_stem("http://127.0.0.1:9200", "art"),
            "opensearch"
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
        let checks = evaluate_slo(&agg);
        let p95 = checks
            .iter()
            .find(|c| c.name.starts_with("artillery p95"))
            .expect("p95 check should exist");
        assert!(!p95.passed, "p95=250 should fail");
        let max = checks
            .iter()
            .find(|c| c.name.starts_with("artillery max"))
            .expect("max check should exist");
        assert!(max.passed, "max=450 within 500 ms SLO");
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
            ..Aggregate::default()
        };
        let slo = evaluate_slo(&agg);
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
        assert!(md.contains("## RSS samples"));
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
