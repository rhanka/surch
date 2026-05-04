use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Query error: {0}")]
    Query(String),

    #[error("Scoring error: {0}")]
    Scoring(String),

    #[error("Collector error: {0}")]
    Collector(String),

    #[error("Parse error: {0}")]
    Parse(String),

    #[error("Fuzzy error: {0}")]
    Fuzzy(String),

    #[error("Invalid query: {0}")]
    InvalidQuery(String),
}

impl serde::Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
