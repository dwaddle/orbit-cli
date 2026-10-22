use anyhow::{anyhow, Result};
use colored::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone)]
pub struct WorktreeSandbox {
    pub id: String,
    pub path: PathBuf,
    pub branch: String,
    pub repo_root: PathBuf,
    pub created_at: u64,
}

impl WorktreeSandbox {
    pub fn new(repo_root: &Path, task_name: &str) -> Result<Self> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let clean_task: String = task_name
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let clean_task = clean_task.trim_matches('_');
        let task_slug = if clean_task.is_empty() {
            "task"
        } else {
            clean_task
        };

        let id = format!("{}_{}", task_slug, ts);
        let branch = format!("orbit-sandbox-{}", id);
        let sandboxes_dir = repo_root.join(".git").join("orbit-sandboxes");
        fs::create_dir_all(&sandboxes_dir)?;
        let path = sandboxes_dir.join(&id);

        let output = Command::new("git")
            .arg("worktree")
            .arg("add")
            .arg("-b")
            .arg(&branch)
            .arg(&path)
            .arg("HEAD")
            .current_dir(repo_root)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to create git worktree: {}", stderr));
        }

        Ok(Self {
            id,
            path,
            branch,
            repo_root: repo_root.to_path_buf(),
            created_at: ts,
        })
    }

    pub fn diff(&self) -> Result<String> {
        let output = Command::new("git")
            .arg("diff")
            .arg("HEAD")
            .current_dir(&self.path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!("Failed to run git diff in sandbox: {}", stderr));
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub fn apply_to_main(&self) -> Result<String> {
        let diff_content = self.diff()?;
        if diff_content.trim().is_empty() {
            return Ok("No changes detected in sandbox to apply.".to_string());
        }

        // Try applying git diff directly to repo_root
        let mut child = Command::new("git")
            .arg("apply")
            .arg("--whitespace=nowarn")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .current_dir(&self.repo_root)
            .spawn()?;

        if let Some(mut stdin) = child.stdin.take() {
            use std::io::Write;
            stdin.write_all(diff_content.as_bytes())?;
        }

        let output = child.wait_with_output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(anyhow!(
                "Failed to apply sandbox diff to main workspace: {}",
                stderr
            ));
        }

        Ok(format!(
            "Successfully applied speculative sandbox changes (branch {}) to main workspace.",
            self.branch
        ))
    }

    pub fn discard(&self) -> Result<()> {
        if self.path.exists() {
            let _ = Command::new("git")
                .arg("worktree")
                .arg("remove")
                .arg("--force")
                .arg(&self.path)
                .current_dir(&self.repo_root)
                .output();

            if self.path.exists() {
                let _ = fs::remove_dir_all(&self.path);
            }
        }

        let _ = Command::new("git")
            .arg("branch")
            .arg("-D")
            .arg(&self.branch)
            .current_dir(&self.repo_root)
            .output();

        Ok(())
    }
}

pub fn get_repo_root(dir: &Path) -> Option<PathBuf> {
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("--show-toplevel")
        .current_dir(dir)
        .output()
        .ok()?;

    if output.status.success() {
        let path_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !path_str.is_empty() {
            return Some(PathBuf::from(path_str));
        }
    }
    None
}

pub fn list_active_sandboxes(repo_root: &Path) -> Vec<PathBuf> {
    let sandboxes_dir = repo_root.join(".git").join("orbit-sandboxes");
    if !sandboxes_dir.exists() {
        return Vec::new();
    }

    let mut list = Vec::new();
    if let Ok(entries) = fs::read_dir(&sandboxes_dir) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.is_dir() {
                list.push(p);
            }
        }
    }
    list
}

pub fn format_sandbox_banner(sandbox: &WorktreeSandbox, action: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "🌿 === ORBIT SPECULATIVE WORKTREE SANDBOX [{}] ===\n",
        action.to_uppercase().cyan()
    ));
    out.push_str(&format!(
        "  Sandbox ID:    {}\n",
        sandbox.id.bold().yellow()
    ));
    out.push_str(&format!(
        "  Worktree Path: {}\n",
        sandbox.path.display().to_string().cyan()
    ));
    out.push_str(&format!("  Git Branch:    {}\n", sandbox.branch.green()));
    out.push_str(&format!(
        "  Original Root: {}\n",
        sandbox.repo_root.display().to_string().dimmed()
    ));
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use tempfile::tempdir;

    fn init_test_git_repo(dir: &Path) {
        Command::new("git")
            .arg("init")
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .arg("config")
            .arg("user.name")
            .arg("Orbit Test")
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .arg("config")
            .arg("user.email")
            .arg("test@orbit.local")
            .current_dir(dir)
            .output()
            .unwrap();
        fs::write(dir.join("init.txt"), "initial content\n").unwrap();
        Command::new("git")
            .arg("add")
            .arg("init.txt")
            .current_dir(dir)
            .output()
            .unwrap();
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg("initial commit")
            .current_dir(dir)
            .output()
            .unwrap();
    }

    #[test]
    fn test_create_and_discard_sandbox() {
        let td = tempdir().unwrap();
        let repo_root = td.path();
        init_test_git_repo(repo_root);

        let sandbox = WorktreeSandbox::new(repo_root, "refactor-auth").unwrap();
        assert!(sandbox.path.exists());
        assert!(sandbox.branch.starts_with("orbit-sandbox-refactor-auth"));

        // Write changes in sandbox
        fs::write(
            sandbox.path.join("sandbox_change.txt"),
            "modified in sandbox",
        )
        .unwrap();
        let _diff = sandbox.diff().unwrap();
        // Since sandbox_change.txt is untracked until git add, let's modify existing file
        fs::write(sandbox.path.join("init.txt"), "modified content\n").unwrap();
        let diff = sandbox.diff().unwrap();
        assert!(diff.contains("+modified content"));

        // Discard sandbox
        sandbox.discard().unwrap();
        assert!(!sandbox.path.exists());
    }

    #[test]
    fn test_apply_sandbox_to_main() {
        let td = tempdir().unwrap();
        let repo_root = td.path();
        init_test_git_repo(repo_root);

        let sandbox = WorktreeSandbox::new(repo_root, "test-apply").unwrap();
        fs::write(sandbox.path.join("init.txt"), "speculative change\n").unwrap();

        let res = sandbox.apply_to_main().unwrap();
        assert!(res.contains("Successfully applied"));

        let main_content = fs::read_to_string(repo_root.join("init.txt")).unwrap();
        assert_eq!(main_content, "speculative change\n");

        sandbox.discard().unwrap();
    }
}
