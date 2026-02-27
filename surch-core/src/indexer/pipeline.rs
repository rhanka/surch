use std::sync::Arc;
use crate::common::{Document, FieldValue, Mapping};
use crate::indexer::analyzer::{Analyzer, AnalyzerRegistry, Token};
use crate::indexer::error::Error;
use crate::indexer::mapping::FieldMapper;

pub struct IndexingPipeline {
    registry: Arc<AnalyzerRegistry>,
    default_analyzer: String,
}

impl IndexingPipeline {
    pub fn new() -> Self {
        Self {
            registry: Arc::new(AnalyzerRegistry::new()),
            default_analyzer: "standard".to_string(),
        }
    }

    pub fn with_default_analyzer(mut self, analyzer: &str) -> Self {
        self.default_analyzer = analyzer.to_string();
        self
    }

    pub fn process(&self, doc: Document, mapping: &Mapping) -> Result<ProcessedDocument, Error> {
        let mut tokens: Vec<Token> = Vec::new();
        
        for (field_name, field_value) in &doc.fields {
            let analyzer_name = mapping
                .get_field(field_name)
                .and_then(|f| f.analyzer.clone())
                .unwrap_or_else(|| self.default_analyzer.clone());
            
            let analyzer = self.registry.get_or_default(&analyzer_name);
            let field_tokens = analyzer.analyze_field(field_name, field_value);
            tokens.extend(field_tokens);
        }
        
        Ok(ProcessedDocument {
            doc_id: doc.id,
            original_fields: doc.fields,
            tokens,
        })
    }

    pub fn analyze(&self, text: &str, analyzer_name: &str) -> Vec<Token> {
        let analyzer = self.registry.get_or_default(analyzer_name);
        analyzer.analyze(text)
    }

    pub fn registry(&self) -> &AnalyzerRegistry {
        &self.registry
    }
}

impl Default for IndexingPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct ProcessedDocument {
    pub doc_id: String,
    pub original_fields: std::collections::HashMap<String, FieldValue>,
    pub tokens: Vec<Token>,
}
