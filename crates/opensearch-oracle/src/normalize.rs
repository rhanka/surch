//! JSON normalization and comparison helpers for OpenSearch oracle responses.

use serde_json::{Number, Value};
use thiserror::Error;

/// Controls JSON normalization and comparison tolerance.
#[derive(Debug, Clone, PartialEq)]
pub struct NormalizeConfig {
    pub ignored_paths: Vec<String>,
    pub score_tolerance: f64,
}

impl Default for NormalizeConfig {
    fn default() -> Self {
        Self {
            ignored_paths: Vec::new(),
            score_tolerance: 0.0,
        }
    }
}

/// A detailed JSON comparison failure.
#[derive(Debug, Error, PartialEq)]
pub enum JsonCompareError {
    #[error("missing path `{path}` in actual response")]
    MissingActualPath { path: String },
    #[error("unexpected path `{path}` in actual response")]
    UnexpectedActualPath { path: String },
    #[error(
        "score mismatch at `{path}`: expected {expected}, actual {actual}, tolerance {tolerance}"
    )]
    ScoreMismatch {
        path: String,
        expected: f64,
        actual: f64,
        tolerance: f64,
    },
    #[error("value mismatch at `{path}`: expected {expected}, actual {actual}")]
    ValueMismatch {
        path: String,
        expected: Value,
        actual: Value,
    },
}

/// Returns a normalized copy of an OpenSearch JSON response.
pub fn normalize_response(value: &Value, config: &NormalizeConfig) -> Value {
    let mut normalized = value.clone();

    for ignored_path in &config.ignored_paths {
        let segments: Vec<&str> = ignored_path
            .split('.')
            .filter(|segment| !segment.is_empty())
            .collect();
        remove_path(&mut normalized, &segments);
    }

    normalized
}

/// Compares two OpenSearch JSON responses after applying normalization rules.
pub fn compare_json(
    expected: &Value,
    actual: &Value,
    config: &NormalizeConfig,
) -> Result<(), JsonCompareError> {
    let normalized_expected = normalize_response(expected, config);
    let normalized_actual = normalize_response(actual, config);

    compare_values(&normalized_expected, &normalized_actual, "", config)
}

fn remove_path(value: &mut Value, segments: &[&str]) {
    if segments.is_empty() {
        return;
    }

    match value {
        Value::Object(map) => {
            if segments.len() == 1 {
                if segments[0] == "*" {
                    map.clear();
                } else {
                    map.remove(segments[0]);
                }
                return;
            }

            if segments[0] == "*" {
                for child in map.values_mut() {
                    remove_path(child, &segments[1..]);
                }
            } else if let Some(child) = map.get_mut(segments[0]) {
                remove_path(child, &segments[1..]);
            }
        }
        Value::Array(values) => {
            if segments[0] == "*" {
                for child in values {
                    remove_path(child, &segments[1..]);
                }
            } else if let Ok(index) = segments[0].parse::<usize>() {
                if let Some(child) = values.get_mut(index) {
                    remove_path(child, &segments[1..]);
                }
            }
        }
        _ => {}
    }
}

fn compare_values(
    expected: &Value,
    actual: &Value,
    path: &str,
    config: &NormalizeConfig,
) -> Result<(), JsonCompareError> {
    match (expected, actual) {
        (Value::Object(expected_map), Value::Object(actual_map)) => {
            for (key, expected_child) in expected_map {
                let child_path = join_path(path, key);
                let actual_child =
                    actual_map
                        .get(key)
                        .ok_or_else(|| JsonCompareError::MissingActualPath {
                            path: child_path.clone(),
                        })?;
                compare_values(expected_child, actual_child, &child_path, config)?;
            }

            for key in actual_map.keys() {
                if !expected_map.contains_key(key) {
                    return Err(JsonCompareError::UnexpectedActualPath {
                        path: join_path(path, key),
                    });
                }
            }

            Ok(())
        }
        (Value::Array(expected_values), Value::Array(actual_values)) => {
            if expected_values.len() != actual_values.len() {
                return Err(JsonCompareError::ValueMismatch {
                    path: path.to_string(),
                    expected: Value::Array(expected_values.clone()),
                    actual: Value::Array(actual_values.clone()),
                });
            }

            for (index, (expected_child, actual_child)) in
                expected_values.iter().zip(actual_values).enumerate()
            {
                let child_path = join_path(path, &index.to_string());
                compare_values(expected_child, actual_child, &child_path, config)?;
            }

            Ok(())
        }
        (Value::Number(expected_number), Value::Number(actual_number)) if is_score_path(path) => {
            compare_score_numbers(expected_number, actual_number, path, config)
        }
        _ if expected == actual => Ok(()),
        _ => Err(JsonCompareError::ValueMismatch {
            path: path.to_string(),
            expected: expected.clone(),
            actual: actual.clone(),
        }),
    }
}

fn compare_score_numbers(
    expected: &Number,
    actual: &Number,
    path: &str,
    config: &NormalizeConfig,
) -> Result<(), JsonCompareError> {
    let Some(expected_score) = expected.as_f64() else {
        return compare_exact_numbers(expected, actual, path);
    };
    let Some(actual_score) = actual.as_f64() else {
        return compare_exact_numbers(expected, actual, path);
    };

    let tolerance = config.score_tolerance.max(0.0);
    if (expected_score - actual_score).abs() <= tolerance {
        Ok(())
    } else {
        Err(JsonCompareError::ScoreMismatch {
            path: path.to_string(),
            expected: expected_score,
            actual: actual_score,
            tolerance,
        })
    }
}

fn compare_exact_numbers(
    expected: &Number,
    actual: &Number,
    path: &str,
) -> Result<(), JsonCompareError> {
    if expected == actual {
        Ok(())
    } else {
        Err(JsonCompareError::ValueMismatch {
            path: path.to_string(),
            expected: Value::Number(expected.clone()),
            actual: Value::Number(actual.clone()),
        })
    }
}

fn is_score_path(path: &str) -> bool {
    path.split('.')
        .any(|segment| segment.contains("_score") || segment.contains("score"))
}

fn join_path(parent: &str, child: &str) -> String {
    if parent.is_empty() {
        child.to_string()
    } else {
        format!("{parent}.{child}")
    }
}
