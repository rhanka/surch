//! Integration tests for the `bench_report` aggregator CLI.
//!
//! Each test drives the binary through `Command::new(...)` so we exercise
//! argument parsing, JSON ingestion, Markdown rendering, and the
//! SLO/regression exit-code contract end-to-end.

use serde_json::Value;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_bench_report")
}

fn unique_dir(label: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let path = std::env::temp_dir().join(format!("surch-bench-report-{label}-{pid}-{nanos}"));
    fs::create_dir_all(&path).expect("create temp dir");
    path
}

fn write_json(dir: &Path, name: &str, body: &str) {
    fs::write(dir.join(name), body).expect("write fixture json");
}

#[test]
fn help_prints_usage_and_schemas() {
    let output = Command::new(bin())
        .arg("--help")
        .output()
        .expect("bench_report binary should run");
    assert!(
        output.status.success(),
        "--help should exit 0: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout utf-8");
    assert!(stdout.contains("bench_report"), "missing tool name");
    assert!(stdout.contains("--dir"), "missing --dir flag");
    assert!(stdout.contains("--baseline"), "missing --baseline flag");
    assert!(stdout.contains("--output"), "missing --output flag");
    assert!(
        stdout.contains("--promote-dir"),
        "missing --promote-dir flag"
    );
    assert!(
        stdout.contains("Human readers use summary.md or promoted README.md"),
        "help should direct humans to Markdown"
    );
    assert!(
        stdout.contains("Agents and CI validate summary.json"),
        "help should direct agents/CI to JSON"
    );
    assert!(
        stdout.contains("surch.bench.artillery.v1"),
        "missing artillery schema in help"
    );
    assert!(
        stdout.contains("surch.bench.ban_http.v1"),
        "missing BAN HTTP schema in help"
    );
    assert!(
        stdout.contains("surch.bench.rss.v1"),
        "missing rss schema in help"
    );
    assert!(
        stdout.contains("surch.bench.pair.v1"),
        "missing pair schema in help"
    );
}

