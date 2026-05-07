use opensearch_oracle::ban::{
    ban_records_to_bulk_ndjson, parse_ban_csv, BanProfile, BAN_DATASET_ID, BAN_LICENSE,
    BAN_SOURCE_CSV_URL,
};
use opensearch_oracle::dataset::{DatasetManifest, DatasetOperationKind};

#[test]
fn ban_source_metadata_matches_datagouv_catalog() {
    assert_eq!(BAN_DATASET_ID, "5530fbacc751df5ff937dddb");
    assert_eq!(BAN_LICENSE, "lov2");
    assert_eq!(
        BAN_SOURCE_CSV_URL,
        "https://adresse.data.gouv.fr/data/ban/adresses/latest/csv"
    );
}

#[test]
fn ban_profiles_define_commit_and_cache_policy() {
    let tiny = BanProfile::Tiny.acquisition_plan();
    assert_eq!(tiny.name, "ban_tiny");
    assert_eq!(tiny.max_records, Some(500));
    assert!(tiny.committable_fixture);
    assert_eq!(tiny.cache_subdir, "ban/tiny");

    let sample = BanProfile::Sample.acquisition_plan();
    assert_eq!(sample.name, "ban_sample");
    assert_eq!(sample.max_records, Some(100_000));
    assert!(!sample.committable_fixture);
    assert_eq!(sample.cache_subdir, "ban/sample");

    let full = BanProfile::Full.acquisition_plan();
    assert_eq!(full.name, "ban_full");
    assert_eq!(full.max_records, None);
    assert!(!full.committable_fixture);
    assert_eq!(full.cache_subdir, "ban/full");
}

#[test]
fn parses_ban_tiny_fixture_with_accents_and_coordinates() {
    let records = parse_ban_csv(include_str!(
        "../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.csv"
    ))
    .expect("BAN tiny fixture should parse");

    assert_eq!(records.len(), 3);
    assert_eq!(records[0].id, "75101_0001_00001");
    assert_eq!(records[0].house_number.as_deref(), Some("1"));
    assert_eq!(records[0].street_name, "Rue de Rivoli");
    assert_eq!(records[0].postcode, "75001");
    assert_eq!(records[0].city_name, "Paris 1er Arrondissement");
    assert_eq!(records[0].longitude, 2.3364);
    assert_eq!(records[0].latitude, 48.8609);

    assert_eq!(records[1].house_number.as_deref(), Some("10B"));
    assert_eq!(records[1].street_name, "Cours de l'Intendance");
    assert_eq!(records[2].street_name, "Allée des Érables");
}

#[test]
fn converts_ban_records_to_opensearch_bulk_ndjson() {
    let records = parse_ban_csv(include_str!(
        "../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.csv"
    ))
    .expect("BAN tiny fixture should parse");

    let ndjson = ban_records_to_bulk_ndjson("ban_tiny", &records)
        .expect("BAN records should convert to bulk NDJSON");
    let lines: Vec<&str> = ndjson.lines().collect();

    assert_eq!(lines.len(), 6);
    assert_eq!(
        lines[0],
        r#"{"index":{"_id":"75101_0001_00001","_index":"ban_tiny"}}"#
    );
    assert!(lines[1].contains(r#""label":"1 Rue de Rivoli 75001 Paris 1er Arrondissement""#));
    assert!(lines[1].contains(r#""location":{"lat":48.8609,"lon":2.3364}"#));
    assert_eq!(
        lines[2],
        r#"{"index":{"_id":"33063_0002_00010B","_index":"ban_tiny"}}"#
    );
    assert!(ndjson.ends_with('\n'));
}

#[test]
fn ban_tiny_dataset_manifest_uses_generated_bulk_fixture() {
    let manifest = DatasetManifest::from_json_str(include_str!(
        "../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.json"
    ))
    .expect("BAN tiny dataset manifest should parse");

    assert_eq!(manifest.name, "ban_tiny");
    assert_eq!(manifest.operations.len(), 3);
    assert_eq!(
        manifest.operations[0].kind,
        DatasetOperationKind::CreateIndex
    );
    assert_eq!(manifest.operations[0].path, "/ban_tiny");
    assert_eq!(manifest.operations[1].kind, DatasetOperationKind::Bulk);
    assert_eq!(
        manifest.operations[1].body.as_deref(),
        Some("ban_tiny.ndjson")
    );
    assert_eq!(manifest.operations[2].kind, DatasetOperationKind::Refresh);

    let records = parse_ban_csv(include_str!(
        "../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.csv"
    ))
    .expect("BAN tiny CSV should parse");
    let generated = ban_records_to_bulk_ndjson("ban_tiny", &records)
        .expect("BAN tiny records should convert to bulk NDJSON");
    let committed =
        include_str!("../../../tests/opensearch_compat/oracle/datasets/ban/ban_tiny.ndjson");

    assert_eq!(generated, committed);
}

#[test]
fn rejects_ban_csv_without_required_columns() {
    let err = parse_ban_csv("id,numero,nom_voie\n1,2,Rue absente\n")
        .expect_err("missing required columns should be rejected");

    assert!(err.to_string().contains("missing required BAN column"));
}
