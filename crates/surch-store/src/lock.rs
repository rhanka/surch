use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use thiserror::Error;

/// Errors returned by Lucene-like lock factories.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum LockError {
    #[error("lock name must not be empty")]
    InvalidEmptyName,
    #[error("lock is already held: {name}")]
    AlreadyLocked { name: String },
}

/// In-memory lock factory with Lucene-style single-holder lock semantics.
#[derive(Clone, Debug, Default)]
pub struct MemoryLockFactory {
    locked_names: Arc<Mutex<HashSet<String>>>,
}

impl MemoryLockFactory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn obtain_lock(&self, name: impl Into<String>) -> Result<MemoryLockGuard, LockError> {
        let name = name.into();
        validate_lock_name(&name)?;

        let mut locked_names = self
            .locked_names
            .lock()
            .expect("memory lock factory mutex poisoned");

        if locked_names.contains(&name) {
            return Err(LockError::AlreadyLocked { name });
        }

        locked_names.insert(name.clone());

        Ok(MemoryLockGuard {
            name,
            locked_names: Arc::clone(&self.locked_names),
        })
    }
}

/// RAII lock guard that releases its lock name on drop.
#[derive(Debug)]
pub struct MemoryLockGuard {
    name: String,
    locked_names: Arc<Mutex<HashSet<String>>>,
}

impl Drop for MemoryLockGuard {
    fn drop(&mut self) {
        if let Ok(mut locked_names) = self.locked_names.lock() {
            locked_names.remove(&self.name);
        }
    }
}

fn validate_lock_name(name: &str) -> Result<(), LockError> {
    if name.is_empty() {
        return Err(LockError::InvalidEmptyName);
    }

    Ok(())
}
