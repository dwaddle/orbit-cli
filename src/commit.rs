use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConventionalCommit {
    pub commit_type: String,
    pub scope: Option<String>,
    pub summary: String,
    pub body: Vec<String>,
}

impl ConventionalCommit {
    pub fn format(&self) -> String {
        let header = if let Some(scope) = &self.scope {
            format!("{}({}): {}", self.commit_type, scope, self.summary)
        } else {
            format!("{}: {}", self.commit_type, self.summary)
        };

        if self.body.is_empty() {
            header
        } else {
            let mut out = header;
            out.push_str("\n\n");
            for bullet in &self.body {
                out.push_str(&format!("- {}\n", bullet));
            }
            out
        }
    }
}

pub struct CommitGenerator;

impl CommitGenerator {
    pub fn get_diff(workspace_root: &Path) -> Result<String> {
        let staged = Command::new("git")
            .arg("diff")
            .arg("--cached")
            .current_dir(workspace_root)
            .output()?;
        let staged_str = String::from_utf8_lossy(&staged.stdout).to_string();
        if !staged_str.trim().is_empty() {
            return Ok(staged_str);
        }

        let unstaged = Command::new("git")
            .arg("diff")
            .arg("HEAD")
            .current_dir(workspace_root)
            .output()?;
        let unstaged_str = String::from_utf8_lossy(&unstaged.stdout).to_string();
        if !unstaged_str.trim().is_empty() {
            return Ok(unstaged_str);
        }

        let plain_diff = Command::new("git")
            .arg("diff")
            .current_dir(workspace_root)
            .output()?;
        Ok(String::from_utf8_lossy(&plain_diff.stdout).to_string())
    }

    pub fn analyze_and_generate(diff: &str) -> Result<ConventionalCommit> {
        if diff.trim().is_empty() {
            return Err(anyhow!("Cannot generate commit: git diff is empty"));
        }

        let mut touched_files = Vec::new();
        let mut adds = 0;
        let mut dels = 0;

        for line in diff.lines() {
            if line.starts_with("diff --git") {
                let parts: Vec<&str> = line.split_whitespace().collect();
                if parts.len() >= 4 {
                    let file = parts[3].trim_start_matches("b/");
                    touched_files.push(file.to_string());
                }
            } else if line.starts_with('+') && !line.starts_with("+++") {
                adds += 1;
            } else if line.starts_with('-') && !line.starts_with("---") {
                dels += 1;
            }
        }

        // Determine scope and type
        let (commit_type, scope) = if touched_files.iter().all(|f| f.contains("test")) {
            ("test", Some("tests".to_string()))
        } else if touched_files
            .iter()
            .all(|f| f.ends_with(".md") || f.ends_with(".org") || f.starts_with("docs/"))
        {
            ("docs", Some("documentation".to_string()))
        } else if touched_files
            .iter()
            .any(|f| f.contains("fix") || f.contains("error") || f.contains("patch"))
        {
            let scope = touched_files
                .first()
                .and_then(|f| Path::new(f).file_stem())
                .map(|s| s.to_string_lossy().to_string());
            ("fix", scope)
        } else if dels > adds && adds < 10 {
            ("refactor", Some("cleanup".to_string()))
        } else {
            let scope = touched_files
                .first()
                .and_then(|f| Path::new(f).file_stem())
                .map(|s| s.to_string_lossy().to_string());
            ("feat", scope)
        };

        let summary = format!(
            "update {} ({} additions, {} deletions)",
            touched_files.first().map(|s| s.as_str()).unwrap_or("files"),
            adds,
            dels
        );

        let mut body = Vec::new();
        for f in &touched_files {
            body.push(format!("Modified `{}`", f));
        }

        Ok(ConventionalCommit {
            commit_type: commit_type.to_string(),
            scope,
            summary,
            body,
        })
    }

    pub fn execute_commit(workspace_root: &Path, message: &str) -> Result<String> {
        let output = Command::new("git")
            .args(["commit", "-m", message])
            .current_dir(workspace_root)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow!(
                "Git commit failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_conventional_commit_formatting() {
        let commit = ConventionalCommit {
            commit_type: "feat".to_string(),
            scope: Some("security".to_string()),
            summary: "add workspace boundary sandboxing".to_string(),
            body: vec![
                "Canonicalize path".to_string(),
                "Reject escapes".to_string(),
            ],
        };

        let formatted = commit.format();
        assert!(formatted.starts_with("feat(security): add workspace boundary sandboxing\n\n"));
        assert!(formatted.contains("- Canonicalize path\n"));
        assert!(formatted.contains("- Reject escapes\n"));
    }

    #[test]
    fn test_analyze_diff_doc_classification() {
        let mock_diff = r#"
diff --git a/docs/guide.md b/docs/guide.md
index 1234567..89abcdef 100644
--- a/docs/guide.md
+++ b/docs/guide.md
@@ -1,2 +1,3 @@
 # Guide
+New section
"#;
        let res = CommitGenerator::analyze_and_generate(mock_diff).unwrap();
        assert_eq!(res.commit_type, "docs");
        assert_eq!(res.scope, Some("documentation".to_string()));
    }
}
