use crate::common::{FieldDefinition, FieldType, Mapping};
use crate::indexer::error::Error;

pub struct FieldMapper;

impl FieldMapper {
    pub fn map_document(
        mapping: &Mapping,
        fields: &std::collections::HashMap<String, crate::common::FieldValue>,
    ) -> Result<std::collections::HashMap<String, crate::common::FieldValue>, Error> {
        let mut mapped = std::collections::HashMap::new();

        for (field_name, field_value) in fields {
            if let Some(field_def) = mapping.get_field(field_name) {
                let mapped_value = Self::map_value(field_def, field_value)?;
                mapped.insert(field_name.clone(), mapped_value);
            } else {
                mapped.insert(field_name.clone(), field_value.clone());
            }
        }

        Ok(mapped)
    }

    fn map_value(
        field_def: &FieldDefinition,
        value: &crate::common::FieldValue,
    ) -> Result<crate::common::FieldValue, Error> {
        Self::validate_field_definition(field_def)?;

        if !Self::value_matches_field_type(&field_def.field_type, value) {
            return Err(Error::InvalidField(field_def.name.clone()));
        }

        Ok(value.clone())
    }

    fn validate_field_definition(field_def: &FieldDefinition) -> Result<(), Error> {
        if let Some(analyzer) = &field_def.analyzer {
            if !matches!(
                analyzer.as_str(),
                "standard" | "simple" | "stop" | "keyword"
            ) {
                return Err(Error::Mapping(format!(
                    "unsupported analyzer '{analyzer}' for field '{}'",
                    field_def.name
                )));
            }
        }

        Ok(())
    }

    fn value_matches_field_type(field_type: &FieldType, value: &crate::common::FieldValue) -> bool {
        use crate::common::FieldValue;

        match value {
            FieldValue::Array(values) => {
                !values.is_empty()
                    && values
                        .iter()
                        .all(|entry| Self::value_matches_field_type(field_type, entry))
            }
            FieldValue::Null => true,
            FieldValue::Text(_) => matches!(field_type, FieldType::Text),
            FieldValue::Keyword(_) => matches!(field_type, FieldType::Keyword),
            FieldValue::Integer(_) => matches!(field_type, FieldType::Integer),
            FieldValue::Long(_) => matches!(field_type, FieldType::Long),
            FieldValue::Float(_) => matches!(field_type, FieldType::Float),
            FieldValue::Double(_) => matches!(field_type, FieldType::Double),
            FieldValue::Bool(_) => matches!(field_type, FieldType::Boolean),
            FieldValue::Date(_) => matches!(field_type, FieldType::Date),
        }
    }

    pub fn infer_mapping(
        fields: &std::collections::HashMap<String, crate::common::FieldValue>,
    ) -> Mapping {
        let mut mapping = Mapping::new();

        for (name, value) in fields {
            let field_type = match value {
                crate::common::FieldValue::Text(_) => FieldType::Text,
                crate::common::FieldValue::Keyword(_) => FieldType::Keyword,
                crate::common::FieldValue::Integer(_) => FieldType::Integer,
                crate::common::FieldValue::Long(_) => FieldType::Long,
                crate::common::FieldValue::Float(_) => FieldType::Float,
                crate::common::FieldValue::Double(_) => FieldType::Double,
                crate::common::FieldValue::Bool(_) => FieldType::Boolean,
                crate::common::FieldValue::Date(_) => FieldType::Date,
                crate::common::FieldValue::Array(arr) => arr
                    .first()
                    .map(|v| v.field_type())
                    .unwrap_or(FieldType::Unknown),
                crate::common::FieldValue::Null => FieldType::Unknown,
            };

            if field_type != FieldType::Unknown {
                mapping.add_field(FieldDefinition::new(name, field_type));
            }
        }

        mapping
    }
}

#[cfg(test)]
mod tests {
    use super::FieldMapper;
    use crate::common::{FieldDefinition, FieldType, FieldValue, Mapping};
    use crate::indexer::Error;
    use std::collections::HashMap;

    #[test]
    fn map_document_rejects_type_mismatch_for_integer_field() {
        let mut mapping = Mapping::new();
        mapping.add_field(FieldDefinition::new("count", FieldType::Integer));

        let fields = HashMap::from([("count".to_string(), FieldValue::Text("twelve".to_string()))]);

        let error = FieldMapper::map_document(&mapping, &fields).expect_err("mapping should fail");

        assert!(matches!(error, Error::InvalidField(field) if field == "count"));
    }

    #[test]
    fn map_document_rejects_unknown_analyzer_for_text_field() {
        let mut mapping = Mapping::new();
        mapping.add_field(
            FieldDefinition::new("title", FieldType::Text).with_analyzer("does-not-exist"),
        );

        let fields = HashMap::from([("title".to_string(), FieldValue::Text("hello".to_string()))]);

        let error = FieldMapper::map_document(&mapping, &fields).expect_err("mapping should fail");

        assert!(matches!(error, Error::Mapping(message) if message.contains("does-not-exist")));
    }

    #[test]
    fn map_document_accepts_supported_text_analyzer_and_matching_type() {
        let mut mapping = Mapping::new();
        mapping.add_field(FieldDefinition::new("title", FieldType::Text).with_analyzer("standard"));

        let fields = HashMap::from([("title".to_string(), FieldValue::Text("hello".to_string()))]);

        let mapped = FieldMapper::map_document(&mapping, &fields).expect("mapping should succeed");

        assert_eq!(
            mapped.get("title").and_then(FieldValue::as_text),
            Some("hello")
        );
    }
}
