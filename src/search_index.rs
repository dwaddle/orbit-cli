use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use crate::outline::{is_home_or_root, is_ignored_path};

#[derive(Debug, Clone)]
pub struct IndexedLine {
    pub line_number: usize,
    pub content: String,
}

#[derive(Debug, Clone)]
pub struct IndexedDocument {
    pub id: usize,
    pub path: PathBuf,
    pub relative_path: String,
    pub lines: Vec<IndexedLine>,
}

#[derive(Debug, Clone)]
pub struct LineMatch {
    pub line_number: usize,
    pub line_content: String,
}

#[derive(Debug, Clone)]
pub struct SearchResult {
    pub relative_path: String,
    pub score: f32,
    pub matches: Vec<LineMatch>,
}

/// Fast inverted in-memory search index for local codebases.
pub struct CodeSearchIndex {
    pub root_dir: PathBuf,
    pub documents: Vec<IndexedDocument>,
    /// Map of normalized token -> Vec<(doc_id, line_number)>
    pub index: HashMap<String, Vec<(usize, usize)>>,
}

impl CodeSearchIndex {
    /// Builds the code search index by crawling `root_dir`.
    pub fn build(root_dir: &Path) -> Self {
        let mut documents = Vec::new();
        collect_index_files(root_dir, root_dir, 0, &mut documents);

        let mut index: HashMap<String, Vec<(usize, usize)>> = HashMap::new();

        for doc in &documents {
            // Index filename terms
            for token in tokenize(&doc.relative_path) {
                index.entry(token).or_default().push((doc.id, 0));
            }

            // Index line contents
            for line in &doc.lines {
                let tokens = tokenize(&line.content);
                for token in tokens {
                    index
                        .entry(token)
                        .or_default()
                        .push((doc.id, line.line_number));
                }
            }
        }

        Self {
            root_dir: root_dir.to_path_buf(),
            documents,
            index,
        }
    }

    /// Performs full-text keyword search with BM25-style frequency scoring.
    pub fn search(&self, query: &str, limit: usize) -> Vec<SearchResult> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Vec::new();
        }

        let mut doc_scores: HashMap<usize, f32> = HashMap::new();
        let mut doc_matches: HashMap<usize, HashMap<usize, String>> = HashMap::new();

        let total_docs = self.documents.len().max(1) as f32;

        for token in &query_tokens {
            if let Some(postings) = self.index.get(token) {
                // Approximate IDF
                let doc_count = postings
                    .iter()
                    .map(|(d, _)| *d)
                    .collect::<HashSet<_>>()
                    .len() as f32;
                let idf = ((total_docs - doc_count + 0.5) / (doc_count + 0.5) + 1.0)
                    .ln()
                    .max(0.1);

                for &(doc_id, line_num) in postings {
                    let score_entry = doc_scores.entry(doc_id).or_insert(0.0);
                    *score_entry += idf;

                    if line_num > 0 {
                        if let Some(doc) = self.documents.get(doc_id) {
                            if let Some(line) = doc.lines.iter().find(|l| l.line_number == line_num)
                            {
                                doc_matches
                                    .entry(doc_id)
                                    .or_default()
                                    .entry(line_num)
                                    .or_insert_with(|| line.content.clone());
                            }
                        }
                    } else {
                        // Bonus for path/filename match
                        *score_entry += 2.0 * idf;
                    }
                }
            }
        }

        let mut scored_results: Vec<(usize, f32)> = doc_scores.into_iter().collect();
        scored_results.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let mut results = Vec::new();
        for (doc_id, score) in scored_results.into_iter().take(limit) {
            if let Some(doc) = self.documents.get(doc_id) {
                let mut line_matches: Vec<LineMatch> = doc_matches
                    .remove(&doc_id)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|(line_number, line_content)| LineMatch {
                        line_number,
                        line_content,
                    })
                    .collect();

                line_matches.sort_by_key(|m| m.line_number);

                results.push(SearchResult {
                    relative_path: doc.relative_path.clone(),
                    score,
                    matches: line_matches.into_iter().take(8).collect(),
                });
            }
        }

        results
    }

    /// Formats search results into a clean markdown block.
    pub fn format_results(&self, query: &str, results: &[SearchResult]) -> String {
        if results.is_empty() {
            return format!("No code search matches found for `{}`.", query);
        }

        let mut out = format!("### Codebase Search Results for `{}`:\n\n", query);
        for res in results {
            out.push_str(&format!(
                "• `{}` (Score: {:.2})\n",
                res.relative_path, res.score
            ));
            for m in &res.matches {
                out.push_str(&format!(
                    "  L{:<4} {}\n",
                    m.line_number,
                    m.line_content.trim_end()
                ));
            }
            out.push('\n');
        }
        out
    }
}

