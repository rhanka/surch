use std::collections::BTreeMap;

use thiserror::Error;

pub type Result<T> = std::result::Result<T, PostingsError>;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum PostingsError {
    #[error("field name must not be empty")]
    EmptyField,
    #[error("term must not be empty")]
    EmptyTerm,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Posting {
    pub doc_id: u32,
    pub freq: u32,
    pub positions: Vec<u32>,
}

impl Posting {
    pub fn new(doc_id: u32, positions: Vec<u32>) -> Self {
        let freq = if positions.is_empty() {
            1
        } else {
            positions.len() as u32
        };

        Self {
            doc_id,
            freq,
            positions,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct PostingsBuilder {
    fields: BTreeMap<String, BTreeMap<String, Vec<Posting>>>,
}

impl PostingsBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(
        &mut self,
        field: impl Into<String>,
        term: impl Into<String>,
        doc_id: u32,
        positions: Vec<u32>,
    ) -> Result<()> {
        let field = field.into();
        if field.trim().is_empty() {
            return Err(PostingsError::EmptyField);
        }

        let term = term.into();
        if term.trim().is_empty() {
            return Err(PostingsError::EmptyTerm);
        }

        self.fields
            .entry(field)
            .or_default()
            .entry(term)
            .or_default()
            .push(Posting::new(doc_id, positions));

        Ok(())
    }

    pub fn build(mut self) -> TermDictionary {
        for terms in self.fields.values_mut() {
            for postings in terms.values_mut() {
                postings.sort_by_key(|posting| posting.doc_id);
            }
        }

        TermDictionary {
            fields: self.fields,
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct TermDictionary {
    fields: BTreeMap<String, BTreeMap<String, Vec<Posting>>>,
}

impl TermDictionary {
    pub fn terms(&self, field: &str) -> TermsEnum<'_> {
        let terms = self
            .fields
            .get(field)
            .map(|terms| terms.keys().map(String::as_str).collect())
            .unwrap_or_default();

        TermsEnum { terms, position: 0 }
    }

    pub fn postings(&self, field: &str, term: &str) -> Option<PostingsEnum<'_>> {
        self.fields
            .get(field)
            .and_then(|terms| terms.get(term))
            .map(|postings| PostingsEnum {
                postings,
                position: 0,
            })
    }
}

#[derive(Debug, Clone)]
pub struct TermsEnum<'a> {
    terms: Vec<&'a str>,
    position: usize,
}

impl<'a> Iterator for TermsEnum<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<Self::Item> {
        let term = self.terms.get(self.position).copied();
        self.position += usize::from(term.is_some());
        term
    }
}

#[derive(Debug, Clone)]
pub struct PostingsEnum<'a> {
    postings: &'a [Posting],
    position: usize,
}

impl<'a> Iterator for PostingsEnum<'a> {
    type Item = &'a Posting;

    fn next(&mut self) -> Option<Self::Item> {
        let posting = self.postings.get(self.position);
        self.position += usize::from(posting.is_some());
        posting
    }
}
