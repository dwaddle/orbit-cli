use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub struct AdrManager;

impl AdrManager {
    pub fn get_adr_dir(workspace_root: &Path) -> PathBuf {
        workspace_root.join("docs").join("adr")
    }

    pub fn list_adrs(workspace_root: &Path) -> Result<Vec<(u32, String, PathBuf)>> {
        let dir = Self::get_adr_dir(workspace_root);
        if !dir.exists() {
            return Ok(Vec::new());
        }

        let mut adrs = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                if let Some(file_name) = path.file_stem().and_then(|s| s.to_str()) {
                    let parts: Vec<&str> = file_name.splitn(2, '-').collect();
                    if parts.len() == 2 {
                        if let Ok(num) = parts[0].parse::<u32>() {
                            let title = parts[1].replace('_', " ");
                            adrs.push((num, title, path));
                        }
                    }
                }
            }
        }
        adrs.sort_by_key(|(num, _, _)| *num);
        Ok(adrs)
    }

    pub fn create_adr(workspace_root: &Path, title: &str) -> Result<(PathBuf, String)> {
        let dir = Self::get_adr_dir(workspace_root);
        fs::create_dir_all(&dir)?;

        let existing = Self::list_adrs(workspace_root)?;
        let next_num = existing.last().map(|(num, _, _)| num + 1).unwrap_or(1);

        let slug: String = title
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .collect::<Vec<&str>>()
            .join("-");

        let file_name = format!("{:04}-{}.md", next_num, slug);
        let path = dir.join(&file_name);

        let date = chrono_date();
        let content = format!(
            r#"# ADR-{:04}: {}

- **Status**: Proposed
- **Date**: {}
- **Deciders**: Orbit Autonomous Agent & Maintainers

## Context and Problem Statement
Briefly describe the architectural context and problem requiring this decision.

## Decision Drivers
- Yield and performance maximization
- Backward compatibility and stability
- Maintainability and zero defect guarantee

## Considered Options
1. Option 1: Description
2. Option 2: Description

## Decision Outcome
Chosen option: "Option 1", because...

### Positive Consequences
- Improved decoupling and clarity
- Deterministic verification

### Negative Consequences
- Slight initial overhead during migration

## Pros and Cons of the Options
### Option 1
- Good, because...
- Bad, because...

## Compliance & Verification
- Unit and integration tests added in CI
- Zero-flake verification pass
"#,
            next_num, title, date
        );

        fs::write(&path, &content)?;
        Ok((path, content))
    }
}

fn chrono_date() -> String {
    // Standard ISO 8601 YYYY-MM-DD
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "2026-10-24".to_string());
    if out.is_empty() {
        "2026-10-24".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_create_and_list_adrs() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let (path1, content1) =
            AdrManager::create_adr(ws, "Use SQLite for Prompt Caching").unwrap();
        assert!(path1.exists());
        assert!(path1
            .to_str()
            .unwrap()
            .contains("0001-use-sqlite-for-prompt-caching.md"));
        assert!(content1.contains("# ADR-0001: Use SQLite for Prompt Caching"));

        let (path2, _) =
            AdrManager::create_adr(ws, "Adopt Bubblewrap Sandbox for Tool Execution").unwrap();
        assert!(path2
            .to_str()
            .unwrap()
            .contains("0002-adopt-bubblewrap-sandbox-for-tool-execution.md"));

        let adrs = AdrManager::list_adrs(ws).unwrap();
        assert_eq!(adrs.len(), 2);
        assert_eq!(adrs[0].0, 1);
        assert_eq!(adrs[1].0, 2);
    }
}
