//! Character filters: Elasticsearch `pattern_replace` (Lucene
//! `PatternReplaceCharFilter`) with Lucene-style offset correction.
//!
//! ES compiles `pattern` with `java.util.regex.Pattern` and applies
//! `Matcher.replaceAll(replacement)` over the whole input. Surch compiles
//! with the `regex` crate after translating the Java syntax that differs:
//!
//! | Java (default flags)          | Rust `regex`                  | Translation                 |
//! |-------------------------------|-------------------------------|-----------------------------|
//! | `\d` = `[0-9]` (ASCII)        | `\d` = `\p{Nd}` (Unicode)     | `\d` -> `[0-9]`             |
//! | `\w` = `[a-zA-Z_0-9]`         | `\w` Unicode                  | `\w` -> `[0-9A-Za-z_]`      |
//! | `\s` = `[ \t\n\x0B\f\r]`      | `\s` Unicode                  | `\s` -> ASCII class         |
//! | `\D` `\W` `\S`                | Unicode                       | negated ASCII classes       |
//! | `\p{Alpha}` etc. (POSIX ASCII)| Unicode properties            | ASCII classes (`[[:alpha:]]`) |
//! | `\p{IsLatin}`, `\p{IsL}`      | `\p{Latin}`, `\p{L}`          | `Is` prefix dropped         |
//! | `\Q...\E` literal quoting     | not supported                 | escaped literal             |
//! | `$1` / `${name}` in replacement | `${1}` / `${name}`          | rewritten, `\x` -> literal  |
//!
//! With the `UNICODE_CHARACTER_CLASS` flag the `\d \w \s` classes are left
//! Unicode, as in Java. Constructs the `regex` crate does not support
//! (look-around, back-references, possessive quantifiers, atomic groups,
//! `\p{InBlock}`, `\p{javaXxx}`) fail with an [`AnalysisError`]; they are not
//! silently approximated. `\b` keeps the `regex` crate's Unicode semantics
//! (Java's default `\b` is close to it; not used by the deces patterns).
//!
//! The deces `alphanum` pattern `[^\p{L}\d]` translates to
//! `[^\p{L}[0-9]]`: anything that is neither a Unicode letter nor an ASCII
//! digit becomes a space. Combining marks (`\p{M}`) and non-ASCII digits are
//! therefore replaced too, exactly as in Java.

use std::fmt;

use regex::{Captures, Regex};

/// Error raised while building an analysis component.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnalysisError {
    pub message: String,
}

impl AnalysisError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for AnalysisError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AnalysisError {}

/// Offset correction map of one char filter (Lucene `BaseCharFilter`):
/// sorted output offsets with the cumulative difference to add to reach
/// the input offset.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OffsetCorrection {
    offsets: Vec<usize>,
    diffs: Vec<isize>,
}

impl OffsetCorrection {
    fn add(&mut self, offset: usize, cumulative: isize) {
        if self.offsets.last() == Some(&offset) {
            if let Some(last) = self.diffs.last_mut() {
                *last = cumulative;
            }
            return;
        }
        self.offsets.push(offset);
        self.diffs.push(cumulative);
    }

    /// Maps an offset of the filtered text back to the input text.
    pub fn correct(&self, offset: usize) -> usize {
        let index = self.offsets.partition_point(|&recorded| recorded <= offset);
        if index == 0 {
            return offset;
        }
        let corrected = offset as isize + self.diffs[index - 1];
        corrected.max(0) as usize
    }
}

/// Java `Pattern` flags accepted by ES `pattern_replace` / `pattern`
/// components (`flags: "CASE_INSENSITIVE|COMMENTS"`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JavaRegexFlags {
    pub case_insensitive: bool,
    pub multiline: bool,
    pub dotall: bool,
    pub comments: bool,
    pub unicode_character_class: bool,
}

impl JavaRegexFlags {
    pub fn parse(flags: &str) -> Result<Self, AnalysisError> {
        let mut parsed = Self::default();
        for flag in flags
            .split('|')
            .map(str::trim)
            .filter(|flag| !flag.is_empty())
        {
            match flag {
                "CASE_INSENSITIVE" => parsed.case_insensitive = true,
                "MULTILINE" => parsed.multiline = true,
                "DOTALL" => parsed.dotall = true,
                "COMMENTS" => parsed.comments = true,
                "UNICODE_CASE" => {}
                "UNICODE_CHARACTER_CLASS" => parsed.unicode_character_class = true,
                other => {
                    return Err(AnalysisError::new(format!(
                        "unsupported regex flag [{other}]"
                    )))
                }
            }
        }
        Ok(parsed)
    }
}

