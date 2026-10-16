use anyhow::Result;
use colored::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Instant;

use crate::watcher::run_project_tests;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TddPhase {
    Red,
    Green,
    Refactor,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TddReport {
    pub feature_spec: String,
    pub phase: TddPhase,
    pub red_test_output: String,
    pub green_test_output: String,
    pub iterations: usize,
    pub duration_ms: u128,
}

pub struct TddOrchestrator;

impl TddOrchestrator {
    /// Conducts an autonomous Red -> Green -> Refactor TDD cycle.
    pub async fn run_tdd_cycle(
        feature_spec: &str,
        workspace_dir: &Path,
        api_key: &str,
        model: &str,
    ) -> Result<TddReport> {
        let start = Instant::now();
        println!(
            "\n{}",
            format!("=== 🧪 STARTING AUTONOMOUS TDD CYCLE: {} ===", feature_spec)
                .bold()
                .magenta()
        );

        // 1. Initial baseline test run
        let initial_tests = run_project_tests(workspace_dir);
        println!(
            "Baseline Tests: {} ({}ms)",
            if initial_tests.success {
                "PASS".green()
            } else {
                "FAIL (warning: existing failures)".yellow()
            },
            initial_tests.duration_ms
        );

        // 2. RED PHASE: Prompt model to write a targeted failing unit test
        println!(
            "\n{}",
            "🔴 [PHASE 1: RED] Generating targeted unit tests..."
                .bold()
                .red()
        );
        let red_prompt = format!(
            "You are a strict Test-Driven Development (TDD) Architect.\n\
            Task: Write a focused unit test for this specification: '{}'.\n\
            Rules:\n\
            1. The test MUST test the new functionality.\n\
            2. The test MUST fail initially because the feature is not yet implemented.\n\
            3. Return the test code inside an appropriate tool call:\n\
            ```tool:write_file\n<file_path>\n<content>\n```\n\
            or\n\
            ```tool:apply_patch\npath: <file_path>\n<<<<<<< SEARCH\n...\n=======\n...\n>>>>>>> REPLACE\n```",
            feature_spec
        );

        let test_response =
            crate::subagent::run_subagent_task(model, "TDD Test Architect", &red_prompt, api_key)
                .await?;

        // Apply tool action from model response
        apply_embedded_tools(&test_response, workspace_dir)?;

        // Verify test runner catches failure or new test
        let red_run = run_project_tests(workspace_dir);
        println!(
            "Red Phase Test Run: {} (Exit: {})",
            if !red_run.success {
                "RED CONFIRMED (Test Fails as Expected)".green()
            } else {
                "All tests passed or compiled cleanly".yellow()
            },
            if red_run.success { "0" } else { "error" }
        );

        let red_output = red_run.output.clone();

        // 3. GREEN PHASE: Implement minimal code to satisfy the tests
        println!(
            "\n{}",
            "🟢 [PHASE 2: GREEN] Implementing minimal code to satisfy tests..."
                .bold()
                .green()
        );
        let mut iterations = 0;
        let mut green_achieved = false;
        let mut last_green_output = String::new();

        while iterations < 3 && !green_achieved {
            iterations += 1;
            println!("  Iteration {}/3...", iterations);

            let green_prompt = format!(
                "You are a Senior Implementation Engineer in a TDD cycle.\n\
                Feature Specification: '{}'\n\n\
                Current Test Diagnostics / Failure Output:\n```\n{}\n```\n\n\
                Task: Implement the MINIMAL code required to make all tests pass.\n\
                Emit your code change using ```tool:write_file or ```tool:apply_patch:\n\
                ```tool:apply_patch\npath: <path>\n<<<<<<< SEARCH\n...\n=======\n...\n>>>>>>> REPLACE\n```",
                feature_spec,
                last_green_output.lines().take(40).collect::<Vec<&str>>().join("\n")
            );

            let impl_response = crate::subagent::run_subagent_task(
                model,
                "TDD Implementation Engineer",
                &green_prompt,
                api_key,
            )
            .await?;

            apply_embedded_tools(&impl_response, workspace_dir)?;

            let green_run = run_project_tests(workspace_dir);
            last_green_output = green_run.output.clone();

            if green_run.success {
                println!(
                    "{}",
                    "  ✅ Green phase succeeded! All tests pass.".bold().green()
                );
                green_achieved = true;
                break;
            } else {
                println!("  ❌ Tests still failing. Retrying with error diagnostics...");
            }
        }

        let duration_ms = start.elapsed().as_millis();

        if green_achieved {
            Ok(TddReport {
                feature_spec: feature_spec.to_string(),
                phase: TddPhase::Completed,
                red_test_output: red_output,
                green_test_output: last_green_output,
                iterations,
                duration_ms,
            })
        } else {
            Ok(TddReport {
                feature_spec: feature_spec.to_string(),
                phase: TddPhase::Failed(
                    "Failed to reach Green phase within 3 iterations".to_string(),
                ),
                red_test_output: red_output,
                green_test_output: last_green_output,
                iterations,
                duration_ms,
            })
        }
    }
}

pub fn format_tdd_banner(report: &TddReport) -> String {
    let mut out = format!(
        "=== 🧪 TDD CYCLE REPORT: {} ===\n",
        match &report.phase {
            TddPhase::Completed => "✅ COMPLETED (100% Tests Green)".bold().green().to_string(),
            TddPhase::Failed(err) => format!("❌ FAILED ({})", err).bold().red().to_string(),
            _ => "IN PROGRESS".yellow().to_string(),
        }
    );
    out.push_str(&format!("• Feature: {}\n", report.feature_spec));
    out.push_str(&format!("• Iterations: {}\n", report.iterations));
    out.push_str(&format!("• Duration: {}ms\n\n", report.duration_ms));

    if report.phase == TddPhase::Completed {
        out.push_str("#### Green Test Run Output:\n```\n");
        let lines: Vec<&str> = report.green_test_output.lines().take(15).collect();
        out.push_str(&lines.join("\n"));
        out.push_str("\n```\n");
    } else {
        out.push_str("#### Last Test Diagnostics:\n```\n");
        let lines: Vec<&str> = report.green_test_output.lines().take(25).collect();
        out.push_str(&lines.join("\n"));
        out.push_str("\n```\n");
    }

    out
}

fn apply_embedded_tools(response: &str, workspace_dir: &Path) -> Result<()> {
    let write_re = regex::Regex::new(r"```tool:write_file\n([\s\S]*?)```")?;
    for cap in write_re.captures_iter(response) {
        let body = &cap[1];
        let mut parts = body.trim_start().splitn(2, '\n');
        let path_str = parts.next().unwrap_or("").trim();
        let content = parts.next().unwrap_or("");
        if !path_str.is_empty() {
            let full_path = workspace_dir.join(path_str);
            if let Some(parent) = full_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            std::fs::write(&full_path, content)?;
            println!(
                "  [TDD Action] Wrote {} bytes to {}",
                content.len(),
                path_str.cyan()
            );
        }
    }

    let patch_re = regex::Regex::new(r"```tool:apply_patch\n([\s\S]*?)```")?;
    for cap in patch_re.captures_iter(response) {
        let body = &cap[1];
        if let Ok(res) = crate::patch::apply_patch_to_file("", body) {
            println!(
                "  [TDD Action] Applied {} patch block(s) to {}",
                res.blocks_applied,
                res.path.cyan()
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_tdd_banner_completed() {
        let report = TddReport {
            feature_spec: "User login".to_string(),
            phase: TddPhase::Completed,
            red_test_output: "FAILED".to_string(),
            green_test_output: "test result: ok. 5 passed".to_string(),
            iterations: 2,
            duration_ms: 1200,
        };
        let banner = format_tdd_banner(&report);
        assert!(banner.contains("COMPLETED"));
        assert!(banner.contains("User login"));
    }
}
