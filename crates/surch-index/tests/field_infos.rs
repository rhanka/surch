use surch_index::field_infos::{
    DocValuesType, FieldInfo, FieldInfos, FieldInfosError, IndexOptions,
};

#[test]
fn field_infos_accepts_unique_fields() {
    let title = FieldInfo::new(
        "title",
        0,
        IndexOptions::DocsAndFreqsAndPositions,
        DocValuesType::None,
        false,
        true,
    );
    let published_at = FieldInfo::new(
        "published_at",
        1,
        IndexOptions::None,
        DocValuesType::Numeric,
        true,
        false,
    );

    let infos = FieldInfos::new(vec![title.clone(), published_at.clone()]).expect("field infos");

    assert_eq!(infos.len(), 2);
    assert_eq!(infos.field_info("title"), Some(&title));
    assert_eq!(infos.field_info_by_number(1), Some(&published_at));
}

#[test]
fn field_infos_rejects_duplicate_field_names() {
    let first = FieldInfo::new(
        "title",
        0,
        IndexOptions::Docs,
        DocValuesType::None,
        false,
        false,
    );
    let duplicate_name = FieldInfo::new(
        "title",
        1,
        IndexOptions::DocsAndFreqs,
        DocValuesType::None,
        false,
        false,
    );

    let err =
        FieldInfos::new(vec![first, duplicate_name]).expect_err("duplicate field name rejected");

    assert!(matches!(
        err,
        FieldInfosError::DuplicateFieldName { name } if name == "title"
    ));
}

#[test]
fn field_infos_rejects_duplicate_field_numbers() {
    let first = FieldInfo::new(
        "title",
        0,
        IndexOptions::Docs,
        DocValuesType::None,
        false,
        false,
    );
    let duplicate_number = FieldInfo::new(
        "body",
        0,
        IndexOptions::DocsAndFreqsAndPositions,
        DocValuesType::Binary,
        false,
        true,
    );

    let err = FieldInfos::new(vec![first, duplicate_number])
        .expect_err("duplicate field number rejected");

    assert!(matches!(
        err,
        FieldInfosError::DuplicateFieldNumber { number: 0 }
    ));
}