/// Translates a Java regex into the `regex` crate syntax (see module docs).
pub fn translate_java_regex(pattern: &str, flags: JavaRegexFlags) -> Result<String, AnalysisError> {
    let mut out = String::with_capacity(pattern.len() + 16);
    let mut inline = String::new();
    if flags.case_insensitive {
        inline.push('i');
    }
    if flags.multiline {
        inline.push('m');
    }
    if flags.dotall {
        inline.push('s');
    }
    if flags.comments {
        inline.push('x');
    }
    if !inline.is_empty() {
        out.push_str("(?");
        out.push_str(&inline);
        out.push(')');
    }

    let chars: Vec<char> = pattern.chars().collect();
    let mut index = 0;
    while index < chars.len() {
        let character = chars[index];
        if character == '(' && chars.get(index + 1) == Some(&'?') {
            let next = chars.get(index + 2).copied();
            if matches!(next, Some('=') | Some('!') | Some('>'))
                || (next == Some('<') && matches!(chars.get(index + 3), Some('=') | Some('!')))
            {
                return Err(AnalysisError::new(format!(
                    "regex [{pattern}] uses look-around or atomic groups, unsupported"
                )));
            }
        }
        if matches!(character, '*' | '+' | '?' | '}') && chars.get(index + 1) == Some(&'+') {
            return Err(AnalysisError::new(format!(
                "regex [{pattern}] uses a possessive quantifier, unsupported"
            )));
        }
        if character != '\\' {
            out.push(character);
            index += 1;
            continue;
        }
        let Some(&escaped) = chars.get(index + 1) else {
            return Err(AnalysisError::new(format!(
                "regex [{pattern}] ends with a dangling backslash"
            )));
        };
        index += 2;
        match escaped {
            'd' | 'D' | 'w' | 'W' | 's' | 'S' if !flags.unicode_character_class => {
                out.push_str(match escaped {
                    'd' => "[0-9]",
                    'D' => "[^0-9]",
                    'w' => "[0-9A-Za-z_]",
                    'W' => "[^0-9A-Za-z_]",
                    's' => "[\\t\\n\\x0B\\x0C\\r ]",
                    _ => "[^\\t\\n\\x0B\\x0C\\r ]",
                });
            }
            'p' | 'P' => {
                let negated = escaped == 'P';
                if chars.get(index) == Some(&'{') {
                    let close = chars[index..]
                        .iter()
                        .position(|&c| c == '}')
                        .map(|offset| index + offset)
                        .ok_or_else(|| {
                            AnalysisError::new(format!("regex [{pattern}] has an unclosed \\p{{"))
                        })?;
                    let name: String = chars[index + 1..close].iter().collect();
                    index = close + 1;
                    out.push_str(&translate_property(&name, negated, pattern)?);
                } else if let Some(&single) = chars.get(index) {
                    index += 1;
                    out.push('\\');
                    out.push(escaped);
                    out.push(single);
                }
            }
            'Q' => {
                let rest: String = chars[index..].iter().collect();
                let (literal, consumed) = match rest.find("\\E") {
                    Some(end) => (&rest[..end], end + 2),
                    None => (rest.as_str(), rest.len()),
                };
                out.push_str(&regex::escape(literal));
                index += rest[..consumed].chars().count();
            }
            '1'..='9' => {
                return Err(AnalysisError::new(format!(
                    "regex [{pattern}] uses a back-reference, unsupported"
                )))
            }
            'h' | 'H' | 'v' | 'V' | 'R' | 'X' | 'G' | 'Z' | 'k' => {
                return Err(AnalysisError::new(format!(
                    "regex [{pattern}] uses the Java-only escape \\{escaped}, unsupported"
                )))
            }
            other => {
                out.push('\\');
                out.push(other);
            }
        }
    }
    Ok(out)
}

