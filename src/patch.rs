use anyhow::{anyhow, Result};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq)]
pub struct PatchBlock {
    pub search: String,
    pub replace: String,
}

#[derive(Debug, Clone)]
pub struct PatchResult {
    pub path: String,
    pub blocks_applied: usize,
    pub original_len: usize,
    pub new_len: usize,
}

/// Parses Aider-style `<<<<<<< SEARCH ... ======= ... >>>>>>> REPLACE` blocks.
pub fn parse_patch_blocks(raw: &str) -> Result<(Option<String>, Vec<PatchBlock>)> {
    let mut target_path = None;
    let mut blocks = Vec::new();

    let mut current_search = String::new();
    let mut current_replace = String::new();
    let mut in_search = false;
    let mut in_replace = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        if !in_search && !in_replace {
            if let Some(p) = trimmed.strip_prefix("path:") {
                target_path = Some(p.trim().to_string());
                continue;
            } else if let Some(p) = trimmed.strip_prefix("file:") {
                target_path = Some(p.trim().to_string());
                continue;
            } else if trimmed.starts_with("--- ") && !trimmed.starts_with("--- SEARCH") {
                target_path = Some(trimmed.trim_start_matches("--- ").trim().to_string());
                continue;
            }
        }

        if trimmed == "<<<<<<< SEARCH" || trimmed == "<<<<<<<" {
            in_search = true;
            in_replace = false;
            current_search.clear();
            current_replace.clear();
            continue;
        }

        if (trimmed == "=======" || trimmed == "======") && in_search {
            in_search = false;
            in_replace = true;
            continue;
        }

        if (trimmed == ">>>>>>> REPLACE" || trimmed == ">>>>>>>") && in_replace {
            in_replace = false;
            // Trim trailing newline added during accumulation if any
            let s = current_search
                .strip_suffix('\n')
                .unwrap_or(&current_search)
                .to_string();
            let r = current_replace
                .strip_suffix('\n')
                .unwrap_or(&current_replace)
                .to_string();
            blocks.push(PatchBlock {
                search: s,
                replace: r,
            });
            current_search.clear();
            current_replace.clear();
            continue;
        }

        if in_search {
            current_search.push_str(line);
            current_search.push('\n');
        } else if in_replace {
            current_replace.push_str(line);
            current_replace.push('\n');
        }
    }

    if in_search || in_replace {
        return Err(anyhow!("Unclosed SEARCH/REPLACE block in patch payload"));
    }

    Ok((target_path, blocks))
}

/// Applies a list of `PatchBlock`s to the given file content.
pub fn apply_patch_blocks_to_content(original: &str, blocks: &[PatchBlock]) -> Result<String> {
    if blocks.is_empty() {
        return Err(anyhow!("No valid SEARCH/REPLACE blocks found in patch"));
    }

    let mut content = original.to_string();

    for (idx, block) in blocks.iter().enumerate() {
        if block.search.is_empty() {
            return Err(anyhow!(
                "Patch block #{} has an empty SEARCH block. Refusing to apply.",
                idx + 1
            ));
        }

        // 1. Direct exact match
        if let Some(pos) = content.find(&block.search) {
            // Check for uniqueness
            let next_pos = content[pos + block.search.len()..].find(&block.search);
            if next_pos.is_some() {
                return Err(anyhow!(
                    "SEARCH block #{} matches multiple locations in the file. Please provide more surrounding context lines to disambiguate:\n```\n{}\n```",
                    idx + 1,
                    block.search
                ));
            }

            content = format!(
                "{}{}{}",
                &content[..pos],
                block.replace,
                &content[pos + block.search.len()..]
            );
            continue;
        }

        // 2. Trailing whitespace normalized match
        let normalized_search = normalize_trailing_spaces(&block.search);
        let normalized_content = normalize_trailing_spaces(&content);

        if let Some(pos) = normalized_content.find(&normalized_search) {
            let next_pos =
                normalized_content[pos + normalized_search.len()..].find(&normalized_search);
            if next_pos.is_some() {
                return Err(anyhow!(
                    "SEARCH block #{} matches multiple locations in the file (fuzzy match). Provide more surrounding context lines to disambiguate.",
                    idx + 1
                ));
            }

            // Map back to original line indices
            let before_lines: Vec<&str> = normalized_content[..pos].lines().collect();
            let search_lines: Vec<&str> = normalized_search.lines().collect();

            let orig_lines: Vec<&str> = content.lines().collect();
            let start_line = before_lines.len();
            let end_line = start_line + search_lines.len();

            if end_line <= orig_lines.len() {
                let mut new_lines = Vec::new();
                new_lines.extend_from_slice(&orig_lines[..start_line]);
                if !block.replace.is_empty() {
                    new_lines.push(&block.replace);
                }
                new_lines.extend_from_slice(&orig_lines[end_line..]);
                content = new_lines.join("\n");
                if original.ends_with('\n') && !content.ends_with('\n') {
                    content.push('\n');
                }
                continue;
            }
        }

        return Err(anyhow!(
            "SEARCH block #{} could not be found in target file. Exact text searched:\n```\n{}\n```\nPlease verify line numbers and surrounding code.",
            idx + 1,
            block.search
        ));
    }

    Ok(content)
}

