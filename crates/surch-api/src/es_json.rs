//! Lot L7: JSON serialization of search responses with ES 8.6.1's float
//! rendering of `_score` and `max_score`.
//!
//! ES prints those two fields as a Java `float` through Jackson
//! (`Float.toString`, JDK 19 shortest decimal — see
//! `surch_search::java_float`), e.g. `1.0E-4`, `1.2345678E7`, `0.33333334`.
//! Surch carries every score as an `f32` widened to `f64`; serde_json would
//! print that `f64` with its own shortest-`f64` digits (`0.3333333432674408`,
//! `1e-4`). [`to_es_json_vec`] serializes with a [`serde_json::ser::Formatter`]
//! that recognises the value of an object key named `_score` / `max_score`
//! and writes it with `Float.toString` when it is an exact `f32` value; every
//! other number keeps serde_json's rendering.

use std::io;

use serde::Serialize;
use serde_json::ser::{CompactFormatter, Formatter};
use surch_search::java_float::java_float_to_string;

/// serde_json formatter rendering `_score` / `max_score` like ES 8.6.1.
#[derive(Debug, Default)]
struct EsScoreFormatter {
    /// Key being written (only while inside an object key).
    key: String,
    in_key: bool,
    /// The value currently being written belongs to `_score` / `max_score`.
    score_value: bool,
}

impl Formatter for EsScoreFormatter {
    fn begin_object_key<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        first: bool,
    ) -> io::Result<()> {
        self.key.clear();
        self.in_key = true;
        CompactFormatter.begin_object_key(writer, first)
    }

    fn end_object_key<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.in_key = false;
        CompactFormatter.end_object_key(writer)
    }

    fn write_string_fragment<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        fragment: &str,
    ) -> io::Result<()> {
        if self.in_key {
            self.key.push_str(fragment);
        }
        CompactFormatter.write_string_fragment(writer, fragment)
    }

    fn write_char_escape<W: ?Sized + io::Write>(
        &mut self,
        writer: &mut W,
        char_escape: serde_json::ser::CharEscape,
    ) -> io::Result<()> {
        if self.in_key {
            // An escaped character never belongs to `_score` / `max_score`.
            self.key.push('\u{0}');
        }
        CompactFormatter.write_char_escape(writer, char_escape)
    }

    fn begin_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = self.key == "_score" || self.key == "max_score";
        CompactFormatter.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = false;
        CompactFormatter.end_object_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = false;
        CompactFormatter.begin_object(writer)
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = false;
        CompactFormatter.begin_array(writer)
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        let as_float = value as f32;
        if self.score_value && f64::from(as_float) == value {
            return writer.write_all(java_float_to_string(as_float).as_bytes());
        }
        CompactFormatter.write_f64(writer, value)
    }
}

/// `serde_json::to_vec` with ES 8.6.1's `_score` / `max_score` rendering.
pub(crate) fn to_es_json_vec<T: ?Sized + Serialize>(value: &T) -> serde_json::Result<Vec<u8>> {
    let mut out = Vec::with_capacity(256);
    let mut serializer =
        serde_json::Serializer::with_formatter(&mut out, EsScoreFormatter::default());
    value.serialize(&mut serializer)?;
    Ok(out)
}

/// An `application/json` HTTP 200 response from [`to_es_json_vec`].
pub(crate) fn es_json_response<T: ?Sized + Serialize>(value: &T) -> axum::response::Response {
    use axum::response::IntoResponse;
    match to_es_json_vec(value) {
        Ok(bytes) => axum::http::Response::builder()
            .status(axum::http::StatusCode::OK)
            .header(axum::http::header::CONTENT_TYPE, "application/json")
            .body(axum::body::Body::from(bytes))
            .expect("static response parts are valid")
            .into_response(),
        Err(_) => (
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "response serialization failed",
        )
            .into_response(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::to_es_json_vec;

    #[test]
    fn scores_render_as_java_floats_other_numbers_untouched() {
        let third = f64::from(1.0_f32 / 3.0);
        let body = json!({
            "hits": {
                "max_score": f64::from(12_345_678.0_f32),
                "hits": [
                    {"_id": "a", "_score": third, "sort": [third]},
                    {"_id": "b", "_score": 1.0, "other": 0.000_1},
                    {"_id": "c", "_score": f64::from(1.0e-4_f32)},
                    {"_id": "d", "_score": null}
                ]
            },
            "took": 3
        });
        let text = String::from_utf8(to_es_json_vec(&body).expect("serializes")).expect("utf8");
        assert_eq!(
            text,
            "{\"hits\":{\"hits\":[\
             {\"_id\":\"a\",\"_score\":0.33333334,\"sort\":[0.3333333432674408]},\
             {\"_id\":\"b\",\"_score\":1.0,\"other\":0.0001},\
             {\"_id\":\"c\",\"_score\":1.0E-4},\
             {\"_id\":\"d\",\"_score\":null}],\
             \"max_score\":1.2345678E7},\"took\":3}"
        );
    }

    #[test]
    fn a_non_float_f64_score_keeps_the_serde_rendering() {
        let text = String::from_utf8(to_es_json_vec(&json!({"_score": 0.1_f64})).expect("ok"))
            .expect("utf8");
        assert_eq!(text, "{\"_score\":0.1}");
    }
}
