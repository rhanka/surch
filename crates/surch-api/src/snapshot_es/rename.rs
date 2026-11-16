//! `_restore` index selection and renaming, as ES 8.6.1 does it.
//!
//! - selection: `SnapshotUtils.filterIndices` — comma list / array of names
//!   and `*` wildcards, `-pattern` exclusions, `_all` / `*` / empty = every
//!   index of the snapshot; a concrete missing name is
//!   `index_not_found_exception` unless `ignore_unavailable` is set;
//! - renaming: `RestoreService.renameIndex` = Java
//!   `index.replaceAll(rename_pattern, rename_replacement)` when both are
//!   given, with `java.util.regex.Matcher.appendReplacement` replacement
//!   syntax (`$1`, `${name}`, `\$` literal); two indices renamed onto the
//!   same name are refused (`RestoreService.renamedIndices`).
//!
//! The pattern is compiled by the Rust `regex` crate: the Java constructs
//! it lacks (look-around, back-references, possessive quantifiers) are
//! refused with an explicit error instead of being misinterpreted.

use regex::Regex;

/// Error of the selection / rename step, rendered by the REST layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameError {
    /// `index_not_found_exception` — `no such index [name]`.
    IndexNotFound(String),
    /// `snapshot_restore_exception` with this reason.
    Restore(String),
}

/// One piece of a parsed Java replacement string.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Literal(String),
    Group(usize),
    Named(String),
}

/// Parse `replacement` with `Matcher.appendReplacement` rules for a pattern
/// with `group_count` capturing groups (group 0 excluded).
fn parse_replacement(
    replacement: &str,
    group_count: usize,
    has_name: impl Fn(&str) -> bool,
) -> Result<Vec<Piece>, String> {
    let chars: Vec<char> = replacement.chars().collect();
    let mut pieces = Vec::new();
    let mut literal = String::new();
    let mut i = 0;
    while i < chars.len() {
        match chars[i] {
            '\\' => {
                i += 1;
                let Some(&escaped) = chars.get(i) else {
                    return Err("character to be escaped is missing".to_owned());
                };
                literal.push(escaped);
                i += 1;
            }
            '$' => {
                i += 1;
                let Some(&next) = chars.get(i) else {
                    return Err("Illegal group reference: group index is missing".to_owned());
                };
                if !literal.is_empty() {
                    pieces.push(Piece::Literal(std::mem::take(&mut literal)));
                }
                if next == '{' {
                    i += 1;
                    let start = i;
                    while i < chars.len() && chars[i].is_ascii_alphanumeric() {
                        i += 1;
                    }
                    let name: String = chars[start..i].iter().collect();
                    if name.is_empty() {
                        return Err("named capturing group has 0 length name".to_owned());
                    }
                    if chars.get(i) != Some(&'}') {
                        return Err("named capturing group is missing trailing '}'".to_owned());
                    }
                    i += 1;
                    if name.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        return Err(format!(
                            "capturing group name {{{name}}} starts with digit character"
                        ));
                    }
                    if !has_name(&name) {
                        return Err(format!("No group with name {{{name}}}"));
                    }
                    pieces.push(Piece::Named(name));
                } else if let Some(first) = next.to_digit(10) {
                    // Java: the first digit is always taken; following digits
                    // are taken while the number stays a valid group.
                    let mut number = first as usize;
                    if number > group_count {
                        return Err(format!("No group {number}"));
                    }
                    i += 1;
                    while let Some(digit) = chars.get(i).and_then(|c| c.to_digit(10)) {
                        let candidate = number * 10 + digit as usize;
                        if candidate > group_count {
                            break;
                        }
                        number = candidate;
                        i += 1;
                    }
                    pieces.push(Piece::Group(number));
                } else {
                    return Err("Illegal group reference".to_owned());
                }
            }
            other => {
                literal.push(other);
                i += 1;
            }
        }
    }
    if !literal.is_empty() {
        pieces.push(Piece::Literal(literal));
    }
    Ok(pieces)
}

