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
    /// L2b: the value about to be written belongs to a `sort` key.
    sort_value: bool,
    /// L2b: inside a hit's `sort` array. A `_score` sort value is a Java
    /// `Float` there too (`SearchSortValues.toXContent`,
    /// SearchSortValues.java:67-76, `builder.value(Object)`).
    in_sort_array: bool,
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
        self.sort_value = self.key == "sort";
        CompactFormatter.begin_object_value(writer)
    }

    fn end_object_value<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = false;
        self.sort_value = false;
        CompactFormatter.end_object_value(writer)
    }

    fn begin_object<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = false;
        self.sort_value = false;
        self.in_sort_array = false;
        CompactFormatter.begin_object(writer)
    }

    fn begin_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.score_value = false;
        self.in_sort_array = self.sort_value;
        self.sort_value = false;
        CompactFormatter.begin_array(writer)
    }

    fn end_array<W: ?Sized + io::Write>(&mut self, writer: &mut W) -> io::Result<()> {
        self.in_sort_array = false;
        CompactFormatter.end_array(writer)
    }

    fn write_f64<W: ?Sized + io::Write>(&mut self, writer: &mut W, value: f64) -> io::Result<()> {
        let as_float = value as f32;
        if (self.score_value || self.in_sort_array) && f64::from(as_float) == value {
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

/// ES 8.6.1 default `format` of a `date` field
/// (`DateFieldMapper.DEFAULT_DATE_TIME_FORMATTER`).
pub(crate) const ES_DEFAULT_DATE_FORMAT: &str = "strict_date_optional_time||epoch_millis";

/// Days since 1970-01-01 for a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
pub(crate) fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let y = if month <= 2 { year - 1 } else { year };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let m = i64::from(month);
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `(year, month, day)` of a day number since 1970-01-01 (inverse of
/// [`days_from_civil`]).
pub(crate) fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// The Java `DateTimeFormatter` pattern ES 8.6.1 prints a built-in named
/// format with (`DateFormatters`), `None` for a custom pattern. Only the
/// names a `date` field or aggregation is likely to carry are listed; the
/// `epoch_*` formats are handled by [`format_es_date`] directly.
fn named_format_pattern(name: &str) -> Option<&'static str> {
    Some(match name {
        "strict_date_optional_time"
        | "date_optional_time"
        | "strict_date_optional_time_nanos"
        | "strict_date_time"
        | "date_time" => "yyyy-MM-dd'T'HH:mm:ss.SSSX",
        "strict_date_time_no_millis" | "date_time_no_millis" => "yyyy-MM-dd'T'HH:mm:ssX",
        "strict_date" | "date" | "strict_year_month_day" | "year_month_day" => "yyyy-MM-dd",
        "strict_year_month" | "year_month" => "yyyy-MM",
        "strict_year" | "year" => "yyyy",
        "basic_date" => "yyyyMMdd",
        "basic_date_time" => "yyyyMMdd'T'HHmmss.SSSX",
        "strict_hour_minute_second" | "hour_minute_second" => "HH:mm:ss",
        _ => return None,
    })
}

/// L2b: print an epoch-millisecond instant (UTC) the way ES 8.6.1's
/// `DocValueFormat.DateTime.format` does for `key_as_string` and
/// `creation.date.string`: the first `||` alternative of `format`, a
/// built-in name ([`named_format_pattern`], `epoch_millis`,
/// `epoch_second`) or a Java `DateTimeFormatter` pattern (letters `y u M
/// d H m s S X Z x`, `'quoted'` literals, any other character verbatim).
pub(crate) fn format_es_date(millis: i64, format: &str) -> String {
    let format = format.split("||").next().unwrap_or(format).trim();
    match format {
        "epoch_millis" => return millis.to_string(),
        "epoch_second" => {
            let seconds = millis.div_euclid(1000);
            let fraction = millis.rem_euclid(1000);
            return if fraction == 0 {
                seconds.to_string()
            } else {
                format!(
                    "{seconds}.{}",
                    format!("{fraction:03}").trim_end_matches('0')
                )
            };
        }
        _ => {}
    }
    let pattern = named_format_pattern(format).unwrap_or(format);
    let days = millis.div_euclid(86_400_000);
    let in_day = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = in_day / 3_600_000;
    let minute = (in_day / 60_000) % 60;
    let second = (in_day / 1000) % 60;
    let milli = in_day % 1000;

    let chars: Vec<char> = pattern.chars().collect();
    let mut out = String::with_capacity(pattern.len() + 8);
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\'' {
            // Quoted literal; `''` is a single quote.
            i += 1;
            if i < chars.len() && chars[i] == '\'' {
                out.push('\'');
                i += 1;
                continue;
            }
            while i < chars.len() {
                if chars[i] == '\'' {
                    if i + 1 < chars.len() && chars[i + 1] == '\'' {
                        out.push('\'');
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                out.push(chars[i]);
                i += 1;
            }
            continue;
        }
        if !c.is_ascii_alphabetic() {
            out.push(c);
            i += 1;
            continue;
        }
        let mut run = 1;
        while i + run < chars.len() && chars[i + run] == c {
            run += 1;
        }
        i += run;
        match c {
            'y' | 'u' => {
                if run == 2 {
                    out.push_str(&format!("{:02}", year.rem_euclid(100)));
                } else {
                    out.push_str(&format!("{year:0run$}"));
                }
            }
            'M' => out.push_str(&format!("{month:0run$}")),
            'd' => out.push_str(&format!("{day:0run$}")),
            'H' => out.push_str(&format!("{hour:0run$}")),
            'm' => out.push_str(&format!("{minute:0run$}")),
            's' => out.push_str(&format!("{second:0run$}")),
            'S' => {
                // Fraction of second, truncated / zero-padded to `run` digits.
                let digits = format!("{milli:03}000000");
                out.push_str(&digits[..run.min(9)]);
            }
            // UTC offset: `X`/`XXX` print `Z`, `Z` prints `+0000`,
            // `ZZZZZ` prints `Z`, `x` prints `+00`, `xxx` `+00:00`.
            'X' => out.push('Z'),
            'Z' => out.push_str(if run >= 5 { "Z" } else { "+0000" }),
            'x' => out.push_str(match run {
                1 => "+00",
                2 => "+0000",
                _ => "+00:00",
            }),
            other => {
                for _ in 0..run {
                    out.push(other);
                }
            }
        }
    }
    out
}

/// L2b: parse a date string to days since 1970-01-01 with ONE ES format
/// (no `||`): `epoch_millis`, `epoch_second`, the date-only built-in
/// names, the `strict_date_optional_time` family (date part, optional
/// `T...` time ignored at day granularity), or a pattern made of `yyyy`,
/// `MM`, `dd` and literal separators. `None` when the text does not
/// match.
pub(crate) fn parse_es_date_days(text: &str, format: &str) -> Option<i64> {
    match format {
        "epoch_millis" => return text.parse::<i64>().ok().map(|m| m.div_euclid(86_400_000)),
        "epoch_second" => return text.parse::<i64>().ok().map(|s| s.div_euclid(86_400)),
        "strict_date_optional_time"
        | "date_optional_time"
        | "strict_date_optional_time_nanos"
        | "strict_date_time"
        | "date_time" => {
            let date = text.split('T').next().unwrap_or(text);
            return parse_date_pattern(date, "yyyy-MM-dd")
                .or_else(|| parse_date_pattern(date, "yyyy-MM").map(|(y, m, _)| (y, m, 1)))
                .or_else(|| parse_date_pattern(date, "yyyy").map(|(y, _, _)| (y, 1, 1)))
                .map(|(y, m, d)| days_from_civil(y, m, d));
        }
        _ => {}
    }
    let pattern = named_format_pattern(format).unwrap_or(format);
    let (y, m, d) = parse_date_pattern(text, pattern)?;
    Some(days_from_civil(y, m, d))
}

/// `(year, month, day)` of `text` under a pattern of `yyyy` / `MM` / `dd`
/// fields and literal characters (`'T'` quotes dropped); time fields in
/// the pattern are skipped as digits. Months / days default to 1.
fn parse_date_pattern(text: &str, pattern: &str) -> Option<(i64, u32, u32)> {
    let pattern: String = pattern.chars().filter(|c| *c != '\'').collect();
    let text = text.as_bytes();
    let pat = pattern.as_bytes();
    let (mut year, mut month, mut day) = (None, 1_u32, 1_u32);
    let (mut ti, mut pi) = (0, 0);
    while pi < pat.len() {
        let c = pat[pi];
        let mut run = 1;
        while pi + run < pat.len() && pat[pi + run] == c {
            run += 1;
        }
        if c.is_ascii_alphabetic() && c != b'T' {
            let digits = text.get(ti..ti + run)?;
            if !digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            let value: i64 = std::str::from_utf8(digits).ok()?.parse().ok()?;
            match c {
                b'y' | b'u' => year = Some(value),
                b'M' => month = u32::try_from(value).ok()?,
                b'd' => day = u32::try_from(value).ok()?,
                _ => {}
            }
            ti += run;
        } else {
            for _ in 0..run {
                if text.get(ti) != Some(&c) {
                    return None;
                }
                ti += 1;
            }
        }
        pi += run;
    }
    if ti != text.len() || !(1..=12).contains(&month) {
        return None;
    }
    let year = year?;
    let next_month_start = if month == 12 {
        days_from_civil(year + 1, 1, 1)
    } else {
        days_from_civil(year, month + 1, 1)
    };
    let month_len = next_month_start - days_from_civil(year, month, 1);
    if day == 0 || i64::from(day) > month_len {
        return None;
    }
    Some((year, month, day))
}

#[cfg(test)]
mod date_tests {
    use super::{civil_from_days, days_from_civil, format_es_date, parse_es_date_days};

    #[test]
    fn civil_round_trip_and_epoch() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(1969, 12, 31), -1);
        assert_eq!(civil_from_days(-25_567), (1900, 1, 1));
        for days in [-80_000_i64, -1, 0, 1, 18_000, 20_000] {
            let (y, m, d) = civil_from_days(days);
            assert_eq!(days_from_civil(y, m, d), days);
        }
    }

    // es-source-derived: `DateFormatters` strict_date_optional_time printer
    // and Java `DateTimeFormatter` pattern letters (JDK 19).
    #[test]
    fn formats_like_es_date_formatter() {
        let millis = days_from_civil(2026, 11, 15) * 86_400_000 + 36_733_123;
        assert_eq!(
            format_es_date(millis, "strict_date_time"),
            "2026-11-15T10:12:13.123Z"
        );
        assert_eq!(
            format_es_date(millis, "strict_date_optional_time||epoch_millis"),
            "2026-11-15T10:12:13.123Z"
        );
        assert_eq!(format_es_date(millis, "yyyyMMdd"), "20261115");
        assert_eq!(format_es_date(millis, "epoch_millis"), millis.to_string());
        let before_epoch = days_from_civil(1931, 2, 1) * 86_400_000;
        assert_eq!(format_es_date(before_epoch, "yyyyMMdd"), "19310201");
        assert_eq!(
            format_es_date(before_epoch, "yyyy-MM-dd'T'HH:mm"),
            "1931-02-01T00:00"
        );
    }

    #[test]
    fn parses_field_formats_at_day_granularity() {
        let day = days_from_civil(1941, 8, 13);
        assert_eq!(parse_es_date_days("19410813", "yyyyMMdd"), Some(day));
        assert_eq!(
            parse_es_date_days("1941-08-13", "strict_date_optional_time"),
            Some(day)
        );
        assert_eq!(
            parse_es_date_days("1941-08-13T10:00:00Z", "strict_date_optional_time"),
            Some(day)
        );
        assert_eq!(parse_es_date_days("19410230", "yyyyMMdd"), None);
        assert_eq!(parse_es_date_days("1941081", "yyyyMMdd"), None);
        assert_eq!(parse_es_date_days("0", "epoch_millis"), Some(0));
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
        // L2b: `preserve_order` keeps the literal key order, and a `_score`
        // sort value is a Java `Float` too (SearchSortValues.java:67-76).
        assert_eq!(
            text,
            "{\"hits\":{\"max_score\":1.2345678E7,\"hits\":[\
             {\"_id\":\"a\",\"_score\":0.33333334,\"sort\":[0.33333334]},\
             {\"_id\":\"b\",\"_score\":1.0,\"other\":0.0001},\
             {\"_id\":\"c\",\"_score\":1.0E-4},\
             {\"_id\":\"d\",\"_score\":null}]},\"took\":3}"
        );
    }

    #[test]
    fn a_non_float_f64_score_keeps_the_serde_rendering() {
        let text = String::from_utf8(to_es_json_vec(&json!({"_score": 0.1_f64})).expect("ok"))
            .expect("utf8");
        assert_eq!(text, "{\"_score\":0.1}");
    }

    /// L2b guard: every ES key order in the responses (hits, `_source`,
    /// composite keys, cat rows) relies on serde_json `preserve_order`
    /// (workspace `Cargo.toml`). Disabling it would sort keys again.
    #[test]
    fn serde_json_preserve_order_is_enabled() {
        let parsed: serde_json::Value =
            serde_json::from_str(r#"{"_index":"i","_id":"b","AGE":1,"NOM":"x"}"#).expect("json");
        assert_eq!(
            serde_json::to_string(&parsed).expect("serializes"),
            r#"{"_index":"i","_id":"b","AGE":1,"NOM":"x"}"#
        );
        // Measured cost of the feature: `Value` is an `IndexMap` node.
        assert_eq!(std::mem::size_of::<serde_json::Value>(), 72);
    }
}