#[test]
fn promote_dir_writes_readme_and_machine_summary_json() {
    let dir = unique_dir("promote-src");
    let promote_dir = unique_dir("promote-dst");
    write_json(
        &dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 0,
                "p50_ms": 4.0, "p95_ms": 50.0, "p99_ms": 80.0, "max_ms": 120.0
            }
        }"#,
    );

    let output = Command::new(bin())
        .args([
            "--dir",
            dir.to_str().unwrap(),
            "--promote-dir",
            promote_dir.to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        output.status.success(),
        "promotion should exit 0: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );

    let readme = fs::read_to_string(promote_dir.join("README.md")).expect("README.md exists");
    assert!(readme.starts_with("# Surch bench summary "));
    assert!(readme.contains("## SLO checks"));

    let json: Value = serde_json::from_str(
        &fs::read_to_string(promote_dir.join("summary.json")).expect("summary.json exists"),
    )
    .expect("summary.json valid json");
    assert_eq!(json["schema"], "surch.bench.summary.v1");
    assert_eq!(json["verdict"]["exit_ok"], true);

    let _ = fs::remove_dir_all(&dir);
    let _ = fs::remove_dir_all(&promote_dir);
}

#[test]
fn output_and_promote_dir_are_mutually_exclusive() {
    let dir = unique_dir("promote-conflict");
    let output = Command::new(bin())
        .args([
            "--dir",
            dir.to_str().unwrap(),
            "--output",
            dir.join("summary.md").to_str().unwrap(),
            "--promote-dir",
            dir.join("promoted").to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        !output.status.success(),
        "--output and --promote-dir should be rejected together"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr utf-8");
    assert!(
        stderr.contains("--output and --promote-dir are mutually exclusive"),
        "unexpected stderr: {stderr}"
    );
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn empty_directory_produces_stable_summary_and_zero_exit() {
    let dir = unique_dir("empty");
    let out_path = dir.join("summary.md");
    let json_path = dir.join("summary.json");
    let output = Command::new(bin())
        .args([
            "--dir",
            dir.to_str().unwrap(),
            "--output",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        output.status.success(),
        "empty dir should exit 0 (no SLO data, no regressions); stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let markdown = fs::read_to_string(&out_path).expect("summary.md should exist");
    assert!(markdown.starts_with("# Surch bench summary "));
    assert!(markdown.contains("## Artillery results"));
    assert!(markdown.contains("## BAN HTTP results"));
    assert!(markdown.contains("## RSS samples"));
    assert!(markdown.contains("## SLO checks"));
    assert!(markdown.contains("_no data_"));
    // No baseline → no regression section.
    assert!(!markdown.contains("## Regression vs baseline"));
    let json: Value =
        serde_json::from_str(&fs::read_to_string(&json_path).expect("summary.json should exist"))
            .expect("summary.json valid json");
    assert_eq!(json["schema"], "surch.bench.summary.v1");
    assert_eq!(json["verdict"]["exit_ok"], true);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn summary_contains_tables_for_known_schemas() {
    let dir = unique_dir("tables");
    let json_path = dir.join("summary.json");
    // Artillery report with PASSING SLOs.
    write_json(
        &dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 0,
                "p50_ms": 4.0, "p95_ms": 50.0, "p99_ms": 80.0, "max_ms": 120.0
            }
        }"#,
    );
    write_json(
        &dir,
        "rss-art-surch.json",
        r#"{
            "schema": "surch.bench.rss.v1",
            "pid": 1234, "duration_s": 60, "samples": 60,
            "peak_kb": 524288, "peak_mb": 512,
            "final_kb": 460800, "final_mb": 450
        }"#,
    );
    write_json(
        &dir,
        "insee25k-pair.json",
        r#"{
            "schema": "surch.bench.pair.v1",
            "workload": "insee25k",
            "surch_out": "/tmp/insee25k-surch.out",
            "os_out": "/tmp/insee25k-os.out"
        }"#,
    );
    write_json(
        &dir,
        "ban-http.json",
        r#"{
            "schema": "surch.bench.ban_http.v1",
            "benchmark": "ban-http-bench",
            "index": "ban_ci",
            "engines": [
                {
                    "name": "surch",
                    "url": "http://127.0.0.1:7700",
                    "metrics": [
                        {
                            "operation": "bulk_ingest",
                            "status": 200,
                            "iterations": 1,
                            "docs_per_second": 2000.0,
                            "bytes_per_second": 600000.0,
                            "error_count": 0,
                            "latency_us": {
                                "min": 100,
                                "p50": 300,
                                "p95": 500,
                                "p99": 500,
                                "max": 500,
                                "total": 1500
                            }
                        }
                    ]
                },
                {
                    "name": "elasticsearch",
                    "url": "http://127.0.0.1:9200",
                    "metrics": [
                        {
                            "operation": "bulk_ingest",
                            "status": 200,
                            "iterations": 1,
                            "docs_per_second": 1000.0,
                            "bytes_per_second": 300000.0,
                            "error_count": 0,
                            "latency_us": {
                                "min": 200,
                                "p50": 600,
                                "p95": 900,
                                "p99": 900,
                                "max": 900,
                                "total": 3000
                            }
                        }
                    ]
                }
            ]
        }"#,
    );

    let out_path = dir.join("summary.md");
    let output = Command::new(bin())
        .args([
            "--dir",
            dir.to_str().unwrap(),
            "--output",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        output.status.success(),
        "all SLOs should pass on fixture; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let md = fs::read_to_string(&out_path).expect("summary.md");

    // Artillery row rendered with engine + workload + p95.
    assert!(md.contains("| surch | deces_25k |"));
    assert!(md.contains("50.0"));
    // RSS row rendered (peak/final MB).
    assert!(md.contains("| 512.0 | 450.0 |"));
    // Pair section appears when at least one pair envelope is present.
    assert!(md.contains("## Pair envelopes"));
    assert!(md.contains("insee25k"));
    // BAN HTTP rows render for human comparison without opening JSON.
    assert!(md.contains("## BAN HTTP results"));
    assert!(md.contains(
        "| surch | bulk_ingest | 200 | 1 | 300 | 500 | 500 | 500 | 0 | 2000.0 | 600000.0 |"
    ));
    assert!(md.contains(
        "| elasticsearch | bulk_ingest | 200 | 1 | 600 | 900 | 900 | 900 | 0 | 1000.0 | 300000.0 |"
    ));
    // SLO checks all PASS.
    assert!(md.contains("PASS"));
    assert!(!md.contains("FAIL"));
    let json: Value =
        serde_json::from_str(&fs::read_to_string(&json_path).expect("summary.json should exist"))
            .expect("summary.json valid json");
    assert_eq!(json["schema"], "surch.bench.summary.v1");
    assert_eq!(json["artillery"][0]["engine"], "surch");
    assert_eq!(json["ban_http"][0]["engine"], "elasticsearch");
    assert_eq!(json["ban_http"][1]["engine"], "surch");
    assert_eq!(json["rss"][0]["peak_mb"], 512.0);
    assert_eq!(json["pair"][0]["workload"], "insee25k");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn slo_failure_yields_nonzero_exit_code() {
    let dir = unique_dir("slo-fail");
    write_json(
        &dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 100,
                "p50_ms": 50.0, "p95_ms": 350.0, "p99_ms": 600.0, "max_ms": 900.0
            }
        }"#,
    );
    let out_path = dir.join("summary.md");
    let output = Command::new(bin())
        .args([
            "--dir",
            dir.to_str().unwrap(),
            "--output",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        !output.status.success(),
        "p95=350ms, max=900ms, errors=10% → all three SLOs must fail and the CLI must exit 1"
    );
    let md = fs::read_to_string(&out_path).expect("summary.md");
    assert!(md.contains("FAIL"), "FAIL markers should appear: {md}");
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn regression_against_baseline_yields_nonzero_exit_code() {
    let base_dir = unique_dir("base");
    let head_dir = unique_dir("head");
    // Baseline: passes SLO, p95=50 ms.
    write_json(
        &base_dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 0,
                "p50_ms": 5.0, "p95_ms": 50.0, "p99_ms": 80.0, "max_ms": 120.0
            }
        }"#,
    );
    // Head: SLO still passes (p95=80 < 200), but regression vs baseline is
    // (80-50)/50 = +60 % > 15 % threshold ⇒ must trip regression gate.
    write_json(
        &head_dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 0,
                "p50_ms": 6.0, "p95_ms": 80.0, "p99_ms": 110.0, "max_ms": 150.0
            }
        }"#,
    );
    let out_path = head_dir.join("summary.md");
    let output = Command::new(bin())
        .args([
            "--dir",
            head_dir.to_str().unwrap(),
            "--baseline",
            base_dir.to_str().unwrap(),
            "--output",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        !output.status.success(),
        "+60 % p95 regression vs baseline should trip exit code 1"
    );
    let md = fs::read_to_string(&out_path).expect("summary.md");
    assert!(md.contains("## Regression vs baseline"));
    assert!(md.contains("FAIL"));
    assert!(md.contains("p95 regression"));
    let _ = fs::remove_dir_all(&base_dir);
    let _ = fs::remove_dir_all(&head_dir);
}

#[test]
fn baseline_within_threshold_keeps_exit_code_zero() {
    let base_dir = unique_dir("base-ok");
    let head_dir = unique_dir("head-ok");
    write_json(
        &base_dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 0,
                "p50_ms": 5.0, "p95_ms": 50.0, "p99_ms": 80.0, "max_ms": 120.0
            }
        }"#,
    );
    write_json(
        &head_dir,
        "art-surch.json",
        r#"{
            "schema": "surch.bench.artillery.v1",
            "url": "http://127.0.0.1:7700",
            "index": "deces_25k",
            "global": {
                "issued": 1000, "errors": 0,
                "p50_ms": 5.0, "p95_ms": 55.0, "p99_ms": 80.0, "max_ms": 120.0
            }
        }"#,
    );
    let out_path = head_dir.join("summary.md");
    let output = Command::new(bin())
        .args([
            "--dir",
            head_dir.to_str().unwrap(),
            "--baseline",
            base_dir.to_str().unwrap(),
            "--output",
            out_path.to_str().unwrap(),
        ])
        .output()
        .expect("bench_report binary should run");
    assert!(
        output.status.success(),
        "+10 % (within 15 % budget) should keep exit code 0; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&base_dir);
    let _ = fs::remove_dir_all(&head_dir);
}
