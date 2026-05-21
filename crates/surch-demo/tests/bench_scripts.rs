use std::fs;
use std::path::PathBuf;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(|path| path.parent())
        .expect("surch-demo should live under crates/surch-demo")
        .to_path_buf()
}

fn trec_covid_script() -> String {
    fs::read_to_string(repo_root().join("scripts/bench/trec-covid-ndcg.sh"))
        .expect("trec-covid script should be readable")
}

fn scifact_script() -> String {
    fs::read_to_string(repo_root().join("scripts/bench/scifact-ndcg.sh"))
        .expect("scifact script should be readable")
}

#[test]
fn trec_covid_ndcg_script_fails_closed_on_http_errors() {
    let script = trec_covid_script();

    assert!(
        script.contains("set -euo pipefail"),
        "trec-covid-ndcg.sh must fail closed like scifact-ndcg.sh"
    );
}

#[test]
fn trec_covid_bulk_chunk_size_stays_below_surch_body_limit() {
    let script = trec_covid_script();

    assert!(
        script.contains(r#"TREC_COVID_BULK_CHUNK_SIZE="${TREC_COVID_BULK_CHUNK_SIZE:-8m}""#),
        "default TREC-COVID bulk chunk size must stay below Surch's 16 MiB body cap"
    );
    assert!(
        script.contains(r#"split -C "$TREC_COVID_BULK_CHUNK_SIZE""#),
        "bulk splitting should use the guarded TREC_COVID_BULK_CHUNK_SIZE knob"
    );
}

#[test]
fn scifact_script_reports_http_failure_context() {
    let script = scifact_script();

    assert!(
        script.contains("http_request()"),
        "scifact-ndcg.sh should wrap curl so HTTP failures include context"
    );
    assert!(
        script.contains(r#"HTTP $status during $label: $method $url"#),
        "scifact-ndcg.sh should print status, operation label, method, and URL"
    );
    assert!(
        script.contains(r#""bulk ingest $INDEX""#),
        "scifact-ndcg.sh should label bulk ingestion failures"
    );
    assert!(
        script.contains(r#""search $INDEX qid=$qid""#),
        "scifact-ndcg.sh should label per-query search failures"
    );
}

#[test]
fn trec_covid_script_reports_http_failure_context() {
    let script = trec_covid_script();

    assert!(
        script.contains("http_request()"),
        "trec-covid-ndcg.sh should wrap curl so HTTP failures include context"
    );
    assert!(
        script.contains(r#"HTTP $status during $label: $method $url"#),
        "trec-covid-ndcg.sh should print status, operation label, method, and URL"
    );
    assert!(
        script.contains(r#""bulk ingest $INDEX chunk $(basename "$chunk")""#),
        "trec-covid-ndcg.sh should label each bulk chunk"
    );
    assert!(
        script.contains(r#""search $INDEX qid=$qid""#),
        "trec-covid-ndcg.sh should label per-query search failures"
    );
}
