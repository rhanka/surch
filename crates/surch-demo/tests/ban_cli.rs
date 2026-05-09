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
    assert!(stdout.contains("OpenSearch over HTTP"));
    assert!(stdout.contains("Surch in-process"));
    assert!(stdout.contains("no global ratio"));
    assert!(stdout.contains("oracle required"));
    assert!(stdout.contains("does not start OpenSearch"));
}
