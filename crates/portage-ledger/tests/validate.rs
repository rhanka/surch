use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use portage_ledger::{check_language_policy, validate_ticket_path};

fn unique_dir(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before unix epoch")
        .as_nanos();
    std::env::temp_dir().join(format!("surch-{name}-{}-{stamp}", std::process::id()))
}

fn write_ticket(path: &Path, golden_tests_required: &str) {
    let ticket = format!(
        r#"{{
  "id": "LUCENE-store-DataInput-001",
  "title": "Port DataInput variable-length integer decoding",
  "owner": "StorageEngine",
  "priority": "Critical",
  "upstream_ref": {{
    "repo": "lucene",
    "commit": "7691b7ef9cfe3b87178646f4f32b3854afa0a567",
    "files": ["lucene/core/src/java/org/apache/lucene/store/DataInput.java"],
    "symbols": ["readVInt", "readVLong", "readZLong"]
  }},
  "parity_level": "P1 behavior",
  "dependencies": [],
  "allowed_paths": ["crates/surch-store/**", "tests/lucene_parity/**"],
  "forbidden_paths": ["crates/surch-api/**"],
  "golden_tests_required": {golden_tests_required},
  "gates": ["cargo test -p surch-store data_input"],
  "status": "discovered"
}}"#
    );
    fs::write(path, ticket).expect("write ticket");
}

#[test]
fn validates_ticket_directories() {
    let dir = unique_dir("ledger-valid");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_ticket(
        &dir.join("valid.json"),
        r#"[
    "Java fixture emits encoded bytes and expected decoded values",
    "Rust test consumes fixture and matches Java behavior"
  ]"#,
    );

    let count = validate_ticket_path(&dir).expect("valid ticket directory");

    assert_eq!(count, 1);
    fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn rejects_tickets_without_golden_tests() {
    let dir = unique_dir("ledger-invalid");
    fs::create_dir_all(&dir).expect("create temp dir");
    write_ticket(&dir.join("missing-golden.json"), "[]");

    let err = validate_ticket_path(&dir).expect_err("invalid ticket directory");

    assert!(err.to_string().contains("golden_tests_required"));
    fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn language_policy_rejects_disallowed_files() {
    let dir = unique_dir("language-policy-invalid");
    fs::create_dir_all(&dir).expect("create temp dir");
    let forbidden_extension = ["p", "y"].concat();
    fs::write(dir.join(format!("bad.{forbidden_extension}")), "print(1)\n")
        .expect("write forbidden file");

    let err = check_language_policy(&dir).expect_err("language policy violation");

    assert!(err.to_string().contains("disallowed file"));
    fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn language_policy_rejects_forbidden_interpreter_shebang() {
    let dir = unique_dir("language-policy-shebang");
    fs::create_dir_all(&dir).expect("create temp dir");
    let forbidden_interpreter = ["p", "y", "t", "h", "o", "n", "3"].concat();
    fs::write(
        dir.join("runner.sh"),
        format!("#!/usr/bin/env {forbidden_interpreter}\n"),
    )
    .expect("write shell file");

    let err = check_language_policy(&dir).expect_err("language policy violation");

    assert!(err.to_string().contains("disallowed file"));
    fs::remove_dir_all(dir).expect("remove temp dir");
}

#[test]
fn language_policy_accepts_rust_workspace_files() {
    let dir = unique_dir("language-policy-valid");
    fs::create_dir_all(dir.join("src")).expect("create temp dir");
    fs::write(dir.join("src/lib.rs"), "pub fn ok() {}\n").expect("write rust file");
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"fixture\"\n").expect("write cargo file");

    check_language_policy(&dir).expect("valid language policy");
    fs::remove_dir_all(dir).expect("remove temp dir");
}
