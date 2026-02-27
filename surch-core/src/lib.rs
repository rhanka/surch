pub mod common;
pub mod indexer;
pub mod search;
pub mod storage;

pub use common::*;
pub use indexer::*;
pub use search::*;
pub use storage::*;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Storage error: {0}")]
    Storage(#[from] storage::Error),

    #[error("Indexer error: {0}")]
    Indexer(#[from] indexer::Error),

    #[error("Search error: {0}")]
    Search(#[from] search::Error),

    #[error("Document not found: {0}")]
    NotFound(String),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Index already exists: {0}")]
    IndexExists(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

impl serde::Serialize for Error {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
