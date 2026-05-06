use thiserror::Error;

pub type Result<T> = std::result::Result<T, LiveDocsError>;

/// Errors returned when mutating or inspecting live-doc state.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LiveDocsError {
    /// The requested document id is greater than or equal to `max_doc`.
    #[error("doc_id {doc_id} is outside max_doc {max_doc}")]
    DocIdOutOfRange { doc_id: u32, max_doc: u32 },
}

/// Tracks Lucene-style live documents for a segment.
///
/// Every document id in `0..max_doc` starts live. Deletions are idempotent and
/// keep iteration sorted by ascending document id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveDocs {
    max_doc: u32,
    deleted: Vec<bool>,
    deleted_count: u32,
}

impl LiveDocs {
    /// Creates live-doc state with every document id in `0..max_doc` live.
    pub fn new(max_doc: u32) -> Self {
        Self {
            max_doc,
            deleted: vec![false; max_doc as usize],
            deleted_count: 0,
        }
    }

    /// Returns the exclusive upper bound for valid document ids.
    pub fn max_doc(&self) -> u32 {
        self.max_doc
    }

    /// Returns whether `doc_id` is currently live.
    pub fn is_live(&self, doc_id: u32) -> Result<bool> {
        self.validate_doc_id(doc_id)?;

        Ok(!self.deleted[doc_id as usize])
    }

    /// Marks `doc_id` as deleted.
    ///
    /// Returns `true` when this call changed the state and `false` when the
    /// document had already been deleted.
    pub fn delete(&mut self, doc_id: u32) -> Result<bool> {
        self.validate_doc_id(doc_id)?;

        let deleted = &mut self.deleted[doc_id as usize];
        if *deleted {
            return Ok(false);
        }

        *deleted = true;
        self.deleted_count += 1;

        Ok(true)
    }

    /// Returns the number of currently live documents.
    pub fn live_count(&self) -> u32 {
        self.max_doc - self.deleted_count
    }

    /// Returns the number of deleted documents.
    pub fn deleted_count(&self) -> u32 {
        self.deleted_count
    }

    /// Iterates live document ids in ascending order.
    pub fn iter_live_doc_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.deleted
            .iter()
            .enumerate()
            .filter_map(|(doc_id, deleted)| (!deleted).then_some(doc_id as u32))
    }

    fn validate_doc_id(&self, doc_id: u32) -> Result<()> {
        if doc_id >= self.max_doc {
            return Err(LiveDocsError::DocIdOutOfRange {
                doc_id,
                max_doc: self.max_doc,
            });
        }

        Ok(())
    }
}
