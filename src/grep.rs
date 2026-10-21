use anyhow::{Context, Result};
use ignore::WalkBuilder;
use regex::RegexBuilder;
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::Path;

#[derive(Debug, Clone)]
pub struct GrepMatch {
    pub path: String,
    pub line_number: usize,
    pub line_text: String,
}

#[derive(Debug, Clone)]
pub struct NativeGrepConfig {
    pub pattern: String,
    pub is_regex: bool,
    pub case_sensitive: bool,
    pub path_filter: Option<String>,
    pub max_results: usize,
}

impl Default for NativeGrepConfig {
    fn default() -> Self {
        Self {
            pattern: String::new(),
            is_regex: false,
            case_sensitive: false,
            path_filter: None,
            max_results: 100,
        }
    }
}

pub struct NativeGrep;

impl NativeGrep {
    pub fn search(root: &Path, config: &NativeGrepConfig) -> Result<Vec<GrepMatch>> {
        if config.pattern.is_empty() {
            return Ok(Vec::new());
        }

        let regex = if config.is_regex {
            RegexBuilder::new(&config.pattern)
                .case_insensitive(!config.case_sensitive)
                .build()
                .with_context(|| format!("Invalid regex pattern: {}", config.pattern))?
        } else {
            let escaped = regex::escape(&config.pattern);
            RegexBuilder::new(&escaped)
                .case_insensitive(!config.case_sensitive)
                .build()?
        };

        let filter_glob = config.path_filter.as_ref().map(|f| f.to_lowercase());

        let mut matches = Vec::new();
        let walker = WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .git_exclude(true)
            .parents(true)
            .build();

        for entry in walker.flatten() {
            if matches.len() >= config.max_results {
                break;
            }

            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let rel_path = path.strip_prefix(root).unwrap_or(path);
            let rel_str = rel_path.to_string_lossy().to_string();

            if let Some(ref filter) = filter_glob {
                if !rel_str.to_lowercase().contains(filter) {
                    continue;
                }
            }

            // Exclude common binary / media extensions
            if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                let e = ext.to_lowercase();
                if matches!(
                    e.as_str(),
                    "png"
                        | "jpg"
                        | "jpeg"
                        | "gif"
                        | "ico"
                        | "bin"
                        | "lock"
                        | "pdf"
                        | "zip"
                        | "tar"
                        | "gz"
                        | "so"
                        | "exe"
                ) {
                    continue;
                }
            }

            let file = match File::open(path) {
                Ok(f) => f,
                Err(_) => continue,
            };

            let reader = BufReader::new(file);
            for (line_idx, line_res) in reader.lines().enumerate() {
                let line = match line_res {
                    Ok(l) => l,
                    Err(_) => break, // Likely non-UTF8 binary file, stop reading this file
                };

                if regex.is_match(&line) {
                    matches.push(GrepMatch {
                        path: rel_str.clone(),
                        line_number: line_idx + 1,
                        line_text: line.trim().to_string(),
                    });

                    if matches.len() >= config.max_results {
                        break;
                    }
                }
            }
        }

        Ok(matches)
    }

    pub fn format_results(matches: &[GrepMatch], pattern: &str, max_results: usize) -> String {
        if matches.is_empty() {
            return format!("No matches found for '{}'.", pattern);
        }

        let mut out = format!(
            "Found {} match(es) for '{}' (capped at {}):\n",
            matches.len(),
            pattern,
            max_results
        );

        for m in matches {
            let mut line = m.line_text.clone();
            if line.len() > 180 {
                line.truncate(180);
                line.push_str("...");
            }
            out.push_str(&format!("{}:{}: {}\n", m.path, m.line_number, line));
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn test_native_grep_literal_and_case() -> Result<()> {
        let temp = tempdir()?;
        let file1 = temp.path().join("a.rs");
        let file2 = temp.path().join("b.py");

        fs::write(
            &file1,
            "fn execute_tool() {\n    println!(\"Running\");\n}\n",
        )?;
        fs::write(&file2, "def Execute_Tool():\n    pass\n")?;

        let config_case_insensitive = NativeGrepConfig {
            pattern: "execute_tool".to_string(),
            is_regex: false,
            case_sensitive: false,
            path_filter: None,
            max_results: 10,
        };

        let matches = NativeGrep::search(temp.path(), &config_case_insensitive)?;
        assert_eq!(matches.len(), 2);

        let config_case_sensitive = NativeGrepConfig {
            pattern: "execute_tool".to_string(),
            is_regex: false,
            case_sensitive: true,
            path_filter: None,
            max_results: 10,
        };

        let matches_sens = NativeGrep::search(temp.path(), &config_case_sensitive)?;
        assert_eq!(matches_sens.len(), 1);
        assert_eq!(matches_sens[0].line_number, 1);

        Ok(())
    }

    #[test]
    fn test_native_grep_regex_and_filter() -> Result<()> {
        let temp = tempdir()?;
        let rust_file = temp.path().join("test.rs");
        let py_file = temp.path().join("test.py");

        fs::write(&rust_file, "pub struct AgentState {\n    id: u32\n}\n")?;
        fs::write(&py_file, "class AgentState:\n    pass\n")?;

        let config = NativeGrepConfig {
            pattern: r"(struct|class)\s+AgentState".to_string(),
            is_regex: true,
            case_sensitive: true,
            path_filter: Some(".rs".to_string()),
            max_results: 10,
        };

        let matches = NativeGrep::search(temp.path(), &config)?;
        assert_eq!(matches.len(), 1);
        assert!(matches[0].path.ends_with("test.rs"));

        let formatted = NativeGrep::format_results(&matches, "(struct|class) AgentState", 10);
        assert!(formatted.contains("test.rs:1: pub struct AgentState {"));

        Ok(())
    }
}
