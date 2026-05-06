use std::collections::BTreeMap;

use surch_analysis::{Analyzer, SimpleAnalyzer};
use thiserror::Error;

use crate::postings::{PostingsBuilder, PostingsEnum, PostingsError, TermDictionary, TermsEnum};
use crate::stored_fields::{StoredDocument, StoredFieldsError, StoredValue};

pub type Result<T> = std::result::Result<T, DocumentIndexError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DocumentIndexError {
    #[error("duplicate doc id: {doc_id}")]
    DuplicateDocId { doc_id: u32 },
    #[error("document field name must not be empty")]
    EmptyFieldName,
    #[error(transparent)]
    StoredFields(#[from] StoredFieldsError),
    #[error(transparent)]
    Postings(#[from] PostingsError),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct DocumentIndex {
    documents: BTreeMap<u32, StoredDocument>,
    postings_builder: PostingsBuilder,
    terms: TermDictionary,
}

impl DocumentIndex {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_document<I, K, V>(&mut self, doc_id: u32, fields: I) -> Result<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        if self.documents.contains_key(&doc_id) {
            return Err(DocumentIndexError::DuplicateDocId { doc_id });
        }

        let fields = fields
            .into_iter()
            .map(|(name, value)| (name.into(), value.into()))
            .collect::<Vec<_>>();

        for (name, _) in &fields {
            if name.trim().is_empty() {
                return Err(DocumentIndexError::EmptyFieldName);
            }
        }

        let mut document = StoredDocument::new();
        for (name, value) in &fields {
            document.insert(name.clone(), StoredValue::String(value.clone()))?;
        }

        let analyzer = SimpleAnalyzer;
        for (field, value) in &fields {
            for ((field, term), positions) in analyzed_terms(&analyzer, field, value) {
                self.postings_builder.add(field, term, doc_id, positions)?;
            }
        }

        self.documents.insert(doc_id, document);
        self.terms = self.postings_builder.clone().build();

        Ok(())
    }

    pub fn doc_ids(&self) -> Vec<u32> {
        self.documents.keys().copied().collect()
    }

    pub fn stored_document(&self, doc_id: u32) -> Option<&StoredDocument> {
        self.documents.get(&doc_id)
    }

    pub fn terms(&self, field: &str) -> TermsEnum<'_> {
        self.terms.terms(field)
    }

    pub fn postings(&self, field: &str, term: &str) -> Option<PostingsEnum<'_>> {
        self.terms.postings(field, term)
    }

    pub fn live_doc_count(&self) -> usize {
        self.documents.len()
    }

    pub fn live_docs(&self) -> Vec<u32> {
        self.doc_ids()
    }
}

fn analyzed_terms(
    analyzer: &impl Analyzer,
    field: &str,
    value: &str,
) -> BTreeMap<(String, String), Vec<u32>> {
    let mut terms = BTreeMap::<(String, String), Vec<u32>>::new();
    let mut position = 0_u32;

    for token in analyzer.token_stream(value) {
        position += token.position_increment;
        terms
            .entry((field.to_owned(), token.term))
            .or_default()
            .push(position - 1);
    }

    terms
}
