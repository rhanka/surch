use std::process::Command;

#[test]
fn ban_poc_prints_stable_demo_summary() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .arg("ban-poc")
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-poc should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Surch BAN PoC"));
    assert!(stdout.contains("dataset: ban_tiny"));
    assert!(stdout.contains("documents: 3"));
    assert!(stdout.contains("count: 3"));
    assert!(stdout.contains("match label: 75101_0001_00001"));
    assert!(stdout.contains("bool address: 33063_0002_00010B"));
    assert!(stdout.contains("fuzzy label: 67482_0003_00007"));
}

#[test]
fn ban_bench_prints_publishable_metric_labels() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-bench", "--iterations", "2"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-bench should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Surch BAN bench"));
    assert!(stdout.contains("dataset: ban_tiny"));
    assert!(stdout.contains("iterations: 2"));
    assert!(stdout.contains("load_ban_tiny:"));
    assert!(stdout.contains("count_match_all:"));
    assert!(stdout.contains("search_match_label:"));
    assert!(stdout.contains("search_bool_address:"));
    assert!(stdout.contains("search_fuzzy_label:"));
}

#[test]
fn ban_bench_prints_reproducible_latency_summary_and_guardrails() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-bench", "--iterations", "3"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-bench should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("dataset: ban_tiny"));
    assert!(stdout.contains("iterations: 3"));
    assert!(stdout.contains("oracle: tests/opensearch_compat/oracle/replays/ban_tiny_search.json"));
    assert!(stdout.contains("runtime: Surch in-process axum router"));
    assert!(stdout.contains("guardrails:"));
    assert!(stdout.contains("no Elasticsearch ratio"));
    assert!(stdout.contains("same host/build only"));

    for operation in [
        "load_ban_tiny",
        "count_match_all",
        "search_match_label",
        "search_bool_address",
        "search_fuzzy_label",
    ] {
        let metric = stdout
            .lines()
            .find(|line| line.starts_with(&format!("{operation}:")))
            .unwrap_or_else(|| panic!("{operation} metric should be printed"));
        assert!(metric.contains(&format!("operation={operation}")));
        assert!(metric.contains("iterations=3"));
        assert!(metric.contains("count="));
        assert!(metric.contains("p50_us="));
        assert!(metric.contains("p95_us="));
    }
}

#[test]
fn ban_compare_plan_prints_guardrails_without_running_engines() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .arg("ban-compare-plan")
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-compare-plan should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Surch BAN compare plan"));
    assert!(stdout.contains("dataset: ban_tiny"));
    assert!(stdout.contains("documents: 3"));
    assert!(stdout.contains("Elasticsearch over HTTP"));
    assert!(stdout.contains("Surch in-process"));
    assert!(stdout.contains("no global ratio"));
    assert!(stdout.contains("oracle required"));
    assert!(stdout.contains("does not start Elasticsearch"));
}

#[test]
fn ban_http_bench_help_describes_dry_run_command() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-http-bench", "--help"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-http-bench --help should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Surch BAN HTTP bench"));
    assert!(stdout.contains("usage: surch-demo ban-http-bench [OPTIONS]"));
    assert!(stdout.contains("--surch-url URL"));
    assert!(stdout.contains("--elasticsearch-url URL"));
    assert!(stdout.contains("--opensearch-url URL"));
    assert!(stdout.contains("--index NAME"));
    assert!(stdout.contains("--iterations N"));
    assert!(stdout.contains("--dataset PATH"));
    assert!(stdout.contains("--oracle PATH"));
    assert!(stdout.contains("--warmup N"));
    assert!(stdout.contains("--report PATH"));
    assert!(stdout.contains("--timeout-seconds N"));
    assert!(stdout.contains("--dry-run"));
    assert!(stdout.contains("Elasticsearch HTTP base URL"));
    assert!(stdout.contains("legacy alias for --elasticsearch-url"));
    assert!(stdout.contains("executes a symmetric HTTP benchmark unless --dry-run is supplied"));
}

