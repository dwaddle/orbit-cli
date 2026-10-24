use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub struct ProjectMemory {
    pub file_path: PathBuf,
    pub content: String,
}

impl ProjectMemory {
    pub fn memory_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".orbit").join("memory.md")
    }

    pub fn load_or_init(workspace_root: &Path) -> Result<Self> {
        let file_path = Self::memory_path(workspace_root);
        if file_path.exists() {
            let content = fs::read_to_string(&file_path)?;
            Ok(Self { file_path, content })
        } else {
            let init_content = r#"# Project Memory & Knowledge Graph
Persistent architectural decisions, discovered constraints, and codebase patterns.

## Architecture & Conventions
- Modular Rust design with async Tokio runtime.
- Deterministic test coverage across all tool implementations.

## Discovered Gotchas & Constraints
- Keep host disk clean; avoid unbounded dependency bloat.
"#
            .to_string();

            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file_path, &init_content)?;
            Ok(Self {
                file_path,
                content: init_content,
            })
        }
    }

    pub fn append_entry(&mut self, heading: &str, entry: &str) -> Result<()> {
        let date = chrono_date();
        let formatted = format!("\n### [{}] {}\n- {}\n", date, heading, entry);
        self.content.push_str(&formatted);
        fs::write(&self.file_path, &self.content)?;
        Ok(())
    }

    pub fn format_prompt_injection(&self) -> Option<String> {
        let trimmed = self.content.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(format!("<project_memory>\n{}\n</project_memory>", trimmed))
        }
    }
}

fn chrono_date() -> String {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "2026-10-24 08:00".to_string());
    if out.is_empty() {
        "2026-10-24 08:00".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_project_memory_init_and_append() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut mem = ProjectMemory::load_or_init(ws).unwrap();
        assert!(mem.file_path.exists());
        assert!(mem.content.contains("# Project Memory"));

        mem.append_entry("Security Rule", "Always sanitize path input")
            .unwrap();
        assert!(mem.content.contains("Security Rule"));
        assert!(mem.content.contains("Always sanitize path input"));

        let injection = mem.format_prompt_injection().unwrap();
        assert!(injection.starts_with("<project_memory>"));
        assert!(injection.ends_with("</project_memory>"));
    }
}
