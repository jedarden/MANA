//! Lightweight text similarity for pattern matching
//!
//! Uses TF-IDF style scoring for fast, effective similarity matching
//! without requiring external ML libraries.

use std::collections::HashMap;

/// Calculate similarity between query and patterns using TF-IDF-like scoring
/// Returns a score between 0.0 and 1.0
pub fn calculate_similarity(query: &str, pattern_text: &str) -> f64 {
    let query_tokens = tokenize(query);
    let pattern_tokens = tokenize(pattern_text);

    if query_tokens.is_empty() || pattern_tokens.is_empty() {
        return 0.0;
    }

    // Build term frequency maps
    let query_tf = build_tf(&query_tokens);
    let pattern_tf = build_tf(&pattern_tokens);

    // Calculate weighted overlap
    let mut score = 0.0;
    let mut query_norm = 0.0;

    for (term, &count) in &query_tf {
        let weight = term_weight(term);
        query_norm += (count as f64 * weight).powi(2);

        if let Some(&pattern_count) = pattern_tf.get(term) {
            // Boost for exact matches
            score += count as f64 * pattern_count as f64 * weight * weight;
        }
    }

    // Normalize
    if query_norm > 0.0 {
        score / query_norm.sqrt()
    } else {
        0.0
    }
}

/// Tokenize text into meaningful terms with basic stemming
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
        .filter(|s| s.len() >= 2)
        .filter(|s| !is_stopword(s))
        .map(|s| stem_word(s))
        .collect()
}

/// Basic stemming - remove common suffixes
fn stem_word(word: &str) -> String {
    let word = word.to_string();

    // Handle common verb forms
    if word.ends_with("ing") && word.len() > 5 {
        // editing -> edit, running -> run
        let stem = &word[..word.len() - 3];
        // Handle doubled consonants: running -> run (not runn)
        if stem.len() > 2 {
            let chars: Vec<char> = stem.chars().collect();
            if chars.len() >= 2 && chars[chars.len()-1] == chars[chars.len()-2] {
                return stem[..stem.len()-1].to_string();
            }
        }
        return stem.to_string();
    }

    // Handle -tion -> -t (configuration -> configurat)
    if word.ends_with("tion") && word.len() > 6 {
        return word[..word.len() - 3].to_string();
    }

    // Handle -ed suffix
    if word.ends_with("ed") && word.len() > 4 {
        return word[..word.len() - 2].to_string();
    }

    // Handle -ly suffix
    if word.ends_with("ly") && word.len() > 4 {
        return word[..word.len() - 2].to_string();
    }

    // Handle -s suffix (but not words like "class")
    if word.ends_with('s') && word.len() > 3 && !word.ends_with("ss") {
        return word[..word.len() - 1].to_string();
    }

    word
}

/// Build term frequency map
fn build_tf(tokens: &[String]) -> HashMap<&str, usize> {
    let mut tf = HashMap::new();
    for token in tokens {
        *tf.entry(token.as_str()).or_insert(0) += 1;
    }
    tf
}

/// Get importance weight for a term
/// Technical terms and file extensions get higher weights
fn term_weight(term: &str) -> f64 {
    // File extensions - very specific
    if is_file_extension(term) {
        return 3.0;
    }

    // Programming language names
    if matches!(term, "rust" | "python" | "javascript" | "typescript" |
                      "golang" | "java" | "ruby" | "cpp" | "shell") {
        return 2.5;
    }

    // Tool names
    if matches!(term, "bash" | "edit" | "write" | "read" | "grep" |
                      "task" | "glob" | "npm" | "cargo" | "git") {
        return 2.0;
    }

    // Action verbs
    if matches!(term, "fix" | "add" | "create" | "update" | "delete" |
                      "implement" | "refactor" | "test" | "build" | "run") {
        return 1.5;
    }

    // Error-related terms
    if matches!(term, "error" | "failed" | "cannot" | "undefined" |
                      "missing" | "invalid" | "null" | "panic") {
        return 2.0;
    }

    // Default weight based on length (longer = more specific = higher weight)
    if term.len() >= 8 {
        1.5
    } else if term.len() >= 5 {
        1.2
    } else {
        1.0
    }
}

/// Check if term is a file extension
fn is_file_extension(term: &str) -> bool {
    matches!(term, "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "rb" |
             "java" | "cpp" | "c" | "h" | "md" | "json" | "yaml" | "yml" |
             "toml" | "sh" | "html" | "css" | "sql" | "vue" | "svelte")
}

/// Check if term is a common stopword
fn is_stopword(term: &str) -> bool {
    matches!(term, "the" | "a" | "an" | "is" | "are" | "was" | "were" |
             "be" | "been" | "being" | "have" | "has" | "had" | "do" |
             "does" | "did" | "will" | "would" | "could" | "should" |
             "may" | "might" | "must" | "can" | "to" | "of" | "in" |
             "for" | "on" | "with" | "at" | "by" | "from" | "as" |
             "it" | "that" | "this" | "these" | "those" | "and" | "or" |
             "but" | "if" | "then" | "else" | "when" | "where" | "how")
}

/// Rank patterns by similarity to query
pub fn rank_patterns<T: AsRef<str>>(query: &str, patterns: &[(T, T)]) -> Vec<(usize, f64)> {
    let mut scores: Vec<(usize, f64)> = patterns
        .iter()
        .enumerate()
        .map(|(idx, (context, _))| {
            let score = calculate_similarity(query, context.as_ref());
            (idx, score)
        })
        .filter(|(_, score)| *score > 0.0)
        .collect();

    // Sort by score descending
    scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scores
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_similarity_exact() {
        let score = calculate_similarity(
            "fix the type error in main.rs",
            "fix type error main.rs rust"
        );
        assert!(score > 0.5, "Exact match should have high score: {}", score);
    }

    #[test]
    fn test_calculate_similarity_partial() {
        let score = calculate_similarity(
            "edit the config file",
            "editing configuration settings"
        );
        assert!(score > 0.0, "Partial match should have some score: {}", score);
    }

    #[test]
    fn test_calculate_similarity_unrelated() {
        let score = calculate_similarity(
            "fix rust compilation error",
            "python web scraping tutorial"
        );
        assert!(score < 0.3, "Unrelated should have low score: {}", score);
    }

    #[test]
    fn test_file_extension_weight() {
        // Query with .rs should match patterns with rust/rs better
        let rs_score = calculate_similarity(
            "editing main.rs",
            "Task: Fix the type error | Approach: Edit - editing main.rs"
        );
        let py_score = calculate_similarity(
            "editing main.rs",
            "Task: Add function | Approach: Edit - editing main.py"
        );
        assert!(rs_score > py_score, "rs should match rs better: {} vs {}", rs_score, py_score);
    }

    #[test]
    fn test_rank_patterns() {
        let query = "fix typescript compilation error";
        let patterns = vec![
            ("Task: Fix TypeScript error | Approach: Edit main.ts", "ts"),
            ("Task: Run Python tests | Approach: Bash pytest", "py"),
            ("Task: Fix Rust error | Approach: Edit lib.rs", "rs"),
        ];

        let ranked = rank_patterns(query, &patterns);
        assert!(!ranked.is_empty());
        // TypeScript pattern should rank first
        assert_eq!(ranked[0].0, 0, "TypeScript pattern should rank first");
    }
}
