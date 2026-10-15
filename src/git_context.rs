use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;

/// Checks if a directory is inside a git work tree.
pub fn is_git_repo(dir: &Path) -> bool {
    let output = Command::new("git")
        .arg("-C")
        .arg(dir)
        .arg("rev-parse")
        .arg("--is-inside-work-tree")
        .output();

    match output {
        Ok(out) => out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true",
        Err(_) => false,
    }
}

/// Retrieves git commit history.
/// If `search_query` is provided, performs a pickaxe search (`-S`) to find commits that introduced or removed the string.
pub fn get_git_log(
    dir: &Path,
    path: Option<&str>,
    search_query: Option<&str>,
    limit: usize,
) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).arg("log");
    cmd.arg(format!("-n{}", limit));
    cmd.arg("--pretty=format:%h | %an | %ad | %s");
    cmd.arg("--date=short");

    if let Some(query) = search_query {
        let trimmed = query.trim();
        if !trimmed.is_empty() {
            cmd.arg(format!("-S{}", trimmed));
        }
    }

    if let Some(file_path) = path {
        let trimmed = file_path.trim();
        if !trimmed.is_empty() {
            cmd.arg("--").arg(trimmed);
        }
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git log failed: {}", err.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        Ok("No matching commits found.".to_string())
    } else {
        Ok(stdout.to_string())
    }
}

/// Retrieves git blame information for a file, optionally bounded to a line range.
pub fn get_git_blame(
    dir: &Path,
    file_path: &str,
    line_range: Option<(usize, usize)>,
) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).arg("blame");
    cmd.arg("--date=short");

    if let Some((start, end)) = line_range {
        cmd.arg(format!("-L{},{}", start, end));
    }

    cmd.arg("--").arg(file_path);

    let output = cmd.output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git blame failed: {}", err.trim()));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// Retrieves git diff (unstaged by default, or staged if `cached == true`).
pub fn get_git_diff(dir: &Path, file_path: Option<&str>, cached: bool) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("-C").arg(dir).arg("diff");

    if cached {
        cmd.arg("--cached");
    }

    if let Some(path) = file_path {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            cmd.arg("--").arg(trimmed);
        }
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git diff failed: {}", err.trim()));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    if stdout.trim().is_empty() {
        Ok("No differences detected.".to_string())
    } else {
        Ok(stdout.to_string())
    }
}

/// Dispatches git commands invoked from the model tool `git_context`.
pub fn execute_git_tool(dir: &Path, body: &str) -> String {
    if !is_git_repo(dir) {
        return format!(
            "Error: Working directory `{}` is not a git repository.",
            dir.display()
        );
    }

    let mut action = "log".to_string();
    let mut path = None;
    let mut query = None;
    let mut range = None;
    let mut limit = 15;

    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(val) = trimmed.strip_prefix("action:") {
            action = val.trim().to_lowercase();
        } else if let Some(val) = trimmed.strip_prefix("path:") {
            let p = val.trim();
            if !p.is_empty() {
                path = Some(p.to_string());
            }
        } else if let Some(val) = trimmed.strip_prefix("query:") {
            let q = val.trim();
            if !q.is_empty() {
                query = Some(q.to_string());
            }
        } else if let Some(val) = trimmed.strip_prefix("range:") {
            let parts: Vec<&str> = val.split(',').map(|s| s.trim()).collect();
            if parts.len() == 2 {
                if let (Ok(s), Ok(e)) = (parts[0].parse::<usize>(), parts[1].parse::<usize>()) {
                    range = Some((s, e));
                }
            }
        } else if let Some(val) = trimmed.strip_prefix("limit:") {
            if let Ok(l) = val.trim().parse::<usize>() {
                limit = l;
            }
        }
    }

    match action.as_str() {
        "log" => match get_git_log(dir, path.as_deref(), query.as_deref(), limit) {
            Ok(res) => format!("### Git Commit History:\n```\n{}\n```", res),
            Err(e) => format!("Git log error: {}", e),
        },
        "search" => match get_git_log(dir, path.as_deref(), query.as_deref(), limit) {
            Ok(res) => format!("### Git Pickaxe (-S) History:\n```\n{}\n```", res),
            Err(e) => format!("Git search error: {}", e),
        },
        "blame" => {
            let file = match path {
                Some(p) => p,
                None => return "Error: `path:` is required for action `blame`.".to_string(),
            };
            match get_git_blame(dir, &file, range) {
                Ok(res) => format!("### Git Blame for `{}`:\n```\n{}\n```", file, res),
                Err(e) => format!("Git blame error: {}", e),
            }
        }
        "diff" => match get_git_diff(dir, path.as_deref(), false) {
            Ok(res) => format!("### Git Diff:\n```diff\n{}\n```", res),
            Err(e) => format!("Git diff error: {}", e),
        },
        _ => format!(
            "Unknown git action `{}`. Supported actions: log, search, blame, diff",
            action
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_git_repo_on_current_dir() {
        let current_dir = std::env::current_dir().unwrap();
        // Since we are in orbit-cli git repo
        assert!(is_git_repo(&current_dir));
    }

    #[test]
    fn test_git_log_execution() {
        let current_dir = std::env::current_dir().unwrap();
        let log = get_git_log(&current_dir, None, None, 5);
        assert!(log.is_ok());
        let content = log.unwrap();
        assert!(!content.is_empty());
    }

    #[test]
    fn test_execute_git_tool_parsing() {
        let current_dir = std::env::current_dir().unwrap();
        let body = "action: log\nlimit: 3";
        let out = execute_git_tool(&current_dir, body);
        assert!(out.contains("Git Commit History"));
    }

    #[test]
    fn test_execute_git_tool_blame_missing_path() {
        let current_dir = std::env::current_dir().unwrap();
        let body = "action: blame";
        let out = execute_git_tool(&current_dir, body);
        assert!(out.contains("path:"));
    }
}