/// Tokenizes text into normalized words, splitting by camelCase, snake_case, and non-alphanumeric chars.
pub fn tokenize(input: &str) -> Vec<String> {
    let mut tokens = HashSet::new();
    let mut current = String::new();

    for ch in input.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            // Check camelCase boundary
            if ch.is_uppercase() && !current.is_empty() && !current.ends_with(char::is_uppercase) {
                let word = current.trim_matches('_').to_lowercase();
                if word.len() >= 2 {
                    tokens.insert(word);
                }
                current.clear();
            }
            current.push(ch);
        } else {
            if !current.is_empty() {
                for part in current.split('_') {
                    let w = part.to_lowercase();
                    if w.len() >= 2 {
                        tokens.insert(w);
                    }
                }
                current.clear();
            }
        }
    }

    if !current.is_empty() {
        for part in current.split('_') {
            let w = part.to_lowercase();
            if w.len() >= 2 {
                tokens.insert(w);
            }
        }
    }

    tokens.into_iter().collect()
}

const ALLOWED_INDEX_EXTENSIONS: &[&str] = &[
    "rs", "py", "js", "ts", "tsx", "jsx", "go", "c", "cpp", "h", "hpp", "cc", "hh", "toml", "json",
    "yaml", "yml", "md", "org", "sh", "bash", "zsh", "html", "css", "sql", "txt", "env", "conf",
    "ini", "xml",
];

const SPECIAL_INDEX_FILES: &[&str] = &[
    "Makefile",
    "Dockerfile",
    "Cargo.lock",
    "README.md",
    "README",
    "AGENTS.md",
    ".orbitrules",
];

fn collect_index_files(
    current_dir: &Path,
    base_dir: &Path,
    depth: usize,
    docs: &mut Vec<IndexedDocument>,
) {
    if docs.len() >= 500 {
        return;
    }
    let max_depth = if is_home_or_root(base_dir) { 1 } else { 6 };
    if depth > max_depth {
        return;
    }

    let entries = match fs::read_dir(current_dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    for entry in entries.flatten() {
        if docs.len() >= 500 {
            break;
        }
        let path = entry.path();
        if is_ignored_path(&path) {
            continue;
        }

        if path.is_dir() {
            if is_home_or_root(base_dir) && depth >= 1 {
                continue;
            }
            collect_index_files(&path, base_dir, depth + 1, docs);
        } else if path.is_file() {
            let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            let is_allowed =
                ALLOWED_INDEX_EXTENSIONS.contains(&ext) || SPECIAL_INDEX_FILES.contains(&filename);

            if !is_allowed {
                continue;
            }

            // Check file size (< 500KB)
            if let Ok(metadata) = path.metadata() {
                if metadata.len() > 500_000 {
                    continue;
                }
            }

            if let Ok(content) = fs::read_to_string(&path) {
                let relative = path
                    .strip_prefix(base_dir)
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|_| path.to_string_lossy().to_string());

                let lines: Vec<IndexedLine> = content
                    .lines()
                    .enumerate()
                    .map(|(idx, line)| IndexedLine {
                        line_number: idx + 1,
                        content: line.to_string(),
                    })
                    .collect();

                let id = docs.len();
                docs.push(IndexedDocument {
                    id,
                    path,
                    relative_path: relative,
                    lines,
                });
            }
        }
    }
}

pub fn execute_code_search_tool(index: &CodeSearchIndex, body: &str) -> String {
    let mut query = "".to_string();
    let mut limit = 5;

    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("query:") {
            query = val.trim().to_string();
        } else if let Some(val) = trimmed.strip_prefix("limit:") {
            if let Ok(l) = val.trim().parse::<usize>() {
                limit = l;
            }
        }
    }

    if query.is_empty() {
        query = body.trim().to_string();
    }

    if query.is_empty() {
        return "Error: Missing `query:` for code_search tool.".to_string();
    }

    let results = index.search(&query, limit);
    index.format_results(&query, &results)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tokenize_camel_and_snake() {
        let tokens = tokenize("execute_tool and runCommandAsync_v2");
        assert!(tokens.iter().any(|t| t == "execute"));
        assert!(tokens.iter().any(|t| t == "tool"));
        assert!(tokens.iter().any(|t| t == "run"));
        assert!(tokens.iter().any(|t| t == "command"));
        assert!(tokens.iter().any(|t| t == "async"));
    }

    #[test]
    fn test_index_and_search() {
        let temp_dir = std::env::temp_dir().join("orbit_test_search_index");
        let _ = fs::remove_dir_all(&temp_dir);
        fs::create_dir_all(&temp_dir).unwrap();

        fs::write(
            temp_dir.join("main.rs"),
            "fn main() {\n    println!(\"hello orbit\");\n}\n",
        )
        .unwrap();

        fs::write(
            temp_dir.join("lib.rs"),
            "pub fn calculate_sum(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
        )
        .unwrap();

        let index = CodeSearchIndex::build(&temp_dir);
        assert_eq!(index.documents.len(), 2);

        let results = index.search("calculate_sum", 5);
        assert!(!results.is_empty());
        assert_eq!(results[0].relative_path, "lib.rs");
        assert_eq!(results[0].matches.len(), 1);
        assert_eq!(results[0].matches[0].line_number, 1);

        let out = index.format_results("calculate_sum", &results);
        assert!(out.contains("lib.rs"));
        assert!(out.contains("calculate_sum"));

        let _ = fs::remove_dir_all(&temp_dir);
    }
}
