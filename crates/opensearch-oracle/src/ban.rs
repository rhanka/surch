//! Base Adresse Nationale fixture import helpers.

use std::collections::HashMap;

use serde_json::{Map, Value};
use thiserror::Error;

pub const BAN_DATASET_ID: &str = "5530fbacc751df5ff937dddb";
pub const BAN_LICENSE: &str = "lov2";
pub const BAN_SOURCE_CSV_URL: &str = "https://adresse.data.gouv.fr/data/ban/adresses/latest/csv";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BanProfile {
    Tiny,
    Sample,
    Full,
}

impl BanProfile {
    pub fn acquisition_plan(self) -> BanAcquisitionPlan {
        match self {
            Self::Tiny => BanAcquisitionPlan {
                name: "ban_tiny",
                max_records: Some(500),
                committable_fixture: true,
                cache_subdir: "ban/tiny",
            },
            Self::Sample => BanAcquisitionPlan {
                name: "ban_sample",
                max_records: Some(100_000),
                committable_fixture: false,
                cache_subdir: "ban/sample",
            },
            Self::Full => BanAcquisitionPlan {
                name: "ban_full",
                max_records: None,
                committable_fixture: false,
                cache_subdir: "ban/full",
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BanAcquisitionPlan {
    pub name: &'static str,
    pub max_records: Option<usize>,
    pub committable_fixture: bool,
    pub cache_subdir: &'static str,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BanRecord {
    pub id: String,
    pub house_number: Option<String>,
    pub street_name: String,
    pub postcode: String,
    pub city_code: Option<String>,
    pub city_name: String,
    pub longitude: f64,
    pub latitude: f64,
}

impl BanRecord {
    pub fn label(&self) -> String {
        match &self.house_number {
            Some(house_number) => format!(
                "{} {} {} {}",
                house_number, self.street_name, self.postcode, self.city_name
            ),
            None => format!("{} {} {}", self.street_name, self.postcode, self.city_name),
        }
    }
}

#[derive(Debug, Error)]
pub enum BanError {
    #[error("BAN CSV must contain a header row")]
    MissingHeader,
    #[error("missing required BAN column `{column}`")]
    MissingColumn { column: &'static str },
    #[error("BAN CSV row {row} has {actual} fields but header has {expected}")]
    WrongFieldCount {
        row: usize,
        expected: usize,
        actual: usize,
    },
    #[error("BAN CSV row {row} field `{field}` must not be empty")]
    EmptyRequiredField { row: usize, field: &'static str },
    #[error("BAN CSV row {row} field `{field}` is not a valid number: {value}")]
    InvalidNumber {
        row: usize,
        field: &'static str,
        value: String,
    },
    #[error("BAN CSV row {row} field `{field}` is outside the valid coordinate range: {value}")]
    InvalidCoordinate {
        row: usize,
        field: &'static str,
        value: String,
    },
    #[error("invalid CSV quoting on row {row}")]
    InvalidCsvQuoting { row: usize },
    #[error("bulk index name must not be empty")]
    EmptyIndexName,
}

pub fn parse_ban_csv(csv: &str) -> Result<Vec<BanRecord>, BanError> {
    let mut lines = csv.lines().filter(|line| !line.trim().is_empty());
    let Some(header_line) = lines.next() else {
        return Err(BanError::MissingHeader);
    };

    let delimiter = detect_delimiter(header_line);
    let header = split_csv_line(header_line, delimiter, 1)?;
    let columns = column_positions(&header);
    let required = RequiredBanColumns::from_header(&columns)?;

    let mut records = Vec::new();
    for (offset, line) in lines.enumerate() {
        let row = offset + 2;
        let fields = split_csv_line(line, delimiter, row)?;
        if fields.len() != header.len() {
            return Err(BanError::WrongFieldCount {
                row,
                expected: header.len(),
                actual: fields.len(),
            });
        }

        records.push(parse_record(row, &fields, &required)?);
    }

    Ok(records)
}

pub fn ban_records_to_bulk_ndjson(
    index_name: &str,
    records: &[BanRecord],
) -> Result<String, BanError> {
    if index_name.trim().is_empty() {
        return Err(BanError::EmptyIndexName);
    }

    let mut ndjson = String::new();
    for record in records {
        let action = serde_json::json!({
            "index": {
                "_id": record.id,
                "_index": index_name,
            }
        });
        let source = record_to_source(record);

        ndjson.push_str(&action.to_string());
        ndjson.push('\n');
        ndjson.push_str(&source.to_string());
        ndjson.push('\n');
    }

    Ok(ndjson)
}

fn record_to_source(record: &BanRecord) -> Value {
    let mut source = Map::new();
    source.insert("id".to_string(), Value::String(record.id.clone()));
    source.insert("label".to_string(), Value::String(record.label()));
    if let Some(house_number) = &record.house_number {
        source.insert(
            "house_number".to_string(),
            Value::String(house_number.clone()),
        );
    }
    source.insert(
        "street_name".to_string(),
        Value::String(record.street_name.clone()),
    );
    source.insert(
        "postcode".to_string(),
        Value::String(record.postcode.clone()),
    );
    if let Some(city_code) = &record.city_code {
        source.insert("city_code".to_string(), Value::String(city_code.clone()));
    }
    source.insert(
        "city_name".to_string(),
        Value::String(record.city_name.clone()),
    );
    source.insert("source".to_string(), Value::String("BAN".to_string()));
    source.insert(
        "location".to_string(),
        serde_json::json!({
            "lat": record.latitude,
            "lon": record.longitude,
        }),
    );
    Value::Object(source)
}

fn parse_record(
    row: usize,
    fields: &[String],
    required: &RequiredBanColumns,
) -> Result<BanRecord, BanError> {
    let id = required_text(row, "id", &fields[required.id])?;
    let street_name = required_text(row, "nom_voie", &fields[required.street_name])?;
    let postcode = required_text(row, "code_postal", &fields[required.postcode])?;
    let city_name = required_text(row, "nom_commune", &fields[required.city_name])?;
    let longitude = required_coordinate(row, "lon", &fields[required.longitude], -180.0, 180.0)?;
    let latitude = required_coordinate(row, "lat", &fields[required.latitude], -90.0, 90.0)?;

    let house_number = optional_house_number(
        fields
            .get(required.number)
            .map(String::as_str)
            .unwrap_or(""),
        required
            .suffix
            .and_then(|suffix| fields.get(suffix))
            .map(String::as_str)
            .unwrap_or(""),
    );
    let city_code = required
        .city_code
        .and_then(|index| optional_text(fields[index].as_str()));

    Ok(BanRecord {
        id,
        house_number,
        street_name,
        postcode,
        city_code,
        city_name,
        longitude,
        latitude,
    })
}

fn required_text(row: usize, field: &'static str, value: &str) -> Result<String, BanError> {
    optional_text(value).ok_or(BanError::EmptyRequiredField { row, field })
}

fn optional_text(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn required_number(row: usize, field: &'static str, value: &str) -> Result<f64, BanError> {
    let text = required_text(row, field, value)?;
    text.parse::<f64>().map_err(|_| BanError::InvalidNumber {
        row,
        field,
        value: text,
    })
}

fn required_coordinate(
    row: usize,
    field: &'static str,
    value: &str,
    min: f64,
    max: f64,
) -> Result<f64, BanError> {
    let coordinate = required_number(row, field, value)?;
    if (min..=max).contains(&coordinate) {
        Ok(coordinate)
    } else {
        Err(BanError::InvalidCoordinate {
            row,
            field,
            value: value.trim().to_string(),
        })
    }
}

fn optional_house_number(number: &str, suffix: &str) -> Option<String> {
    let number = number.trim();
    if number.is_empty() {
        return None;
    }

    let suffix = suffix.trim();
    if suffix.is_empty() {
        Some(number.to_string())
    } else {
        Some(format!("{number}{suffix}"))
    }
}

#[derive(Debug, Clone, Copy)]
struct RequiredBanColumns {
    id: usize,
    number: usize,
    suffix: Option<usize>,
    street_name: usize,
    postcode: usize,
    city_code: Option<usize>,
    city_name: usize,
    longitude: usize,
    latitude: usize,
}

impl RequiredBanColumns {
    fn from_header(columns: &HashMap<String, usize>) -> Result<Self, BanError> {
        Ok(Self {
            id: required_column(columns, "id")?,
            number: required_column(columns, "numero")?,
            suffix: columns.get("rep").copied(),
            street_name: required_column(columns, "nom_voie")?,
            postcode: required_column(columns, "code_postal")?,
            city_code: columns.get("code_insee").copied(),
            city_name: required_column(columns, "nom_commune")?,
            longitude: required_column(columns, "lon")?,
            latitude: required_column(columns, "lat")?,
        })
    }
}

fn required_column(
    columns: &HashMap<String, usize>,
    column: &'static str,
) -> Result<usize, BanError> {
    columns
        .get(column)
        .copied()
        .ok_or(BanError::MissingColumn { column })
}

fn column_positions(header: &[String]) -> HashMap<String, usize> {
    header
        .iter()
        .enumerate()
        .map(|(index, name)| (name.trim().to_string(), index))
        .collect()
}

fn detect_delimiter(header_line: &str) -> char {
    let comma_count = count_unquoted_delimiter(header_line, ',');
    let semicolon_count = count_unquoted_delimiter(header_line, ';');
    if semicolon_count > comma_count {
        ';'
    } else {
        ','
    }
}

fn count_unquoted_delimiter(line: &str, delimiter: char) -> usize {
    let mut in_quotes = false;
    let mut count = 0;
    let mut chars = line.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' {
            if in_quotes && chars.peek() == Some(&'"') {
                chars.next();
            } else {
                in_quotes = !in_quotes;
            }
        } else if ch == delimiter && !in_quotes {
            count += 1;
        }
    }

    count
}

fn split_csv_line(line: &str, delimiter: char, row: usize) -> Result<Vec<String>, BanError> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut in_quotes = false;
    let mut chars = line.trim_end_matches('\r').chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '"' {
            if in_quotes && chars.peek() == Some(&'"') {
                field.push('"');
                chars.next();
            } else {
                in_quotes = !in_quotes;
            }
        } else if ch == delimiter && !in_quotes {
            fields.push(field);
            field = String::new();
        } else {
            field.push(ch);
        }
    }

    if in_quotes {
        return Err(BanError::InvalidCsvQuoting { row });
    }

    fields.push(field);
    Ok(fields)
}
