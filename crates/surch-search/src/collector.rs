use thiserror::Error;

#[derive(Debug, Clone, PartialEq)]
pub struct ScoreDoc {
    pub doc_id: u32,
    pub score: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TopDocs {
    pub total_hits: usize,
    pub score_docs: Vec<ScoreDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CollectorError {
    #[error("top docs size must be greater than zero")]
    SizeMustBePositive,
    #[error("score for doc {doc_id} must be finite")]
    NonFiniteScore { doc_id: u32 },
}

#[derive(Debug, Clone)]
pub struct TopDocsCollector {
    size: usize,
    score_docs: Vec<ScoreDoc>,
}

impl TopDocsCollector {
    pub fn new(size: usize) -> Result<Self, CollectorError> {
        if size == 0 {
            return Err(CollectorError::SizeMustBePositive);
        }

        Ok(Self {
            size,
            score_docs: Vec::new(),
        })
    }

    pub fn collect(&mut self, doc_id: u32, score: f64) -> Result<(), CollectorError> {
        if !score.is_finite() {
            return Err(CollectorError::NonFiniteScore { doc_id });
        }

        self.score_docs.push(ScoreDoc { doc_id, score });
        Ok(())
    }

    pub fn finish(mut self) -> TopDocs {
        let total_hits = self.score_docs.len();
        self.score_docs.sort_by(|left, right| {
            right
                .score
                .total_cmp(&left.score)
                .then_with(|| left.doc_id.cmp(&right.doc_id))
        });
        self.score_docs.truncate(self.size);

        TopDocs {
            total_hits,
            score_docs: self.score_docs,
        }
    }
}
