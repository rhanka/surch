use std::collections::{BTreeMap, BTreeSet};

use surch_analysis::{
    Analyzer, KeywordAnalyzer, SimpleAnalyzer, StandardAnalyzer, StopAnalyzer, WhitespaceAnalyzer,
};
use thiserror::Error;

use crate::mapping::{AnalyzerName, FieldType, IndexMapping};
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
        self.add_document_with_mapping(doc_id, fields, &IndexMapping::default())
    }

    pub fn add_document_with_mapping<I, K, V>(
        &mut self,
        doc_id: u32,
        fields: I,
        mapping: &IndexMapping,
    ) -> Result<()>
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        self.add_documents_with_mapping([(doc_id, fields)], mapping)
    }

    pub fn add_documents_with_mapping<D, I, K, V>(
        &mut self,
        documents: D,
        mapping: &IndexMapping,
    ) -> Result<()>
    where
        D: IntoIterator<Item = (u32, I)>,
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let mut seen = BTreeSet::new();
        let mut documents = documents
            .into_iter()
            .map(|(doc_id, fields)| {
                if self.documents.contains_key(&doc_id) || !seen.insert(doc_id) {
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

                Ok((doc_id, fields))
            })
            .collect::<Result<Vec<_>>>()?;

        for (doc_id, fields) in documents.drain(..) {
            self.add_validated_document(doc_id, fields, mapping)?;
        }

        self.terms = self.postings_builder.clone().build();

        Ok(())
    }

    pub fn clear(&mut self) {
        self.documents.clear();
        self.postings_builder = PostingsBuilder::new();
        self.terms = TermDictionary::default();
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

    fn add_validated_document(
        &mut self,
        doc_id: u32,
        fields: Vec<(String, String)>,
        mapping: &IndexMapping,
    ) -> Result<()> {
        let mut document = StoredDocument::new();
        for (name, value) in &fields {
            document.insert(name.clone(), StoredValue::String(value.clone()))?;
        }

        for (field, value) in &fields {
            let field_mapping = mapping.field(field);
            let analyzer = field_mapping
                .map_or(AnalyzerName::default_for(FieldType::Text), |field| {
                    field.analyzer()
                });

            for ((field, term), positions) in analyzed_terms(analyzer, field, value) {
                self.postings_builder.add(field, term, doc_id, positions)?;
            }
        }

        self.documents.insert(doc_id, document);
        Ok(())
    }
}

fn analyzed_terms(
    analyzer: AnalyzerName,
    field: &str,
    value: &str,
) -> BTreeMap<(String, String), Vec<u32>> {
    let tokenized = match analyzer {
        AnalyzerName::Standard => StandardAnalyzer.token_stream(value),
        AnalyzerName::Simple => SimpleAnalyzer.token_stream(value),
        AnalyzerName::Stop => StopAnalyzer.token_stream(value),
        AnalyzerName::Keyword => KeywordAnalyzer.token_stream(value),
        AnalyzerName::Whitespace => WhitespaceAnalyzer.token_stream(value),
    };

    let mut terms = BTreeMap::<(String, String), Vec<u32>>::new();
    let mut position = 0_u32;

    for token in tokenized {
        position += token.position_increment;
        terms
            .entry((field.to_owned(), token.term))
            .or_default()
            .push(position - 1);
    }

    terms
}