fn translate_property(name: &str, negated: bool, pattern: &str) -> Result<String, AnalysisError> {
    let posix = match name {
        "Lower" => Some("[:lower:]"),
        "Upper" => Some("[:upper:]"),
        "ASCII" => Some("[:ascii:]"),
        "Alpha" => Some("[:alpha:]"),
        "Digit" => Some("[:digit:]"),
        "Alnum" => Some("[:alnum:]"),
        "Punct" => Some("[:punct:]"),
        "Graph" => Some("[:graph:]"),
        "Print" => Some("[:print:]"),
        "Blank" => Some("[:blank:]"),
        "Cntrl" => Some("[:cntrl:]"),
        "XDigit" => Some("[:xdigit:]"),
        "Space" => Some("[:space:]"),
        _ => None,
    };
    if let Some(class) = posix {
        let caret = if negated { "^" } else { "" };
        return Ok(format!("[{caret}{class}]"));
    }
    if name.starts_with("In") || name.starts_with("java") {
        return Err(AnalysisError::new(format!(
            "regex [{pattern}] uses the Java-only property \\p{{{name}}}, unsupported"
        )));
    }
    let name = name.strip_prefix("Is").unwrap_or(name);
    let letter = if negated { 'P' } else { 'p' };
    Ok(format!("\\{letter}{{{name}}}"))
}

/// Translates a Java `Matcher.appendReplacement` replacement string into the
/// `regex` crate expansion syntax. `group_count` is the number of capture
/// groups of the pattern (Java consumes `$` digits greedily while the group
/// exists).
pub fn translate_java_replacement(
    replacement: &str,
    group_count: usize,
) -> Result<String, AnalysisError> {
    let chars: Vec<char> = replacement.chars().collect();
    let mut out = String::with_capacity(replacement.len());
    let mut index = 0;
    while index < chars.len() {
        match chars[index] {
            '\\' => {
                let literal = chars.get(index + 1).ok_or_else(|| {
                    AnalysisError::new("character to be escaped is missing in replacement")
                })?;
                if *literal == '$' {
                    out.push_str("$$");
                } else {
                    out.push(*literal);
                }
                index += 2;
            }
            '$' => {
                index += 1;
                match chars.get(index) {
                    Some('{') => {
                        let close = chars[index..]
                            .iter()
                            .position(|&c| c == '}')
                            .map(|offset| index + offset)
                            .ok_or_else(|| {
                                AnalysisError::new("named capturing group is missing trailing '}'")
                            })?;
                        let name: String = chars[index + 1..close].iter().collect();
                        out.push_str("${");
                        out.push_str(&name);
                        out.push('}');
                        index = close + 1;
                    }
                    Some(digit) if digit.is_ascii_digit() => {
                        let mut group = digit.to_digit(10).unwrap_or(0) as usize;
                        index += 1;
                        while let Some(next) = chars.get(index).and_then(|c| c.to_digit(10)) {
                            let candidate = group * 10 + next as usize;
                            if candidate > group_count {
                                break;
                            }
                            group = candidate;
                            index += 1;
                        }
                        if group > group_count {
                            return Err(AnalysisError::new(format!("no group {group}")));
                        }
                        out.push_str(&format!("${{{group}}}"));
                    }
                    _ => return Err(AnalysisError::new("illegal group reference in replacement")),
                }
            }
            other => {
                out.push(other);
                index += 1;
            }
        }
    }
    Ok(out)
}

/// ES `pattern_replace` char filter.
#[derive(Clone, Debug)]
pub struct PatternReplaceCharFilter {
    regex: Regex,
    replacement: String,
    java_pattern: String,
}

impl PatternReplaceCharFilter {
    pub fn new(pattern: &str, replacement: &str, flags: &str) -> Result<Self, AnalysisError> {
        let flags = JavaRegexFlags::parse(flags)?;
        let translated = translate_java_regex(pattern, flags)?;
        let regex = Regex::new(&translated).map_err(|error| {
            AnalysisError::new(format!(
                "invalid pattern_replace pattern [{pattern}]: {error}"
            ))
        })?;
        let group_count = regex.captures_len().saturating_sub(1);
        let replacement = translate_java_replacement(replacement, group_count)?;
        Ok(Self {
            regex,
            replacement,
            java_pattern: pattern.to_owned(),
        })
    }