/// Java `input.replaceAll(pattern, replacement)`.
pub fn java_replace_all(input: &str, pattern: &str, replacement: &str) -> Result<String, String> {
    let regex = Regex::new(pattern)
        .map_err(|error| format!("invalid rename_pattern [{pattern}]: {error}"))?;
    let mut matches = regex.captures_iter(input).peekable();
    if matches.peek().is_none() {
        // Java never parses the replacement when nothing matches.
        return Ok(input.to_owned());
    }
    let group_count = regex.captures_len().saturating_sub(1);
    let names: Vec<&str> = regex.capture_names().flatten().collect();
    let pieces = parse_replacement(replacement, group_count, |name| names.contains(&name))
        .map_err(|error| format!("invalid rename_replacement [{replacement}]: {error}"))?;
    let mut out = String::with_capacity(input.len());
    let mut last = 0;
    for captures in matches {
        let whole = captures.get(0).expect("group 0 always participates");
        out.push_str(&input[last..whole.start()]);
        for piece in &pieces {
            match piece {
                Piece::Literal(text) => out.push_str(text),
                Piece::Group(number) => {
                    if let Some(group) = captures.get(*number) {
                        out.push_str(group.as_str());
                    }
                }
                Piece::Named(name) => {
                    if let Some(group) = captures.name(name) {
                        out.push_str(group.as_str());
                    }
                }
            }
        }
        last = whole.end();
    }
    out.push_str(&input[last..]);
    Ok(out)
}

fn wildcard_match(pattern: &str, name: &str) -> bool {
    // `Regex.simpleMatch`: `*` matches any sequence, everything else literal.
    let parts: Vec<&str> = pattern.split('*').collect();
    if parts.len() == 1 {
        return pattern == name;
    }
    let mut rest = name;
    for (position, part) in parts.iter().enumerate() {
        if position == 0 {
            let Some(stripped) = rest.strip_prefix(part) else {
                return false;
            };
            rest = stripped;
        } else if position == parts.len() - 1 {
            return rest.ends_with(part);
        } else {
            match rest.find(part) {
                Some(at) => rest = &rest[at + part.len()..],
                None => return false,
            }
        }
    }
    true
}

/// `SnapshotUtils.filterIndices`: resolve the request `indices` against the
/// snapshot's index list, keeping the snapshot order.
pub fn filter_indices(
    available: &[String],
    requested: &[String],
    ignore_unavailable: bool,
) -> Result<Vec<String>, RenameError> {
    let all = requested.is_empty()
        || (requested.len() == 1 && (requested[0] == "_all" || requested[0] == "*"));
    if all {
        return Ok(available.to_vec());
    }
    let mut selected: Vec<bool> = vec![false; available.len()];
    for (position, expression) in requested.iter().enumerate() {
        let (exclude, pattern) = match expression.strip_prefix('-') {
            // A leading `-` is an exclusion only after a first expression.
            Some(rest) if position > 0 => (true, rest),
            _ => (false, expression.as_str()),
        };
        if pattern.contains('*') {
            for (slot, name) in available.iter().enumerate() {
                if wildcard_match(pattern, name) {
                    selected[slot] = !exclude;
                }
            }
            continue;
        }
        match available.iter().position(|name| name == pattern) {
            Some(slot) => selected[slot] = !exclude,
            None if exclude || ignore_unavailable => {}
            None => return Err(RenameError::IndexNotFound(pattern.to_owned())),
        }
    }
    Ok(available
        .iter()
        .zip(selected)
        .filter(|(_, keep)| *keep)
        .map(|(name, _)| name.clone())
        .collect())
}

