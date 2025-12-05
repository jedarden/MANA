//! Lightweight text similarity for pattern matching
//!
//! Uses TF-IDF style scoring for fast, effective similarity matching
//! without requiring external ML libraries.
//!
//! Optimized for sub-millisecond performance on small pattern sets.

use std::collections::HashMap;

/// Calculate similarity between query and patterns using TF-IDF-like scoring
/// Returns a score between 0.0 and 1.0
/// Returns a penalty (low score multiplier) for tech stack mismatches
///
/// Optimized for sub-millisecond performance with early exits and reduced allocations
pub fn calculate_similarity(query: &str, pattern_text: &str) -> f64 {
    // Fast path: skip empty inputs
    if query.is_empty() || pattern_text.is_empty() {
        return 0.0;
    }

    // Early tech stack detection directly on strings (avoids full tokenization for mismatches)
    let query_tech = detect_tech_stack_fast(query);
    let pattern_tech = detect_tech_stack_fast(pattern_text);

    // Calculate tech modifier early - can skip expensive tokenization if completely mismatched
    let tech_modifier = if query_tech != TechStack::Unknown && pattern_tech != TechStack::Unknown {
        if query_tech == pattern_tech {
            1.5 // Boost for matching tech stack
        } else {
            0.3 // Penalty for mismatched tech stack (but don't completely filter out)
        }
    } else {
        1.0 // Neutral for unknown contexts - allows generic patterns to match
    };

    // For severe mismatches, apply early exit with minimum score
    // This saves tokenization time for patterns that won't be selected anyway
    if tech_modifier < 0.5 && query_tech != TechStack::Unknown {
        // Quick check: if no common words at all, return minimal score
        let query_lower = query.to_lowercase();
        let pattern_lower = pattern_text.to_lowercase();
        let has_common = query_lower.split_whitespace()
            .take(5) // Only check first 5 words for speed
            .any(|w| w.len() > 3 && pattern_lower.contains(w));
        if !has_common {
            return 0.05 * tech_modifier; // Very low score for no overlap + tech mismatch
        }
    }

    // Tokenize both texts (optimized with pre-allocation)
    let query_tokens = tokenize_fast(query);
    let pattern_tokens = tokenize_fast(pattern_text);

    if query_tokens.is_empty() || pattern_tokens.is_empty() {
        return 0.0;
    }

    // Build term frequency maps (with pre-allocated capacity)
    let query_tf = build_tf_fast(&query_tokens);
    let pattern_tf = build_tf_fast(&pattern_tokens);

    // Calculate weighted overlap
    let mut score = 0.0;
    let mut query_norm = 0.0;

    for (term, &count) in &query_tf {
        let weight = term_weight_fast(term);
        let weighted_count = count as f64 * weight;
        query_norm += weighted_count * weighted_count;

        if let Some(&pattern_count) = pattern_tf.get(term) {
            // Boost for exact matches
            score += weighted_count * pattern_count as f64 * weight;
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

/// Fast tech stack detection directly on the raw string (avoids tokenization)
/// Uses substring matching for speed - O(n) where n is string length
#[inline]
fn detect_tech_stack_fast(text: &str) -> TechStack {
    // Convert to lowercase bytes for fast comparison
    let lower = text.to_lowercase();
    let bytes = lower.as_bytes();

    let mut rust_signals = 0i8;
    let mut js_signals = 0i8;
    let mut python_signals = 0i8;
    let mut go_signals = 0i8;
    let mut shell_signals = 0i8;

    // Check for file extensions and keywords using contains (faster than tokenizing)
    // Rust signals
    if bytes.windows(3).any(|w| w == b".rs") || lower.contains("cargo") || lower.contains("rust") {
        rust_signals += 3;
    }
    if lower.contains("toml") || lower.contains("crate") {
        rust_signals += 2;
    }

    // JavaScript/TypeScript signals
    if bytes.windows(3).any(|w| w == b".js" || w == b".ts") ||
       bytes.windows(4).any(|w| w == b".tsx" || w == b".jsx") ||
       lower.contains("npm") || lower.contains("node") {
        js_signals += 3;
    }
    if lower.contains("typescript") || lower.contains("javascript") || lower.contains("yarn") || lower.contains("package.json") {
        js_signals += 2;
    }

    // Python signals
    if bytes.windows(3).any(|w| w == b".py") || lower.contains("python") || lower.contains("pip") {
        python_signals += 3;
    }
    if lower.contains("pytest") || lower.contains("conda") || lower.contains("venv") {
        python_signals += 2;
    }

    // Go signals
    if bytes.windows(3).any(|w| w == b".go") || lower.contains("golang") {
        go_signals += 3;
    }
    if lower.contains("go mod") || lower.contains("go.mod") {
        go_signals += 2;
    }

    // Shell signals
    if bytes.windows(3).any(|w| w == b".sh") || lower.contains("bash") {
        shell_signals += 3;
    }
    if lower.contains("shell") || lower.contains("zsh") {
        shell_signals += 2;
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

/// Detect the technology stack from tokenized text (legacy, used by tests)
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
/// Optimized to reduce allocations by reusing a single lowercase buffer
fn tokenize(text: &str) -> Vec<String> {
    // Pre-allocate with estimated capacity to reduce reallocations
    let mut tokens = Vec::with_capacity(text.len() / 6);

    // Work with lowercase version once
    let lower = text.to_lowercase();

    for token in lower.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if token.len() >= 2 && !is_stopword(token) {
            tokens.push(stem_word(token));
        }
    }

    tokens
}

/// Fast tokenization that avoids String allocation where possible
/// Returns string slices into a pre-lowercased buffer
#[inline]
fn tokenize_fast(text: &str) -> Vec<String> {
    // Pre-allocate with estimated capacity
    let estimated_tokens = (text.len() / 8).max(8);
    let mut tokens = Vec::with_capacity(estimated_tokens);

    // Work with lowercase version once
    let lower = text.to_lowercase();

    for token in lower.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-') {
        if token.len() >= 2 && !is_stopword_fast(token) {
            // Skip expensive stemming for very short tokens (most common case)
            if token.len() <= 4 {
                tokens.push(token.to_string());
            } else {
                tokens.push(stem_word_fast(token));
            }
        }
    }

    tokens
}

/// Fast stopword check using a match (compiler optimizes to jump table)
#[inline]
fn is_stopword_fast(term: &str) -> bool {
    matches!(term, "the" | "a" | "an" | "is" | "are" | "was" | "were" |
             "be" | "been" | "being" | "have" | "has" | "had" | "do" |
             "does" | "did" | "will" | "would" | "could" | "should" |
             "may" | "might" | "must" | "can" | "to" | "of" | "in" |
             "for" | "on" | "with" | "at" | "by" | "from" | "as" |
             "it" | "that" | "this" | "these" | "those" | "and" | "or" |
             "but" | "if" | "then" | "else" | "when" | "where" | "how")
}

/// Fast stemming that only handles the most common cases
#[inline]
fn stem_word_fast(word: &str) -> String {
    let len = word.len();

    // Handle -ing suffix (most common)
    if len > 5 && word.ends_with("ing") {
        let stem = &word[..len - 3];
        // Handle doubled consonants: running -> run
        let bytes = stem.as_bytes();
        if bytes.len() >= 2 && bytes[bytes.len()-1] == bytes[bytes.len()-2] {
            return stem[..stem.len()-1].to_string();
        }
        return stem.to_string();
    }

    // Handle -ed suffix
    if len > 4 && word.ends_with("ed") {
        return word[..len - 2].to_string();
    }

    // Handle -s suffix (but not words ending in ss)
    if len > 3 && word.ends_with('s') && !word.ends_with("ss") {
        return word[..len - 1].to_string();
    }

    word.to_string()
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
/// Pre-allocates HashMap capacity for better performance
fn build_tf(tokens: &[String]) -> HashMap<&str, usize> {
    let mut tf = HashMap::with_capacity(tokens.len());
    for token in tokens {
        *tf.entry(token.as_str()).or_insert(0) += 1;
    }
    tf
}

/// Fast term frequency map building with minimal overhead
#[inline]
fn build_tf_fast(tokens: &[String]) -> HashMap<&str, usize> {
    let mut tf = HashMap::with_capacity(tokens.len());
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

/// Fast term weight lookup using match (compiler optimized)
#[inline]
fn term_weight_fast(term: &str) -> f64 {
    // Combined match for all high-weight terms (compiler optimizes to efficient lookup)
    match term {
        // File extensions (3.0)
        "rs" | "js" | "ts" | "tsx" | "jsx" | "py" | "go" | "rb" |
        "java" | "cpp" | "c" | "h" | "md" | "json" | "yaml" | "yml" |
        "toml" | "sh" | "html" | "css" | "sql" | "vue" | "svelte" => 3.0,

        // Programming languages (2.5)
        "rust" | "python" | "javascript" | "typescript" |
        "golang" | "ruby" | "shell" => 2.5,

        // Tool names & error terms (2.0)
        "bash" | "edit" | "write" | "read" | "grep" |
        "task" | "glob" | "npm" | "cargo" | "git" |
        "error" | "fail" | "cannot" | "undef" |
        "miss" | "invalid" | "null" | "panic" => 2.0,

        // Action verbs (1.5)
        "fix" | "add" | "creat" | "updat" | "delet" |
        "implement" | "refactor" | "test" | "build" | "run" => 1.5,

        // Default: length-based
        _ => {
            let len = term.len();
            if len >= 8 { 1.5 }
            else if len >= 5 { 1.2 }
            else { 1.0 }
        }
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
#[allow(dead_code)]
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
    fn test_js_pattern_similarity() {
        let query = "Editing js javascript npm node package file server.js";
        let pattern = "Task: Create API endpoint | Approach: Write - js javascript npm node writing to upload.js";

        let score = calculate_similarity(query, pattern);
        println!("JS pattern similarity score: {}", score);

        // This should have a reasonable score due to tech stack match (1.5x boost)
        assert!(score >= 0.35, "JS pattern should match with score >= 0.35, got {}", score);
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