    /// The pattern as declared (Java syntax).
    pub fn pattern(&self) -> &str {
        &self.java_pattern
    }

    /// Replaces every match; returns the filtered text and the offset map
    /// back to `input` (same algorithm as Lucene `PatternReplaceCharFilter`,
    /// in byte units).
    pub fn filter(&self, input: &str) -> (String, OffsetCorrection) {
        let mut output = String::with_capacity(input.len());
        let mut correction = OffsetCorrection::default();
        let mut cumulative: isize = 0;
        let mut last_end = 0_usize;
        for captures in self.regex.captures_iter(input) {
            let whole = captures.get(0).expect("group 0 always exists");
            output.push_str(&input[last_end..whole.start()]);
            let before = output.len();
            expand(&captures, &self.replacement, &mut output);
            let replacement_len = output.len() - before;
            let group_len = whole.end() - whole.start();
            if replacement_len < group_len {
                cumulative += (group_len - replacement_len) as isize;
                correction.add(before + replacement_len, cumulative);
            } else {
                for extra in group_len..replacement_len {
                    cumulative -= 1;
                    correction.add(before + extra, cumulative);
                }
            }
            last_end = whole.end();
        }
        output.push_str(&input[last_end..]);
        (output, correction)
    }
}

fn expand(captures: &Captures<'_>, replacement: &str, output: &mut String) {
    captures.expand(replacement, output);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deces_alphanum_pattern_translates_digits_to_ascii() {
        assert_eq!(
            translate_java_regex(r"[^\p{L}\d]", JavaRegexFlags::default()).unwrap(),
            r"[^\p{L}[0-9]]"
        );
    }

    #[test]
    fn alphanum_replaces_everything_but_letters_and_ascii_digits() {
        let filter = PatternReplaceCharFilter::new(r"[^\p{L}\d]", " ", "").unwrap();
        let (text, _) = filter.filter("Saint-Étienne d'Arc ٣2");
        assert_eq!(text, "Saint Étienne d Arc  2");
    }

    #[test]
    fn offsets_shrink_back_to_the_input() {
        // `’` is 3 bytes replaced by 1: offsets after it shift by 2.
        let filter = PatternReplaceCharFilter::new(r"[^\p{L}\d]", " ", "").unwrap();
        let (text, correction) = filter.filter("d’Arc");
        assert_eq!(text, "d Arc");
        assert_eq!(correction.correct(0), 0);
        assert_eq!(correction.correct(2), 4);
        assert_eq!(correction.correct(5), 7);
    }

    #[test]
    fn offsets_grow_point_to_the_end_of_the_group() {
        let filter = PatternReplaceCharFilter::new("a", "xyz", "").unwrap();
        let (text, correction) = filter.filter("bab");
        assert_eq!(text, "bxyzb");
        assert_eq!(correction.correct(4), 2);
        assert_eq!(correction.correct(5), 3);
    }

    #[test]
    fn java_group_references_are_rewritten() {
        let filter = PatternReplaceCharFilter::new(r"(\d+)-(\d+)", "$2_$1", "").unwrap();
        assert_eq!(filter.filter("12-34").0, "34_12");
        let filter = PatternReplaceCharFilter::new(r"(\d)", r"\$$1", "").unwrap();
        assert_eq!(filter.filter("a1").0, "a$1");
    }

    #[test]
    fn unsupported_java_constructs_fail_explicitly() {
        assert!(PatternReplaceCharFilter::new(r"a(?=b)", "", "").is_err());
        assert!(PatternReplaceCharFilter::new(r"(a)\1", "", "").is_err());
        assert!(PatternReplaceCharFilter::new(r"a*+", "", "").is_err());
        assert!(PatternReplaceCharFilter::new(r"\p{InGreek}", "", "").is_err());
        assert!(PatternReplaceCharFilter::new("a", "", "CANON_EQ").is_err());
    }

    #[test]
    fn posix_and_is_properties_follow_java() {
        let filter = PatternReplaceCharFilter::new(r"\p{Alpha}", "_", "").unwrap();
        assert_eq!(filter.filter("aé").0, "_é");
        let filter = PatternReplaceCharFilter::new(r"\p{IsLatin}", "_", "").unwrap();
        assert_eq!(filter.filter("aé1").0, "__1");
    }
}
