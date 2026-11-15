//! MVP L2: `_analyze` parity table of the real matchID `deces` index
//! definition (`tests/matchid_compat/deces/analyze_parity.json`).
//!
//! Every row is the body of an ES `POST /deces/_analyze` request (`analyzer`,
//! `normalizer`, `field` or `tokenizer`) plus a text and the token list ES
//! 8.6.1 is expected to return. No row has been run against ES yet: the
//! `status` of each row says where its expectation comes from (`es-doc`,
//! `spec-derived`, `es-unverified`, `known-deviation`). Surch must emit
//! `surch` when the row records a divergence, `expected` otherwise.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};
use surch_index::mapping::IndexMapping;

const INDEX_DEFINITION: &str =
    include_str!("../../../tests/matchid_compat/deces/mapping_real.json");
const PARITY_TABLE: &str = include_str!("../../../tests/matchid_compat/deces/analyze_parity.json");

const STATUSES: [&str; 4] = ["es-doc", "spec-derived", "es-unverified", "known-deviation"];

fn tokenizer_probe(name: &str) -> String {
    format!("__tokenizer_probe__{name}")
}

fn strings(value: &Value) -> Vec<String> {
    value
        .as_array()
        .expect("token list is an array")
        .iter()
        .map(|token| token.as_str().expect("token is a string").to_owned())
        .collect()
}

fn deces_mapping(rows: &[Value]) -> IndexMapping {
    let definition: Value = serde_json::from_str(INDEX_DEFINITION).expect("mapping_real.json");
    let mut settings = definition["settings"].clone();
    // `_analyze {"tokenizer": ...}` has no IndexMapping entry point: probe a
    // tokenizer through a synthetic analyzer made of it alone.
    for row in rows {
        if let Some(name) = row["request"]["tokenizer"].as_str() {
            settings["analysis"]["analyzer"][tokenizer_probe(name)] = json!({"tokenizer": name});
        }
    }
    let mut mapping = IndexMapping::from_properties_value(&definition["mappings"]["properties"])
        .expect("deces properties parse");
    mapping.set_analysis(
        IndexMapping::from_index_settings_value(&settings).expect("deces analysis compiles"),
    );
    mapping
}

fn surch_tokens(mapping: &IndexMapping, request: &Value, text: &str) -> Vec<String> {
    let analysis = mapping.analysis();
    if let Some(name) = request["analyzer"].as_str() {
        return analysis
            .resolve_analyzer(name)
            .unwrap_or_else(|| panic!("analyzer {name} resolves"))
            .terms(text);
    }
    if let Some(name) = request["normalizer"].as_str() {
        return analysis
            .resolve_normalizer(name)
            .unwrap_or_else(|| panic!("normalizer {name} resolves"))
            .terms(text);
    }
    if let Some(field) = request["field"].as_str() {
        assert!(
            mapping.resolve_field(field).is_some(),
            "field {field} is mapped"
        );
        return mapping.index_analyzer(field).terms(text);
    }
    if let Some(name) = request["tokenizer"].as_str() {
        return analysis
            .resolve_analyzer(&tokenizer_probe(name))
            .expect("tokenizer probe resolves")
            .terms(text);
    }
    panic!("unsupported _analyze request {request}");
}

#[test]
fn deces_analyze_parity_table_matches_surch() {
    let table: Value = serde_json::from_str(PARITY_TABLE).expect("analyze_parity.json");
    let rows = table["rows"].as_array().expect("rows").clone();
    let mapping = deces_mapping(&rows);

    let mut ids = BTreeSet::new();
    let mut per_status = BTreeMap::<String, usize>::new();
    let mut mismatches = Vec::new();
    for row in &rows {
        let id = row["id"].as_str().expect("row id");
        assert!(ids.insert(id.to_owned()), "duplicate row id {id}");
        let status = row["status"].as_str().expect("row status");
        assert!(STATUSES.contains(&status), "{id}: unknown status {status}");
        *per_status.entry(status.to_owned()).or_default() += 1;
        let divergent = !row["surch"].is_null();
        if divergent {
            assert!(
                matches!(status, "es-unverified" | "known-deviation"),
                "{id}: a `surch` divergence is only allowed on es-unverified / known-deviation rows"
            );
        }
        if status == "known-deviation" {
            assert!(
                divergent,
                "{id}: known-deviation rows record the Surch output"
            );
        }

        let text = row["text"].as_str().expect("row text");
        let expected = strings(if divergent {
            &row["surch"]
        } else {
            &row["expected"]
        });
        let actual = surch_tokens(&mapping, &row["request"], text);
        if actual != expected {
            mismatches.push(format!(
                "{id}: {text:?} -> {actual:?}, expected {expected:?}"
            ));
        }
    }

    println!("deces _analyze parity rows by status: {per_status:?}");
    assert!(
        mismatches.is_empty(),
        "_analyze parity mismatches:\n{}",
        mismatches.join("\n")
    );
}
