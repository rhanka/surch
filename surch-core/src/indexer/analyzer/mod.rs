mod keyword;
mod simple;
mod standard;
mod stop;

pub use keyword::KeywordAnalyzer;
pub use simple::SimpleAnalyzer;
pub use standard::StandardAnalyzer;
pub use stop::StopAnalyzer;

use crate::common::FieldValue;

pub trait Analyzer: Send + Sync {
    fn name(&self) -> &str;
    fn analyze(&self, text: &str) -> Vec<Token>;
    fn analyze_field(&self, field: &str, value: &FieldValue) -> Vec<Token> {
        if let Some(text) = value.as_text() {
            self.analyze(text)
                .into_iter()
                .map(|mut t| {
                    t.field = field.to_string();
                    t
                })
                .collect()
        } else {
            Vec::new()
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub text: String,
    pub field: String,
    pub start_offset: usize,
    pub end_offset: usize,
    pub position: usize,
    pub term_freq: u32,
    pub pos_increment: u32,
}

impl Token {
    pub fn new(text: String, field: String, position: usize) -> Self {
        Self {
            text,
            field,
            start_offset: 0,
            end_offset: 0,
            position,
            term_freq: 1,
            pos_increment: 1,
        }
    }
}

pub struct AnalyzerRegistry {
    analyzers: std::collections::HashMap<String, Box<dyn Analyzer>>,
}

impl AnalyzerRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            analyzers: std::collections::HashMap::new(),
        };

        registry.register("standard", Box::new(StandardAnalyzer::new()));
        registry.register("simple", Box::new(SimpleAnalyzer::new()));
        registry.register("stop", Box::new(StopAnalyzer::new()));
        registry.register("keyword", Box::new(KeywordAnalyzer::new()));

        registry
    }

    pub fn register(&mut self, name: &str, analyzer: Box<dyn Analyzer>) {
        self.analyzers.insert(name.to_string(), analyzer);
    }

    pub fn get(&self, name: &str) -> Option<&dyn Analyzer> {
        self.analyzers.get(name).map(|a| a.as_ref())
    }

    pub fn get_or_default(&self, name: &str) -> &dyn Analyzer {
        self.get(name)
            .unwrap_or_else(|| self.get("standard").unwrap())
    }
}

impl Default for AnalyzerRegistry {
    fn default() -> Self {
        Self::new()
    }
}
