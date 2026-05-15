//! Integration tests for the `bench_report` aggregator CLI.
//!
//! Each test drives the binary through `Command::new(...)` so we exercise
//! argument parsing, JSON ingestion, Markdown rendering, and the
//! SLO/regression exit-code contract end-to-end.

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
        stdout.contains("surch.bench.artillery.v1"),
        "missing artillery schema in help"
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
fn empty_directory_produces_stable_summary_and_zero_exit() {
    let dir = unique_dir("empty");
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
        "empty dir should exit 0 (no SLO data, no regressions); stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let markdown = fs::read_to_string(&out_path).expect("summary.md should exist");
    assert!(markdown.starts_with("# Surch bench summary "));
    assert!(markdown.contains("## Artillery results"));
    assert!(markdown.contains("## RSS samples"));
    assert!(markdown.contains("## SLO checks"));
    assert!(markdown.contains("_no data_"));
    // No baseline → no regression section.
    assert!(!markdown.contains("## Regression vs baseline"));
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn summary_contains_tables_for_known_schemas() {
    let dir = unique_dir("tables");
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
    // SLO checks all PASS.
    assert!(md.contains("PASS"));
    assert!(!md.contains("FAIL"));
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
