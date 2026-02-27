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
        Ok(value.clone())
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
