//! `java.lang.Float.toString(float)` as ES 8.6.1 prints `_score` / `max_score`.
//!
//! Chain (sources pinned to the ES 8.6.1 tag):
//! * `SearchHit.toInnerXContent` writes `builder.field("_score", score)`
//!   (SearchHit.java l.664-667, `null` when the score is NaN) and
//!   `SearchHits.toXContent` writes `max_score` the same way (SearchHits.java
//!   l.185-188);
//! * `XContentBuilder.field(String, float)` -> `JsonXContentGenerator
//!   .writeNumberField(String, float)` (l.248-254) -> Jackson
//!   `JsonGenerator.writeNumberField(String, float)`;
//! * Jackson 2.13.3 (`version.properties`: `jackson = 2.13.3`)
//!   `UTF8JsonGenerator.writeNumber(float f)` (l.1035-1046) writes
//!   `String.valueOf(f)`, i.e. `Float.toString(f)` — no custom float writer
//!   in 2.13 (the `USE_FAST_DOUBLE_WRITER` feature only appears in 2.14);
//! * ES 8.6.1 bundles JDK 19.0.1 (`bundled_jdk = 19.0.1+10`), whose
//!   `Float.toString` is Raffaello Giulietti's shortest-decimal algorithm
//!   (JDK-4511638, JDK 19).
//!
//! JDK 19 `Float.toString` specification: let R be the decimals that round to
//! `v` (IEEE round-half-even), m the minimal length over R; T = the decimals
//! of R of length m when m >= 2, of length 1 or 2 when m == 1; the selected
//! decimal is the one of T closest to `v`, the one with the even significand
//! on a tie. Formatting: for `10^-3 <= |d| < 10^7`, the plain form with at
//! least one fractional digit (`1.0`, `0.001`, `1234567.0`); otherwise the
//! computerized scientific form `d.ddd` + `E` + exponent, with at least one
//! fractional digit (`1.0E7`, `1.0E-4`, `1.4E-45`).
//!
//! Rust's `{:e}` formatting yields the shortest round-tripping digits, the
//! closest one when several share that length (flt2dec shortest mode); the
//! only extra JDK rule is the length-2 widening when m == 1, handled here.

/// `Float.toString(value)` of JDK 19+.
pub fn java_float_to_string(value: f32) -> String {
    if value.is_nan() {
        return "NaN".to_owned();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.to_owned();
    }
    if value == 0.0 {
        return if value.is_sign_negative() {
            "-0.0"
        } else {
            "0.0"
        }
        .to_owned();
    }
    let negative = value < 0.0;
    let magnitude = value.abs();
    let (digits, exponent) = java_decimal(magnitude);
    let body = format_java(&digits, exponent);
    if negative {
        format!("-{body}")
    } else {
        body
    }
}

/// Significant digits (no trailing zero) and decimal exponent `e` of the
/// decimal JDK 19 selects for a positive finite `value`
/// (`value ~= d1.d2d3... x 10^e`).
fn java_decimal(value: f32) -> (String, i32) {
    let (digits, exponent) = shortest(value);
    if digits.len() != 1 {
        return (digits, exponent);
    }
    // m == 1: T also holds the length-2 decimals that round to `value`;
    // keep the closest to `value` (even significand on a tie).
    let exact = f64::from(value);
    let lead = i64::from(digits.as_bytes()[0] - b'0');
    let mut best: (String, i32, f64, i64) = (
        digits.clone(),
        exponent,
        (decimal_value(lead, exponent) - exact).abs(),
        lead,
    );
    // The closest length-2 decimal at a given scale is the rounded quotient
    // (or its neighbour on a tie); `value` may sit just below `10^exponent`,
    // so both scales `exponent - 1` and `exponent - 2` are probed.
    for scale in [exponent - 2, exponent - 1] {
        let rounded = (exact / format!("1e{scale}").parse::<f64>().unwrap_or(f64::NAN)).round();
        if !rounded.is_finite() {
            continue;
        }
        let rounded = rounded as i64;
        for significand in (rounded - 1).max(10)..=(rounded + 1).min(99) {
            let text = format!("{significand}e{scale}");
            if text.parse::<f32>().ok() != Some(value) {
                continue;
            }
            let distance = (decimal_value(significand, scale) - exact).abs();
            let better = distance < best.2
                || (distance == best.2 && significand % 2 == 0 && best.3 % 2 != 0);
            if better {
                let (trimmed, trimmed_exponent) = normalize(significand, scale);
                best = (trimmed, trimmed_exponent, distance, significand);
            }
        }
    }
    (best.0, best.1)
}

