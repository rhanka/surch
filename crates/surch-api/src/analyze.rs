//! OpenSearch-compatible `/_analyze` and `/{index}/_analyze` endpoints.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    Json,
};
use serde_json::{json, Value};
use surch_analysis::{
    Analyzer, KeywordAnalyzer, SimpleAnalyzer, StandardAnalyzer, StopAnalyzer, Token,
    WhitespaceAnalyzer,
};

use crate::{index::validate_index_name, state::AppState, OpenSearchError};

#[derive(Debug, Clone)]
struct AnalyzeRequest {
    analyzer: AnalyzerKind,
    texts: Vec<String>,
}

#[derive(Debug, Clone, Copy)]
enum AnalyzerKind {
    Standard,
    Simple,
    Stop,
    Keyword,
    Whitespace,
}

impl AnalyzerKind {
    fn from_name(name: &str) -> Result<Self, OpenSearchError> {
        match name {
            "standard" => Ok(Self::Standard),
            "simple" => Ok(Self::Simple),
            "stop" => Ok(Self::Stop),
            "keyword" => Ok(Self::Keyword),
            "whitespace" => Ok(Self::Whitespace),
            other => Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                format!("unknown analyzer `{other}`"),
            )),
        }
    }

    fn token_stream(self, text: &str) -> Vec<Token> {
        match self {
            Self::Standard => StandardAnalyzer.token_stream(text),
            Self::Simple => SimpleAnalyzer.token_stream(text),
            Self::Stop => StopAnalyzer.token_stream(text),
            Self::Keyword => KeywordAnalyzer.token_stream(text),
            Self::Whitespace => WhitespaceAnalyzer.token_stream(text),
        }
    }
}

/// Axum handler for `GET|POST /_analyze` (no path index).
pub async fn analyze_state_handler(
    State(_state): State<AppState>,
    body: String,
) -> impl IntoResponse {
    handle_analyze(&body)
}

/// Axum handler for `GET|POST /{index}/_analyze`.
pub async fn index_analyze_state_handler(
    State(state): State<AppState>,
    Path(target): Path<String>,
    body: String,
) -> impl IntoResponse {
    if let Err(error) = validate_index_name(&target) {
        return error.into_response();
    }
    if state.resolve_index(&target).is_empty() {
        return OpenSearchError::new(
            StatusCode::NOT_FOUND,
            "index_not_found_exception",
            format!("index [{target}] missing"),
        )
        .into_response();
    }
    handle_analyze(&body)
}

fn handle_analyze(body: &str) -> axum::response::Response {
    match parse_analyze_request(body) {
        Ok(request) => {
            let mut tokens: Vec<Value> = Vec::new();
            let mut position: i64 = -1;
            let mut text_offset_base = 0_usize;
            for text in &request.texts {
                for token in request.analyzer.token_stream(text) {
                    position += i64::from(token.position_increment);
                    tokens.push(json!({
                        "token": token.term,
                        "start_offset": token.start_offset + text_offset_base,
                        "end_offset": token.end_offset + text_offset_base,
                        "type": "<ALPHANUM>",
                        "position": position.max(0),
                    }));
                }
                text_offset_base += text.len();
            }
            (StatusCode::OK, Json(json!({ "tokens": tokens }))).into_response()
        }
        Err(error) => error.into_response(),
    }
}

fn parse_analyze_request(body: &str) -> Result<AnalyzeRequest, OpenSearchError> {
    if body.trim().is_empty() {
        return Err(OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_analyze request body must contain `text`",
        ));
    }

    let value: Value = serde_json::from_str(body).map_err(|error| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            error.to_string(),
        )
    })?;

    let object = value.as_object().ok_or_else(|| {
        OpenSearchError::new(
            StatusCode::BAD_REQUEST,
            "parsing_exception",
            "_analyze request body must be an object",
        )
    })?;

    let analyzer = match object.get("analyzer") {
        Some(Value::String(name)) if !name.is_empty() => AnalyzerKind::from_name(name)?,
        Some(Value::String(_)) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_analyze `analyzer` must not be empty",
            ));
        }
        Some(_) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_analyze `analyzer` must be a string",
            ));
        }
        None => AnalyzerKind::Standard,
    };

    let texts = match object.get("text") {
        Some(Value::String(text)) => vec![text.clone()],
        Some(Value::Array(items)) => {
            let mut texts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    Value::String(text) => texts.push(text.clone()),
                    _ => {
                        return Err(OpenSearchError::new(
                            StatusCode::BAD_REQUEST,
                            "parsing_exception",
                            "_analyze `text` array entries must be strings",
                        ));
                    }
                }
            }
            if texts.is_empty() {
                return Err(OpenSearchError::new(
                    StatusCode::BAD_REQUEST,
                    "parsing_exception",
                    "_analyze `text` must not be empty",
                ));
            }
            texts
        }
        Some(_) => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_analyze `text` must be a string or array of strings",
            ));
        }
        None => {
            return Err(OpenSearchError::new(
                StatusCode::BAD_REQUEST,
                "parsing_exception",
                "_analyze request body must contain `text`",
            ));
        }
    };

    Ok(AnalyzeRequest { analyzer, texts })
}
