use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum EditDistance {
    DamerauLevenshtein,
    Levenshtein,
}

impl Default for EditDistance {
    fn default() -> Self {
        EditDistance::DamerauLevenshtein
    }
}

pub struct FuzzyAlgorithm;

impl FuzzyAlgorithm {
    pub fn damerau_levenshtein(s1: &str, s2: &str, max_distance: usize) -> usize {
        let s1_chars: Vec<char> = s1.chars().collect();
        let s2_chars: Vec<char> = s2.chars().collect();
        let len1 = s1_chars.len();
        let len2 = s2_chars.len();

        if len1 == 0 {
            return len2.min(max_distance + 1);
        }
        if len2 == 0 {
            return len1.min(max_distance + 1);
        }

        let mut matrix = vec![vec![0usize; len2 + 1]; len1 + 1];

        for i in 0..=len1 {
            matrix[i][0] = i;
        }
        for j in 0..=len2 {
            matrix[0][j] = j;
        }

        for i in 1..=len1 {
            for j in 1..=len2 {
                let cost = if s1_chars[i - 1] == s2_chars[j - 1] {
                    0
                } else {
                    1
                };
                let mut value = std::cmp::min(
                    std::cmp::min(matrix[i - 1][j] + 1, matrix[i][j - 1] + 1),
                    matrix[i - 1][j - 1] + cost,
                );

                if i > 1
                    && j > 1
                    && s1_chars[i - 1] == s2_chars[j - 2]
                    && s1_chars[i - 2] == s2_chars[j - 1]
                {
                    value = value.min(matrix[i - 2][j - 2] + 1);
                }

                matrix[i][j] = value;
            }
        }

        matrix[len1][len2].min(max_distance + 1)
    }

    pub fn levenshtein(s1: &str, s2: &str) -> usize {
        let s1_chars: Vec<char> = s1.chars().collect();
        let s2_chars: Vec<char> = s2.chars().collect();
        let len1 = s1_chars.len();
        let len2 = s2_chars.len();

        if len1 == 0 {
            return len2;
        }
        if len2 == 0 {
            return len1;
        }

        let mut matrix = vec![vec![0usize; len2 + 1]; len1 + 1];

        for i in 0..=len1 {
            matrix[i][0] = i;
        }
        for j in 0..=len2 {
            matrix[0][j] = j;
        }

        for i in 1..=len1 {
            for j in 1..=len2 {
                let cost = if s1_chars[i - 1] == s2_chars[j - 1] {
                    0
                } else {
                    1
                };
                matrix[i][j] = std::cmp::min(
                    std::cmp::min(matrix[i - 1][j] + 1, matrix[i][j - 1] + 1),
                    matrix[i - 1][j - 1] + cost,
                );
            }
        }

        matrix[len1][len2]
    }

    pub fn is_fuzzy_match(s1: &str, s2: &str, max_distance: usize) -> bool {
        Self::damerau_levenshtein(s1, s2, max_distance) <= max_distance
    }

    pub fn find_fuzzy_matches<'a>(
        term: &'a str,
        candidates: &[&'a str],
        max_distance: usize,
    ) -> Vec<(&'a str, usize)> {
        candidates
            .iter()
            .filter_map(|candidate| {
                let distance = Self::damerau_levenshtein(term, candidate, max_distance);
                if distance <= max_distance {
                    Some((*candidate, distance))
                } else {
                    None
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_levenshtein() {
        assert_eq!(FuzzyAlgorithm::levenshtein("kitten", "kitten"), 0);
        assert_eq!(FuzzyAlgorithm::levenshtein("kitten", "sitting"), 3);
        assert_eq!(FuzzyAlgorithm::levenshtein("hello", "hallo"), 1);
    }

    #[test]
    fn test_damerau_levenshtein() {
        assert_eq!(FuzzyAlgorithm::damerau_levenshtein("ab", "ba", 3), 1);
        assert_eq!(FuzzyAlgorithm::damerau_levenshtein("ab", "abc", 2), 1);
        assert_eq!(FuzzyAlgorithm::damerau_levenshtein("ca", "abc", 3), 3);
    }

    #[test]
    fn test_fuzzy_match() {
        assert!(FuzzyAlgorithm::is_fuzzy_match("hello", "hallo", 2));
        assert!(FuzzyAlgorithm::is_fuzzy_match("hello", "hallo", 2));
        assert!(!FuzzyAlgorithm::is_fuzzy_match("hello", "world", 1));
    }

    #[test]
    fn test_find_matches() {
        let candidates = vec!["hello", "hallo", "hullo", "world", "hero"];
        let matches = FuzzyAlgorithm::find_fuzzy_matches("hello", &candidates, 2);
        assert!(matches.len() >= 2);
    }
}
