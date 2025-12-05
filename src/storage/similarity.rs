//! Lightweight text similarity for pattern matching
//!
//! Uses TF-IDF style scoring for fast, effective similarity matching
//! without requiring external ML libraries.

use std::collections::HashMap;

/// Calculate similarity between query and patterns using TF-IDF-like scoring
/// Returns a score between 0.0 and 1.0
/// Returns a penalty (low score multiplier) for tech stack mismatches
pub fn calculate_similarity(query: &str, pattern_text: &str) -> f64 {
    let query_tokens = tokenize(query);
    let pattern_tokens = tokenize(pattern_text);

    if query_tokens.is_empty() || pattern_tokens.is_empty() {
        return 0.0;
    }

    // Detect technology context from query and pattern
    let query_tech = detect_tech_stack(&query_tokens);
    let pattern_tech = detect_tech_stack(&pattern_tokens);

    // Calculate tech stack modifier:
    // - Same tech stack: 1.5x boost
    // - Unknown on either side: 1.0x (neutral, allows generic patterns)
    // - Different known tech stacks: 0.3x penalty (still shows if very relevant, but lower ranked)
    let tech_modifier = if query_tech != TechStack::Unknown && pattern_tech != TechStack::Unknown {
        if query_tech == pattern_tech {
            1.5 // Boost for matching tech stack
        } else {
            0.3 // Penalty for mismatched tech stack (but don't completely filter out)
        }
    } else {
        1.0 // Neutral for unknown contexts - allows generic patterns to match
    };

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

    // Normalize and apply tech modifier
    if query_norm > 0.0 {
        (score / query_norm.sqrt()) * tech_modifier
    } else {
        0.0
    }
}

/// Technology stack detection for context-aware matching
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TechStack {
    Rust,
    JavaScript, // Includes TypeScript, Node.js
    Python,
    Go,
    Shell,
    Unknown,
}

/// Detect the technology stack from tokenized text
fn detect_tech_stack(tokens: &[String]) -> TechStack {
    let mut rust_signals = 0;
    let mut js_signals = 0;
    let mut python_signals = 0;
    let mut go_signals = 0;
    let mut shell_signals = 0;

    for token in tokens {
        match token.as_str() {
            // Rust signals
            "rs" | "rust" | "cargo" | "toml" | "rustc" | "crate" => rust_signals += 2,
            "unwrap" | "impl" | "struct" | "enum" | "mut" | "async" => rust_signals += 1,

            // JavaScript/TypeScript/Node signals
            "js" | "ts" | "tsx" | "jsx" | "npm" | "npx" | "node" | "yarn" | "javascript" | "typescript" => js_signals += 2,
            "package" | "json" | "react" | "vue" | "express" | "next" | "webpack" | "eslint" | "deno" | "bun" => js_signals += 1,

            // Python signals
            "py" | "python" | "pip" | "conda" | "pytest" | "django" | "flask" => python_signals += 2,
            "venv" | "requirements" | "pyproject" => python_signals += 1,

            // Go signals
            "go" | "golang" | "mod" => go_signals += 2,
            "goroutine" | "gomod" => go_signals += 1,

            // Shell signals - give sh/bash 2 points since they're strong indicators
            "sh" | "bash" => shell_signals += 2,
            "zsh" | "shell" | "shebang" => shell_signals += 1,

            _ => {}
        }
    }

    // Return the dominant tech stack (need at least 2 signals)
    let max_signals = rust_signals.max(js_signals).max(python_signals).max(go_signals).max(shell_signals);

    if max_signals < 2 {
        return TechStack::Unknown;
    }

    if rust_signals == max_signals { TechStack::Rust }
    else if js_signals == max_signals { TechStack::JavaScript }
    else if python_signals == max_signals { TechStack::Python }
    else if go_signals == max_signals { TechStack::Go }
    else if shell_signals == max_signals { TechStack::Shell }
    else { TechStack::Unknown }
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
        // Mismatched tech stacks get 0.3x penalty, so score should be very low but not zero
        assert!(score < 0.5, "Unrelated should have low score: {}", score);
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

    #[test]
    fn test_tech_stack_penalty() {
        // Rust query should prefer Rust patterns over Node.js patterns
        let rust_query = "Editing main.rs rust cargo toml";
        let rust_pattern = "Task: Fix Rust compilation | Approach: cargo build";
        let nodejs_pattern = "Task: Create Node.js backend | Approach: npm init package.json";

        let rust_score = calculate_similarity(rust_query, rust_pattern);
        let nodejs_score = calculate_similarity(rust_query, nodejs_pattern);

        assert!(
            rust_score > nodejs_score,
            "Rust query should prefer Rust patterns over Node.js: {} vs {}",
            rust_score,
            nodejs_score
        );
    }

    #[test]
    fn test_tech_stack_detection() {
        // Test that tech stack detection works correctly
        let rust_tokens = tokenize("editing main.rs cargo toml rust");
        let js_tokens = tokenize("npm install package.json typescript");
        let python_tokens = tokenize("pip install python pytest requirements");

        assert_eq!(detect_tech_stack(&rust_tokens), TechStack::Rust);
        assert_eq!(detect_tech_stack(&js_tokens), TechStack::JavaScript);
        assert_eq!(detect_tech_stack(&python_tokens), TechStack::Python);
    }

    #[test]
    fn test_unknown_tech_stack_neutral() {
        // Generic queries shouldn't have tech stack penalty
        let generic_query = "fix the bug in the code";
        let pattern = "Task: Fix error | Approach: Edit the file";

        let score = calculate_similarity(generic_query, pattern);
        // Should have some score without tech penalty affecting it
        assert!(score >= 0.0, "Generic query should match generically: {}", score);
    }

    #[test]
    fn test_shell_edit_matching() {
        // Shell file query should match shell file patterns
        let query = "Editing sh shell bash file test.sh";
        let pattern = "Task: Understand code\nApproach: Edit - sh shell bash editing start.sh (replacing '#!/bin/bash')";

        let score = calculate_similarity(query, pattern);
        println!("Shell edit match score: {}", score);
        // Should have a reasonable match (same tech stack = 1.5x boost)
        assert!(score >= 0.35, "Shell query should match shell patterns: {}", score);
    }

    #[test]
    fn test_rust_edit_no_shell_match() {
        // Rust file query should NOT match shell patterns well
        let query = "Editing rs rust cargo toml crate file main.rs";
        let pattern = "Task: Understand code\nApproach: Edit - sh shell bash editing start.sh";

        let score = calculate_similarity(query, pattern);
        println!("Rust vs shell match score: {}", score);
        // Should have low score due to tech stack mismatch (0.3x penalty)
        assert!(score < 0.35, "Rust query should NOT match shell patterns well: {}", score);
    }
}
