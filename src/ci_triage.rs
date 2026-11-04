use anyhow::{anyhow, Result};
use colored::Colorize;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone)]
pub struct CiRunEntry {
    pub id: String,
    pub status: String,
    pub conclusion: String,
    pub workflow: String,
    pub branch: String,
    pub commit: String,
}

#[derive(Debug, Clone)]
pub struct CiFailureReport {
    pub run_id: String,
    pub workflow_name: String,
    pub failed_job: Option<String>,
    pub failed_step: Option<String>,
    pub extracted_errors: Vec<String>,
    pub raw_log_excerpt: String,
}

pub struct CiTriageManager;

impl CiTriageManager {
    pub fn list_runs(workspace: &Path, limit: usize) -> Result<Vec<CiRunEntry>> {
        let output = Command::new("gh")
            .args([
                "run",
                "list",
                "--limit",
                &limit.to_string(),
                "--json",
                "databaseId,status,conclusion,name,headBranch,headSha",
            ])
            .current_dir(workspace)
            .output()?;

        if !output.status.success() {
            return Err(anyhow!(
                "Failed to run gh run list: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let json_str = String::from_utf8_lossy(&output.stdout);
        let parsed: Vec<serde_json::Value> = serde_json::from_str(&json_str)
            .map_err(|e| anyhow!("Failed to parse gh run list JSON: {}", e))?;

        let mut runs = Vec::new();
        for item in parsed {
            runs.push(CiRunEntry {
                id: item["databaseId"].to_string(),
                status: item["status"].as_str().unwrap_or("unknown").to_string(),
                conclusion: item["conclusion"]
                    .as_str()
                    .unwrap_or("in_progress")
                    .to_string(),
                workflow: item["name"].as_str().unwrap_or("CI").to_string(),
                branch: item["headBranch"].as_str().unwrap_or("main").to_string(),
                commit: item["headSha"]
                    .as_str()
                    .unwrap_or("")
                    .chars()
                    .take(7)
                    .collect(),
            });
        }

        Ok(runs)
    }

    pub fn triage_failure(workspace: &Path, run_id: Option<&str>) -> Result<CiFailureReport> {
        let mut cmd = Command::new("gh");
        cmd.current_dir(workspace);

        if let Some(id) = run_id {
            cmd.args(["run", "view", id, "--log-failed"]);
        } else {
            cmd.args(["run", "view", "--log-failed"]);
        }

        let output = cmd.output()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);

        if !output.status.success() && stdout.is_empty() {
            return Err(anyhow!("Failed to fetch CI failure log: {}", stderr.trim()));
        }

        let raw = if !stdout.is_empty() {
            stdout.to_string()
        } else {
            stderr.to_string()
        };
        let extracted_errors = Self::extract_actionable_errors(&raw);

        let report = CiFailureReport {
            run_id: run_id.unwrap_or("latest").to_string(),
            workflow_name: "CI Action".to_string(),
            failed_job: None,
            failed_step: None,
            extracted_errors,
            raw_log_excerpt: raw
                .lines()
                .rev()
                .take(40)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n"),
        };

        Ok(report)
    }

    pub fn extract_actionable_errors(log: &str) -> Vec<String> {
        let mut errors = Vec::new();
        for line in log.lines() {
            let trimmed = line.trim();
            // Match typical compiler / test failure lines
            if trimmed.contains("error[E")
                || trimmed.starts_with("error:")
                || trimmed.contains("FAILED")
                || trimmed.contains("panic")
                || trimmed.contains("Assertion `")
                || trimmed.contains("failure:")
            {
                // Clean ANSI or step prefixes
                let clean = trimmed.split('\t').next_back().unwrap_or(trimmed);
                errors.push(clean.to_string());
                if errors.len() >= 10 {
                    break;
                }
            }
        }
        errors
    }

    pub fn format_triage_banner(report: &CiFailureReport) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}\n",
            "=== CI FAILURE TRIAGE REPORT ===".bold().red()
        ));
        out.push_str(&format!("  Run ID:    {}\n", report.run_id));
        if !report.extracted_errors.is_empty() {
            out.push_str(&format!(
                "{}\n",
                "  Actionable Diagnostic Errors:".bold().yellow()
            ));
            for err in &report.extracted_errors {
                out.push_str(&format!("   {} {}\n", "x".red().bold(), err));
            }
        } else {
            out.push_str(
                "  No structured compiler errors detected. Displaying recent log lines:\n",
            );
        }

        out.push_str(&format!("\n{}\n", "--- Log Excerpt (tail) ---".dimmed()));
        out.push_str(&report.raw_log_excerpt);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_actionable_errors() {
        let sample_log = r#"
2026-11-04T05:00:10Z cargo test
2026-11-04T05:00:12Z error[E0308]: mismatched types
2026-11-04T05:00:13Z    --> src/main.rs:12:5
2026-11-04T05:00:14Z test test_checkpoint_and_rollback ... FAILED
2026-11-04T05:00:15Z thread 'tests::test_checkpoint' panicked at 'assertion failed'
"#;
        let errors = CiTriageManager::extract_actionable_errors(sample_log);
        assert_eq!(errors.len(), 3);
        assert!(errors[0].contains("error[E0308]"));
        assert!(errors[1].contains("FAILED"));
        assert!(errors[2].contains("panicked"));
    }
}
