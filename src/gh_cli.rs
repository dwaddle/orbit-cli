use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

pub struct GhCli;

impl GhCli {
    pub fn is_available() -> bool {
        Command::new("gh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn list_issues(workspace: &Path, limit: usize) -> Result<String> {
        if !Self::is_available() {
            return Err(anyhow!(
                "GitHub CLI ('gh') is not installed or accessible in PATH"
            ));
        }
        let output = Command::new("gh")
            .args(["issue", "list", "--limit", &limit.to_string()])
            .current_dir(workspace)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow!(
                "Failed to list issues: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    pub fn list_prs(workspace: &Path, limit: usize) -> Result<String> {
        if !Self::is_available() {
            return Err(anyhow!(
                "GitHub CLI ('gh') is not installed or accessible in PATH"
            ));
        }
        let output = Command::new("gh")
            .args(["pr", "list", "--limit", &limit.to_string()])
            .current_dir(workspace)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow!(
                "Failed to list PRs: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    pub fn view_pr(workspace: &Path, pr_number: &str) -> Result<String> {
        if !Self::is_available() {
            return Err(anyhow!(
                "GitHub CLI ('gh') is not installed or accessible in PATH"
            ));
        }
        let output = Command::new("gh")
            .args(["pr", "view", pr_number])
            .current_dir(workspace)
            .output()?;

        if output.status.success() {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        } else {
            Err(anyhow!(
                "Failed to view PR #{}: {}",
                pr_number,
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gh_availability_check() {
        let expected = std::process::Command::new("gh")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        let avail = GhCli::is_available();
        assert_eq!(avail, expected);
    }
}
