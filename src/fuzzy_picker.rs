use anyhow::Result;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;
use ignore::WalkBuilder;
use regex::Regex;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct FuzzyMatch {
    pub target: String,
    pub score: i64,
    pub indices: Vec<usize>,
    pub detail: Option<String>,
}

pub struct FuzzyPicker;

impl FuzzyPicker {
    pub fn find_files(root: &Path, query: &str, limit: usize) -> Result<Vec<FuzzyMatch>> {
        let matcher = SkimMatcherV2::default();
        let walker = WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .parents(true)
            .build();

        let mut matches = Vec::new();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            if query.is_empty() {
                matches.push(FuzzyMatch {
                    target: rel_path,
                    score: 0,
                    indices: Vec::new(),
                    detail: None,
                });
                if matches.len() >= limit {
                    break;
                }
                continue;
            }

            if let Some((score, indices)) = matcher.fuzzy_indices(&rel_path, query) {
                matches.push(FuzzyMatch {
                    target: rel_path,
                    score,
                    indices,
                    detail: None,
                });
            }
        }

        matches.sort_by_key(|a| std::cmp::Reverse(a.score));
        if matches.len() > limit {
            matches.truncate(limit);
        }

        Ok(matches)
    }

    pub fn find_symbols(root: &Path, query: &str, limit: usize) -> Result<Vec<FuzzyMatch>> {
        let matcher = SkimMatcherV2::default();
        let walker = WalkBuilder::new(root)
            .hidden(true)
            .git_ignore(true)
            .parents(true)
            .build();

        let sym_re = Regex::new(
            r"(?m)^\s*(?:pub(?:\([^\)]+\))?\s+)?(?:export\s+)?(fn|def|struct|class|interface|enum|trait)\s+([A-Za-z0-9_]+)"
        ).unwrap();

        let mut matches = Vec::new();

        for entry in walker.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let ext = match path.extension().and_then(|s| s.to_str()) {
                Some(e) => e.to_lowercase(),
                None => continue,
            };

            if !matches!(ext.as_str(), "rs" | "py" | "ts" | "js" | "go") {
                continue;
            }

            let content = match fs::read_to_string(path) {
                Ok(c) => c,
                Err(_) => continue,
            };

            let rel_path = path
                .strip_prefix(root)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            for cap in sym_re.captures_iter(&content) {
                let kind = cap.get(1).map_or("", |m| m.as_str());
                let name = cap.get(2).map_or("", |m| m.as_str());
                let full_ident = format!("{}::{}", rel_path, name);

                if query.is_empty() {
                    matches.push(FuzzyMatch {
                        target: full_ident,
                        score: 0,
                        indices: Vec::new(),
                        detail: Some(kind.to_string()),
                    });
                    if matches.len() >= limit {
                        break;
                    }
                    continue;
                }

                if let Some((score, indices)) = matcher.fuzzy_indices(&full_ident, query) {
                    matches.push(FuzzyMatch {
                        target: full_ident,
                        score,
                        indices,
                        detail: Some(kind.to_string()),
                    });
                }
            }
        }

        matches.sort_by_key(|a| std::cmp::Reverse(a.score));
        if matches.len() > limit {
            matches.truncate(limit);
        }

        Ok(matches)
    }

    pub fn format_results(matches: &[FuzzyMatch], query: &str) -> String {
        if matches.is_empty() {
            return format!("No fuzzy matches found for '{}'.", query);
        }

        let mut out = format!(
            "🔍 Fuzzy Matches for '{}' ({} results):\n",
            query,
            matches.len()
        );
        for (i, m) in matches.iter().enumerate() {
            if let Some(ref detail) = m.detail {
                out.push_str(&format!(
                    "  {:2}. [{}] {} (score: {})\n",
                    i + 1,
                    detail,
                    m.target,
                    m.score
                ));
            } else {
                out.push_str(&format!(
                    "  {:2}. {} (score: {})\n",
                    i + 1,
                    m.target,
                    m.score
                ));
            }
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
    fn test_fuzzy_find_files() -> Result<()> {
        let temp = tempdir()?;
        fs::write(temp.path().join("main.rs"), "")?;
        fs::write(temp.path().join("modal_prompt.rs"), "")?;
        fs::write(temp.path().join("search_index.rs"), "")?;

        let matches = FuzzyPicker::find_files(temp.path(), "modprom", 5)?;
        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].target, "modal_prompt.rs");
        assert!(matches[0].score > 0);

        Ok(())
    }

    #[test]
    fn test_fuzzy_find_symbols() -> Result<()> {
        let temp = tempdir()?;
        fs::write(
            temp.path().join("lib.rs"),
            "pub fn execute_tool_quiet() {}\npub struct AgentState {}\n",
        )?;

        let matches = FuzzyPicker::find_symbols(temp.path(), "tool_quiet", 5)?;
        assert_eq!(matches.len(), 1);
        assert!(matches[0].target.contains("execute_tool_quiet"));
        assert_eq!(matches[0].detail.as_deref(), Some("fn"));

        Ok(())
    }
}