/// `significand x 10^scale` as the nearest `f64` (correctly rounded parse).
fn decimal_value(significand: i64, scale: i32) -> f64 {
    format!("{significand}e{scale}")
        .parse::<f64>()
        .unwrap_or(f64::NAN)
}

/// Digits without trailing zeros and the scientific exponent of
/// `significand x 10^scale`.
fn normalize(significand: i64, scale: i32) -> (String, i32) {
    let text = significand.to_string();
    let exponent = scale + text.len() as i32 - 1;
    let trimmed = text.trim_end_matches('0');
    (
        if trimmed.is_empty() { "0" } else { trimmed }.to_owned(),
        exponent,
    )
}

/// Rust shortest round-tripping digits of a positive finite `f32`.
fn shortest(value: f32) -> (String, i32) {
    let text = format!("{value:e}");
    let (mantissa, exponent) = text
        .split_once('e')
        .expect("`{:e}` always prints an exponent");
    let exponent: i32 = exponent.parse().expect("integer exponent");
    let digits: String = mantissa.chars().filter(char::is_ascii_digit).collect();
    let trimmed = digits.trim_end_matches('0');
    (
        if trimmed.is_empty() { "0" } else { trimmed }.to_owned(),
        exponent,
    )
}

/// JDK `Float.toString` layout of `d1.d2... x 10^exponent`.
fn format_java(digits: &str, exponent: i32) -> String {
    if (-3..7).contains(&exponent) {
        if exponent >= 0 {
            let int_len = exponent as usize + 1;
            if digits.len() <= int_len {
                format!("{digits}{}.0", "0".repeat(int_len - digits.len()))
            } else {
                format!("{}.{}", &digits[..int_len], &digits[int_len..])
            }
        } else {
            format!("0.{}{digits}", "0".repeat((-exponent - 1) as usize))
        }
    } else {
        let (first, rest) = digits.split_at(1);
        let rest = if rest.is_empty() { "0" } else { rest };
        format!("{first}.{rest}E{exponent}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Expected strings are what `Float.toString` returns on JDK 19+
    /// (`lucene-source-derived`: from the JDK specification and the
    /// published Giulietti test vectors, not from an ES run).
    #[test]
    fn matches_jdk19_float_to_string() {
        let cases: [(f32, &str); 24] = [
            (1.0, "1.0"),
            (0.5, "0.5"),
            (2.2, "2.2"),
            (10.0, "10.0"),
            (100.0, "100.0"),
            (1234567.0, "1234567.0"),
            (1.0e7, "1.0E7"),
            (12345678.0, "1.2345678E7"),
            (0.001, "0.001"),
            (0.0009999999, "9.999999E-4"),
            (1.0e-4, "1.0E-4"),
            (0.1, "0.1"),
            (0.3, "0.3"),
            (1.0 / 3.0, "0.33333334"),
            (6.69709, "6.69709"),
            (16.179722, "16.179722"),
            (3.4028235e38, "3.4028235E38"),
            (f32::MIN_POSITIVE, "1.1754944E-38"),
            (f32::from_bits(1), "1.4E-45"),
            (f32::from_bits(2), "2.8E-45"),
            (f32::from_bits(3), "4.2E-45"),
            (-1.5, "-1.5"),
            (0.0, "0.0"),
            (-0.0, "-0.0"),
        ];
        for (value, expected) in cases {
            assert_eq!(java_float_to_string(value), expected, "{value:e}");
        }
    }

    /// Every printed decimal round-trips, and no other decimal of the same
    /// length is strictly closer (JDK selection rule).
    #[test]
    fn printed_decimal_round_trips_and_is_closest() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..200_000 {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let value = f32::from_bits((state >> 32) as u32 & 0x7FFF_FFFF);
            if !value.is_finite() || value == 0.0 {
                continue;
            }
            let text = java_float_to_string(value);
            assert_eq!(text.parse::<f32>().ok(), Some(value), "{text}");
            let (digits, exponent) = java_decimal(value);
            let significand: i64 = digits.parse().expect("digits");
            let scale = exponent - (digits.len() as i32 - 1);
            let exact = f64::from(value);
            let distance = (decimal_value(significand, scale) - exact).abs();
            for neighbour in [significand - 1, significand + 1] {
                if neighbour <= 0 || neighbour.to_string().len() != digits.len() {
                    continue;
                }
                let text = format!("{neighbour}e{scale}");
                if text.parse::<f32>().ok() == Some(value) {
                    assert!(
                        (decimal_value(neighbour, scale) - exact).abs() >= distance,
                        "{neighbour}e{scale} closer than {digits}e{scale} to {value:e}"
                    );
                }
            }
        }
    }
}
