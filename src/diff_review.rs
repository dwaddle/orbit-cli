use anyhow::{anyhow, Result};
use colored::*;
use std::io::{self, Write};
use std::path::Path;
use std::process::Command;

pub fn format_colored_diff(diff: &str) -> String {
    let mut out = String::new();
    for line in diff.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            out.push_str(&format!("{}\n", line.bold().white()));
        } else if line.starts_with('+') {
            out.push_str(&format!("{}\n", line.green()));
        } else if line.starts_with('-') {
            out.push_str(&format!("{}\n", line.red()));
        } else if line.starts_with("@@") {
            out.push_str(&format!("{}\n", line.cyan()));
        } else if line.starts_with("diff --git") || line.starts_with("index ") {
            out.push_str(&format!("{}\n", line.dimmed()));
        } else {
            out.push_str(&format!("{}\n", line));
        }
    }
    out
}

pub fn get_git_diff(repo_root: &Path, file_path: Option<&str>) -> Result<String> {
    let mut cmd = Command::new("git");
    cmd.arg("diff").current_dir(repo_root);

    if let Some(fp) = file_path {
        cmd.arg(fp);
    }

    let output = cmd.output()?;
    if !output.status.success() {
        let err = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!("git diff failed: {}", err));
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

pub fn generate_unified_diff(original: &str, modified: &str, file_name: &str) -> String {
    let orig_lines: Vec<&str> = original.lines().collect();
    let mod_lines: Vec<&str> = modified.lines().collect();

    let mut diff = String::new();
    diff.push_str(&format!("--- a/{}\n", file_name));
    diff.push_str(&format!("+++ b/{}\n", file_name));

    // Simple line diff for preview
    let mut i = 0;
    let mut j = 0;

    let mut hunk_header_emitted = false;

    while i < orig_lines.len() || j < mod_lines.len() {
        if i < orig_lines.len() && j < mod_lines.len() && orig_lines[i] == mod_lines[j] {
            // unchanged
            i += 1;
            j += 1;
        } else {
            if !hunk_header_emitted {
                diff.push_str("@@ -1 +1 @@\n");
                hunk_header_emitted = true;
            }
            if i < orig_lines.len() && (j >= mod_lines.len() || orig_lines[i] != mod_lines[j]) {
                diff.push_str(&format!("-{}\n", orig_lines[i]));
                i += 1;
            }
            if j < mod_lines.len()
                && (i > orig_lines.len()
                    || orig_lines.get(i.saturating_sub(1)) != Some(&mod_lines[j]))
            {
                diff.push_str(&format!("+{}\n", mod_lines[j]));
                j += 1;
            }
        }
    }

    if !hunk_header_emitted && original != modified {
        diff.push_str("@@ -1 +1 @@\n");
        for line in mod_lines {
            diff.push_str(&format!("+{}\n", line));
        }
    }

    diff
}

pub fn prompt_diff_approval(diff: &str, file_name: &str) -> Result<bool> {
    if diff.trim().is_empty() {
        return Ok(true);
    }

    println!(
        "{}",
        "\n🔍 === ORBIT INTERACTIVE DIFF REVIEW ===".bold().cyan()
    );
    print!("{}", format_colored_diff(diff));
    println!("{}", "=========================================".cyan());
    print!(
        "Apply this change to {}? [y/N/e (edit in $EDITOR)]: ",
        file_name.bold().yellow()
    );
    io::stdout().flush()?;

    let mut response = String::new();
    io::stdin().read_line(&mut response)?;
    let ans = response.trim().to_lowercase();

    if ans == "y" || ans == "yes" {
        Ok(true)
    } else if ans == "e" || ans == "edit" {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "nano".to_string());
        println!("Opening {} with {}...", file_name, editor);
        let _ = Command::new(editor).arg(file_name).status();
        Ok(true)
    } else {
        println!("{}", "❌ Modification rejected by user.".yellow());
        Ok(false)
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_format_colored_diff() {
        let raw = "--- a/test.rs\n+++ b/test.rs\n@@ -1,3 +1,3 @@\n-old line\n+new line\n unchanged";
        let colored = format_colored_diff(raw);
        assert!(colored.contains("old line"));
        assert!(colored.contains("new line"));
    }

    #[test]
    fn test_generate_unified_diff() {
        let orig = "fn main() {\n    println!(\"hello\");\n}\n";
        let modified = "fn main() {\n    println!(\"world\");\n}\n";
        let diff = generate_unified_diff(orig, modified, "src/main.rs");
        assert!(diff.contains("--- a/src/main.rs"));
        assert!(diff.contains("+++ b/src/main.rs"));
        assert!(diff.contains("-    println!(\"hello\");"));
        assert!(diff.contains("+    println!(\"world\");"));
    }
}
