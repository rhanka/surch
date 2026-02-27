use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Analyzer error: {0}")]
    Analyzer(String),

    #[error("Mapping error: {0}")]
    Mapping(String),

    #[error("Pipeline error: {0}")]
    Pipeline(String),

    #[error("Tokenization error: {0}")]
    Tokenization(String),

    #[error("Index not found: {0}")]
    IndexNotFound(String),

    #[error("Invalid field: {0}")]
    InvalidField(String),
}

impl serde::Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