#[test]
fn ban_http_bench_prints_structured_dry_run_plan() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args([
            "ban-http-bench",
            "--surch-url",
            "http://127.0.0.1:7700",
            "--elasticsearch-url",
            "http://127.0.0.1:9200",
            "--index",
            "ban_ci",
            "--iterations",
            "7",
            "--dataset",
            "tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson",
            "--oracle",
            "tests/opensearch_compat/oracle/replays/ban_tiny_search.json",
            "--warmup",
            "2",
            "--timeout-seconds",
            "45",
            "--report",
            "target/ban-http-bench.json",
            "--dry-run",
        ])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-http-bench should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("Surch BAN HTTP bench plan"));
    assert!(stdout.contains("mode: dry-run"));
    assert!(stdout.contains("dataset: tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson"));
    assert!(stdout.contains("documents: 3"));
    assert!(stdout.contains("dataset_bytes:"));
    assert!(stdout.contains("index: ban_ci"));
    assert!(stdout.contains("iterations: 7"));
    assert!(stdout.contains("warmup: 2"));
    assert!(stdout.contains("timeout_seconds: 45"));
    assert!(stdout.contains("surch_url: http://127.0.0.1:7700"));
    assert!(stdout.contains("elasticsearch_url: http://127.0.0.1:9200"));
    assert!(!stdout.contains("opensearch_url:"));
    assert!(stdout.contains("oracle: tests/opensearch_compat/oracle/replays/ban_tiny_search.json"));
    assert!(stdout.contains("report: target/ban-http-bench.json"));
    assert!(stdout.contains("operations:"));
    assert!(stdout.contains("  - create_index"));
    assert!(stdout.contains("  - bulk_ingest"));
    assert!(stdout.contains("  - refresh"));
    assert!(stdout.contains("  - count_ban_tiny_addresses"));
    assert!(stdout.contains("  - search_ban_tiny_by_label"));
    assert!(stdout.contains("  - search_ban_tiny_by_address_fields"));
    assert!(stdout.contains("  - future_fuzzy_label_typo"));
    assert!(stdout.contains("guardrail: dry-run mode sends no HTTP requests"));
    assert!(stdout.contains("guardrail: does not start Elasticsearch, Docker, or a Surch server"));
}

#[test]
fn ban_http_bench_prints_default_artifact_paths() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-http-bench", "--dry-run"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-http-bench should succeed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("dataset: tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson"));
    assert!(stdout.contains("oracle: tests/opensearch_compat/oracle/replays/ban_tiny_search.json"));
    assert!(stdout.contains("warmup: 0"));
    assert!(stdout.contains("timeout_seconds: 30"));
    assert!(stdout.contains("report: <none>"));
    assert!(stdout.contains("mode: dry-run"));
    assert!(stdout.contains("guardrail: dry-run mode sends no HTTP requests"));
    assert!(stdout.contains("elasticsearch_url: http://127.0.0.1:9200"));
    assert!(!stdout.contains("opensearch_url:"));
}

#[test]
fn ban_http_bench_accepts_legacy_opensearch_url_alias() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args([
            "ban-http-bench",
            "--opensearch-url",
            "http://127.0.0.1:19200",
            "--dry-run",
        ])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        output.status.success(),
        "ban-http-bench should accept the legacy alias: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8(output.stdout).expect("stdout should be utf8");
    assert!(stdout.contains("elasticsearch_url: http://127.0.0.1:19200"));
    assert!(!stdout.contains("opensearch_url:"));
}

#[test]
fn ban_http_bench_attempts_http_execution_by_default() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args([
            "ban-http-bench",
            "--surch-url",
            "http://127.0.0.1:1",
            "--elasticsearch-url",
            "http://127.0.0.1:1",
            "--iterations",
            "1",
        ])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        !output.status.success(),
        "ban-http-bench should fail when the HTTP engine is unavailable"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("error:"));
    assert!(!stderr.contains("dry-run"));
}

#[test]
fn ban_http_bench_rejects_zero_iterations() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-http-bench", "--iterations", "0"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        !output.status.success(),
        "ban-http-bench should reject zero iterations"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("error: --iterations must be greater than zero"));
}

#[test]
fn ban_http_bench_rejects_negative_warmup() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-http-bench", "--warmup", "-1"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        !output.status.success(),
        "ban-http-bench should reject negative warmup"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("error: --warmup must be a non-negative integer"));
}

#[test]
fn ban_http_bench_rejects_zero_timeout() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-http-bench", "--timeout-seconds", "0"])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        !output.status.success(),
        "ban-http-bench should reject zero timeout"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("error: --timeout-seconds must be greater than zero"));
}

#[test]
fn ban_http_bench_rejects_empty_dataset() {
    let output = Command::new(env!("CARGO_BIN_EXE_surch-demo"))
        .args(["ban-http-bench", "--dataset", ""])
        .output()
        .expect("surch-demo binary should run");

    assert!(
        !output.status.success(),
        "ban-http-bench should reject empty dataset"
    );

    let stderr = String::from_utf8(output.stderr).expect("stderr should be utf8");
    assert!(stderr.contains("error: --dataset must not be empty"));
}