/// `RestoreService.renamedIndices`: `(snapshot index, restored name)` pairs.
pub fn renamed_indices(
    selected: &[String],
    rename_pattern: Option<&str>,
    rename_replacement: Option<&str>,
) -> Result<Vec<(String, String)>, RenameError> {
    let mut out: Vec<(String, String)> = Vec::with_capacity(selected.len());
    for index in selected {
        let renamed = match (rename_pattern, rename_replacement) {
            (Some(pattern), Some(replacement)) => {
                java_replace_all(index, pattern, replacement).map_err(RenameError::Restore)?
            }
            _ => index.clone(),
        };
        if let Some((previous, _)) = out.iter().find(|(_, target)| *target == renamed) {
            return Err(RenameError::Restore(format!(
                "indices [{index}] and [{previous}] are renamed into the same index [{renamed}]"
            )));
        }
        out.push((index.clone(), renamed));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn deces_job_literal_rename() {
        // matchID prod Job: rename_pattern `^deces$`, replacement
        // `deces-<safe snapshot>`.
        assert_eq!(
            java_replace_all("deces", "^deces$", "deces-snap-2026-11").unwrap(),
            "deces-snap-2026-11"
        );
        // Anchored: a longer name is left alone.
        assert_eq!(
            java_replace_all("deces-old", "^deces$", "x").unwrap(),
            "deces-old"
        );
    }

    #[test]
    fn group_references_follow_java_rules() {
        assert_eq!(
            java_replace_all("deces", "(.+)", "restored_$1").unwrap(),
            "restored_deces"
        );
        assert_eq!(java_replace_all("ab", "(a)(b)", "$2$1$0").unwrap(), "baab");
        // `$10` with a single group: `$1` then literal `0` (Java greedy rule).
        assert_eq!(java_replace_all("ab", "(a)", "$10").unwrap(), "a0b");
        assert_eq!(
            java_replace_all("index-7", "(?<base>[a-z]+)-(\\d)", "${base}_v$2").unwrap(),
            "index_v7"
        );
        // Escapes.
        assert_eq!(java_replace_all("a", "a", "\\$1\\\\").unwrap(), "$1\\");
        // replaceAll: every match.
        assert_eq!(java_replace_all("a-b-c", "-", "_").unwrap(), "a_b_c");
        // An unmatched optional group appends nothing.
        assert_eq!(java_replace_all("a", "(a)(b)?", "[$2]").unwrap(), "[]");
    }

    #[test]
    fn invalid_replacements_are_refused_like_java() {
        assert!(java_replace_all("a", "(a)", "$2")
            .unwrap_err()
            .contains("No group 2"));
        assert!(java_replace_all("a", "a", "$x")
            .unwrap_err()
            .contains("Illegal group reference"));
        assert!(java_replace_all("a", "a", "x\\")
            .unwrap_err()
            .contains("escaped is missing"));
        assert!(java_replace_all("a", "a", "${nope}")
            .unwrap_err()
            .contains("No group with name"));
        assert!(java_replace_all("a", "(", "x")
            .unwrap_err()
            .contains("invalid rename_pattern"));
        // No match: the replacement is never parsed (Java).
        assert_eq!(java_replace_all("b", "a", "$9").unwrap(), "b");
    }

    #[test]
    fn filter_indices_matches_snapshot_utils() {
        let available = names(&["deces", "deces-old", "places"]);
        assert_eq!(filter_indices(&available, &[], false).unwrap(), available);
        assert_eq!(
            filter_indices(&available, &names(&["_all"]), false).unwrap(),
            available
        );
        assert_eq!(
            filter_indices(&available, &names(&["deces"]), false).unwrap(),
            names(&["deces"])
        );
        assert_eq!(
            filter_indices(&available, &names(&["deces*", "-deces-old"]), false).unwrap(),
            names(&["deces"])
        );
        assert_eq!(
            filter_indices(&available, &names(&["missing"]), false),
            Err(RenameError::IndexNotFound("missing".to_owned()))
        );
        assert_eq!(
            filter_indices(&available, &names(&["missing", "places"]), true).unwrap(),
            names(&["places"])
        );
    }

    #[test]
    fn collisions_are_refused() {
        let selected = names(&["a-1", "a-2"]);
        let error = renamed_indices(&selected, Some("-\\d"), Some("")).unwrap_err();
        assert_eq!(
            error,
            RenameError::Restore(
                "indices [a-2] and [a-1] are renamed into the same index [a]".to_owned()
            )
        );
        assert_eq!(
            renamed_indices(&selected, Some("^a"), None).unwrap(),
            vec![
                ("a-1".to_owned(), "a-1".to_owned()),
                ("a-2".to_owned(), "a-2".to_owned())
            ]
        );
    }
}
