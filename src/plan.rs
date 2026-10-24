use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub struct PlanGenerator;

impl PlanGenerator {
    pub fn plan_dir(workspace: &Path) -> PathBuf {
        workspace.join(".orbit").join("plans")
    }

    pub fn generate_plan(workspace: &Path, goal: &str) -> Result<(PathBuf, String)> {
        let dir = Self::plan_dir(workspace);
        fs::create_dir_all(&dir)?;

        let timestamp = chrono_timestamp_slug();
        let slug: String = goal
            .to_lowercase()
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect::<String>()
            .split('-')
            .filter(|s| !s.is_empty())
            .take(6)
            .collect::<Vec<&str>>()
            .join("-");

        let file_name = format!(
            "{}-{}.md",
            timestamp,
            if slug.is_empty() { "task-plan" } else { &slug }
        );
        let path = dir.join(&file_name);

        let content = format!(
            r#"# Execution Plan: {}

- **Generated**: {}
- **Status**: Ready for Review

## 1. Goal & Requirements Spec
- **Target Objective**: {}
- **Success Criteria**: 100% test pass, zero regressions, clean diff

## 2. Architecture & Affected Modules
- Inspect touched files and dependency boundaries
- Verify interface and type compatibility

## 3. Incremental Implementation Phases
### Phase 1: Core Types & Contracts
- Define new structs, errors, and trait implementations

### Phase 2: Logic & Dispatch Integration
- Wire logic into CLI REPL and tool dispatcher
- Apply workspace sandboxing and credential redaction

### Phase 3: Comprehensive Verification
- Execute isolated unit tests with zero-flake verification
- Run formatting and AST pattern audits

## 4. Rollback & Contingency Plan
- Instant checkpoint restore via `/undo` or `git checkout`
"#,
            goal,
            chrono_human_date(),
            goal
        );

        fs::write(&path, &content)?;
        Ok((path, content))
    }
}

fn chrono_timestamp_slug() -> String {
    let out = std::process::Command::new("date")
        .arg("+%Y%m%d-%H%M%S")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "20261024-080000".to_string());
    if out.is_empty() {
        "20261024-080000".to_string()
    } else {
        out
    }
}

fn chrono_human_date() -> String {
    let out = std::process::Command::new("date")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "Sat Oct 24 2026".to_string());
    if out.is_empty() {
        "Sat Oct 24 2026".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_generate_plan_file() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let (path, content) =
            PlanGenerator::generate_plan(ws, "Implement ONNX Vector Search").unwrap();
        assert!(path.exists());
        assert!(content.contains("Execution Plan: Implement ONNX Vector Search"));
        assert!(content.contains("Phase 1: Core Types & Contracts"));
    }
}