fn normalize_trailing_spaces(input: &str) -> String {
    input
        .lines()
        .map(|l| l.trim_end())
        .collect::<Vec<&str>>()
        .join("\n")
}

/// Applies a surgical patch to a file on disk.
pub fn apply_patch_to_file(path_str: &str, raw_patch: &str) -> Result<PatchResult> {
    let (embedded_path, blocks) = parse_patch_blocks(raw_patch)?;
    let final_path = if !path_str.trim().is_empty() {
        path_str.trim()
    } else if let Some(ref ep) = embedded_path {
        ep.as_str()
    } else {
        return Err(anyhow!("No target file path specified for apply_patch"));
    };

    let p = Path::new(final_path);
    if !p.exists() {
        return Err(anyhow!("File does not exist: {}", final_path));
    }

    let original = fs::read_to_string(p)?;
    let updated = apply_patch_blocks_to_content(&original, &blocks)?;

    fs::write(p, &updated)?;

    Ok(PatchResult {
        path: final_path.to_string(),
        blocks_applied: blocks.len(),
        original_len: original.len(),
        new_len: updated.len(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_patch_block() {
        let raw = r#"path: src/main.rs
<<<<<<< SEARCH
let x = 1;
=======
let x = 42;
>>>>>>> REPLACE"#;

        let (path, blocks) = parse_patch_blocks(raw).unwrap();
        assert_eq!(path.as_deref(), Some("src/main.rs"));
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].search, "let x = 1;");
        assert_eq!(blocks[0].replace, "let x = 42;");
    }

    #[test]
    fn test_apply_single_patch_block() {
        let original = "fn test() {\n    let x = 1;\n    println!(\"{}\", x);\n}";
        let blocks = vec![PatchBlock {
            search: "let x = 1;".to_string(),
            replace: "let x = 999;".to_string(),
        }];

        let patched = apply_patch_blocks_to_content(original, &blocks).unwrap();
        assert_eq!(
            patched,
            "fn test() {\n    let x = 999;\n    println!(\"{}\", x);\n}"
        );
    }

    #[test]
    fn test_apply_multiple_patch_blocks() {
        let original = "line 1\nline 2\nline 3\nline 4";
        let blocks = vec![
            PatchBlock {
                search: "line 1".to_string(),
                replace: "LINE ONE".to_string(),
            },
            PatchBlock {
                search: "line 3".to_string(),
                replace: "LINE THREE".to_string(),
            },
        ];

        let patched = apply_patch_blocks_to_content(original, &blocks).unwrap();
        assert_eq!(patched, "LINE ONE\nline 2\nLINE THREE\nline 4");
    }

    #[test]
    fn test_ambiguous_search_block_rejected() {
        let original = "foo bar\nother\nfoo bar\n";
        let blocks = vec![PatchBlock {
            search: "foo bar".to_string(),
            replace: "replaced".to_string(),
        }];

        let res = apply_patch_blocks_to_content(original, &blocks);
        assert!(res.is_err());
        assert!(res
            .unwrap_err()
            .to_string()
            .contains("matches multiple locations"));
    }

    #[test]
    fn test_missing_search_block_error() {
        let original = "alpha beta gamma";
        let blocks = vec![PatchBlock {
            search: "delta".to_string(),
            replace: "omega".to_string(),
        }];

        let res = apply_patch_blocks_to_content(original, &blocks);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("could not be found"));
    }
}
