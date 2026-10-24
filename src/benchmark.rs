use anyhow::{anyhow, Result};
use std::path::Path;
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct BenchmarkStats {
    pub command: String,
    pub iterations: usize,
    pub min_ms: f64,
    pub max_ms: f64,
    pub mean_ms: f64,
    pub std_dev_ms: f64,
}

impl BenchmarkStats {
    pub fn format_report(&self) -> String {
        format!(
            r#"| Metric | Value |
|---|---|
| **Command** | `{}` |
| **Runs** | {} |
| **Mean** | {:.2} ms |
| **Min** | {:.2} ms |
| **Max** | {:.2} ms |
| **StdDev** | {:.2} ms |"#,
            self.command, self.iterations, self.mean_ms, self.min_ms, self.max_ms, self.std_dev_ms
        )
    }
}

pub struct BenchmarkRunner;

impl BenchmarkRunner {
    pub fn run(command_str: &str, workspace: &Path, iterations: usize) -> Result<BenchmarkStats> {
        if iterations == 0 {
            return Err(anyhow!("Iterations must be greater than zero"));
        }

        let mut durations = Vec::with_capacity(iterations);

        for _ in 0..iterations {
            let start = Instant::now();
            let status = Command::new("sh")
                .arg("-c")
                .arg(command_str)
                .current_dir(workspace)
                .output()?;

            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            if !status.status.success() {
                return Err(anyhow!(
                    "Benchmark failed during execution: {}",
                    String::from_utf8_lossy(&status.stderr).trim()
                ));
            }
            durations.push(elapsed);
        }

        let min_ms = durations.iter().cloned().fold(f64::INFINITY, f64::min);
        let max_ms = durations.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let mean_ms = durations.iter().sum::<f64>() / (iterations as f64);

        let variance = durations
            .iter()
            .map(|val| {
                let diff = mean_ms - (*val);
                diff * diff
            })
            .sum::<f64>()
            / (iterations as f64);
        let std_dev_ms = variance.sqrt();

        Ok(BenchmarkStats {
            command: command_str.to_string(),
            iterations,
            min_ms,
            max_ms,
            mean_ms,
            std_dev_ms,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_benchmark_simple_command() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let stats = BenchmarkRunner::run("echo 'hello'", ws, 3).unwrap();
        assert_eq!(stats.iterations, 3);
        assert!(stats.min_ms >= 0.0);
        assert!(stats.max_ms >= stats.min_ms);

        let report = stats.format_report();
        assert!(report.contains("| **Command** | `echo 'hello'` |"));
        assert!(report.contains("| **Runs** | 3 |"));
    }
}
