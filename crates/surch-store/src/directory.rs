use std::collections::BTreeMap;

use crate::index_io::{ByteArrayIndexInput, ByteArrayIndexOutput};
use thiserror::Error;

pub type Result<T> = std::result::Result<T, DirectoryError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DirectoryError {
    #[error("file not found: {name}")]
    FileNotFound { name: String },
    #[error("file already exists: {name}")]
    AlreadyExists { name: String },
    #[error("file name must not be empty")]
    InvalidEmptyName,
}

#[derive(Debug, Clone, Default)]
pub struct MemoryDirectory {
    files: BTreeMap<String, Vec<u8>>,
}

impl MemoryDirectory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn create_output(&self, name: &str) -> Result<ByteArrayIndexOutput> {
        validate_name(name)?;
        if self.files.contains_key(name) {
            return Err(DirectoryError::AlreadyExists {
                name: name.to_string(),
            });
        }

        Ok(ByteArrayIndexOutput::new())
    }

    pub fn write_output(&mut self, name: &str, output: ByteArrayIndexOutput) -> Result<()> {
        validate_name(name)?;
        if self.files.contains_key(name) {
            return Err(DirectoryError::AlreadyExists {
                name: name.to_string(),
            });
        }

        self.files.insert(name.to_string(), output.into_inner());
        Ok(())
    }

    pub fn open_input(&self, name: &str) -> Result<ByteArrayIndexInput<'_>> {
        validate_name(name)?;
        self.files
            .get(name)
            .map(|bytes| ByteArrayIndexInput::new(bytes))
            .ok_or_else(|| DirectoryError::FileNotFound {
                name: name.to_string(),
            })
    }

    pub fn write_file(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        validate_name(name)?;
        if self.files.contains_key(name) {
            return Err(DirectoryError::AlreadyExists {
                name: name.to_string(),
            });
        }

        self.files.insert(name.to_string(), bytes.to_vec());
        Ok(())
    }

    pub fn read_file(&self, name: &str) -> Result<Vec<u8>> {
        validate_name(name)?;
        self.files
            .get(name)
            .cloned()
            .ok_or_else(|| DirectoryError::FileNotFound {
                name: name.to_string(),
            })
    }

    pub fn delete_file(&mut self, name: &str) -> Result<()> {
        validate_name(name)?;
        self.files
            .remove(name)
            .map(|_| ())
            .ok_or_else(|| DirectoryError::FileNotFound {
                name: name.to_string(),
            })
    }

    pub fn rename(&mut self, source: &str, target: &str) -> Result<()> {
        validate_name(source)?;
        validate_name(target)?;
        if !self.files.contains_key(source) {
            return Err(DirectoryError::FileNotFound {
                name: source.to_string(),
            });
        }
        if self.files.contains_key(target) {
            return Err(DirectoryError::AlreadyExists {
                name: target.to_string(),
            });
        }

        let bytes = self
            .files
            .remove(source)
            .expect("source presence checked before remove");
        self.files.insert(target.to_string(), bytes);
        Ok(())
    }

    pub fn list_all(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }

    pub fn file_length(&self, name: &str) -> Result<u64> {
        validate_name(name)?;
        self.files
            .get(name)
            .map(|bytes| bytes.len() as u64)
            .ok_or_else(|| DirectoryError::FileNotFound {
                name: name.to_string(),
            })
    }

    pub fn contains_file(&self, name: &str) -> Result<bool> {
        validate_name(name)?;
        Ok(self.files.contains_key(name))
    }
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(DirectoryError::InvalidEmptyName);
    }

    Ok(())
}
