use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

pub struct GitBisect;

impl GitBisect {
    pub fn start(workspace: &Path, bad: Option<&str>, good: Option<&str>) -> Result<String> {
        let mut cmd = Command::new("git");
        cmd.arg("bisect").arg("start").current_dir(workspace);
        if let Some(bad_rev) = bad {
            cmd.arg(bad_rev);
        }
        if let Some(good_rev) = good {
            cmd.arg(good_rev);
        }
        let out = cmd.output()?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
        } else {
            Err(anyhow!(
                "git bisect start failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ))
        }
    }

    pub fn run_automated(workspace: &Path, test_command: &str) -> Result<String> {
        let out = Command::new("sh")
            .args(["-c", &format!("git bisect run {}", test_command)])
            .current_dir(workspace)
            .output()?;

        let stdout = String::from_utf8_lossy(&out.stdout).to_string();
        let stderr = String::from_utf8_lossy(&out.stderr).to_string();
        if out.status.success() || stdout.contains("is the first bad commit") {
            Ok(stdout.trim().to_string())
        } else {
            Err(anyhow!("git bisect run failed: {}\n{}", stdout, stderr))
        }
    }

    pub fn reset(workspace: &Path) -> Result<String> {
        let out = Command::new("git")
            .args(["bisect", "reset"])
            .current_dir(workspace)
            .output()?;
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_git_bisect_reset_on_non_bisecting_repo() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        Command::new("git")
            .arg("init")
            .current_dir(ws)
            .output()
            .unwrap();
        // Resetting when not bisecting returns clean status or empty
        let res = GitBisect::reset(ws);
        assert!(res.is_ok());
    }
}
