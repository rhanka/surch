//! Integration smoke for the `artillery_bench` Rust harness.
//!
//! These tests do not require a running HTTP server. They exercise the
//! CLI surface: --help is stable and the --phases parser rejects
//! malformed values without panicking.

use std::process::Command;

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_artillery_bench")
}

#[test]
fn help_prints_usage_and_schema_label() {
    let output = Command::new(bin())
        .arg("--help")
        .output()
        .expect("artillery_bench binary should run");
    assert!(
        output.status.success(),
        "--help should exit 0: stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf-8");
    assert!(stdout.contains("artillery_bench"));
    assert!(stdout.contains("--phases"));
    assert!(stdout.contains("--workers"));
    assert!(stdout.contains("surch.bench.artillery.v1"));
}

#[test]
fn rejects_invalid_phases() {
    let output = Command::new(bin())
        .args([
            "--url",
            "http://127.0.0.1:7700",
            "--names",
            "/dev/null",
            "--phases",
            "0:10",
        ])
        .output()
        .expect("artillery_bench binary should run");
    assert!(
        !output.status.success(),
        "0:10 should be rejected as invalid (rps must be > 0)"
    );
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(
        stderr.contains("rps must be > 0") || stderr.contains("rps"),
        "stderr should explain rps validation: {stderr}"
    );
}

#[test]
fn rejects_unknown_option() {
    let output = Command::new(bin())
        .args(["--not-a-flag"])
        .output()
        .expect("artillery_bench binary should run");
    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf-8");
    assert!(stderr.contains("unknown option"), "stderr={stderr}");
}
