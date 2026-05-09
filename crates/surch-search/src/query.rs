//! Query wrappers and rewrite helpers.

use thiserror::Error;

use crate::fuzzy::{expand_fuzzy_terms, FuzzyError, FuzzyQueryConfig};

/// Exact term query wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TermQuery {
    pub field: String,
    pub value: String,
}

/// Fuzzy term query wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FuzzyQuery {
    pub field: String,
    pub value: String,
    pub config: FuzzyQueryConfig,
}

/// Minimal boolean query wrapper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BooleanQuery {
    pub must: Vec<Query>,
}

/// P0 query wrapper enum for exact, fuzzy, and boolean queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Query {
    Term(TermQuery),
    Fuzzy(FuzzyQuery),
    Boolean(BooleanQuery),
}

/// Errors returned by query wrappers and rewrites.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum QueryError {
    /// Query fields must be explicit.
    #[error("field must not be empty")]
    EmptyField,
    /// Query values must be explicit.
    #[error("value must not be empty")]
    EmptyValue,
    /// Boolean queries require at least one required clause.
    #[error("must clauses must not be empty")]
    EmptyMustClauses,
    /// Fuzzy expansion failed.
    #[error(transparent)]
    Fuzzy(#[from] FuzzyError),
}

impl TermQuery {
    /// Creates a term query after validating its field and value.
    pub fn new(field: impl Into<String>, value: impl Into<String>) -> Result<Self, QueryError> {
        let field = field.into();
        let value = value.into();
        validate_field_value(&field, &value)?;

        Ok(Self { field, value })
    }
}

impl FuzzyQuery {
    /// Creates a fuzzy query after validating its field and value.
    pub fn new(
        field: impl Into<String>,
        value: impl Into<String>,
        config: FuzzyQueryConfig,
    ) -> Result<Self, QueryError> {
        let field = field.into();
        let value = value.into();
        validate_field_value(&field, &value)?;

        Ok(Self {
            field,
            value,
            config,
        })
    }
}

impl BooleanQuery {
    /// Creates a boolean query after validating its required clauses.
    pub fn new(must: Vec<Query>) -> Result<Self, QueryError> {
        if must.is_empty() {
            return Err(QueryError::EmptyMustClauses);
        }

        Ok(Self { must })
    }
}

/// Rewrites a fuzzy query into exact term queries using fuzzy term expansion.
pub fn rewrite_fuzzy_query<'a>(
    query: &FuzzyQuery,
    terms: impl IntoIterator<Item = &'a str>,
) -> Result<Vec<TermQuery>, QueryError> {
    validate_field_value(&query.field, &query.value)?;

    expand_fuzzy_terms(&query.value, terms, query.config)?
        .into_iter()
        .map(|term_match| TermQuery::new(query.field.clone(), term_match.term))
        .collect()
}

fn validate_field_value(field: &str, value: &str) -> Result<(), QueryError> {
    if field.is_empty() {
        return Err(QueryError::EmptyField);
    }

    if value.is_empty() {
        return Err(QueryError::EmptyValue);
    }

    Ok(())
}
