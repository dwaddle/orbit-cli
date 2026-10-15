use anyhow::Result;
use colored::*;
use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::outline::is_ignored_path;

#[derive(Debug, Clone)]
pub struct WatcherTestResult {
    pub success: bool,
    pub command: String,
    pub duration_ms: u128,
    pub output: String,
}

/// Detects the standard test runner command for the current codebase.
pub fn detect_test_command(workspace_dir: &Path) -> (String, Vec<String>) {
    if workspace_dir.join("Cargo.toml").exists() {
        ("cargo".to_string(), vec!["test".to_string()])
    } else if workspace_dir.join("pyproject.toml").exists()
        || workspace_dir.join("pytest.ini").exists()
        || workspace_dir.join("tests").exists()
    {
        ("pytest".to_string(), vec![])
    } else if workspace_dir.join("package.json").exists() {
        if Path::new("/usr/bin/bun").exists() || Path::new("/home/rene/.bun/bin/bun").exists() {
            ("bun".to_string(), vec!["test".to_string()])
        } else {
            ("npm".to_string(), vec!["test".to_string()])
        }
    } else if workspace_dir.join("go.mod").exists() {
        (
            "go".to_string(),
            vec!["test".to_string(), "./...".to_string()],
        )
    } else {
        ("cargo".to_string(), vec!["test".to_string()])
    }
}

/// Runs the test command once and captures timing and output.
pub fn run_project_tests(workspace_dir: &Path) -> WatcherTestResult {
    let (program, args) = detect_test_command(workspace_dir);
    let full_cmd = format!("{} {}", program, args.join(" "));
    let start = Instant::now();

    let output = Command::new(&program)
        .args(&args)
        .current_dir(workspace_dir)
        .output();

    let duration_ms = start.elapsed().as_millis();

    match output {
        Ok(out) => {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            WatcherTestResult {
                success: out.status.success(),
                command: full_cmd,
                duration_ms,
                output: combined.trim().to_string(),
            }
        }
        Err(e) => WatcherTestResult {
            success: false,
            command: full_cmd,
            duration_ms,
            output: format!("Failed to launch test runner '{}': {}", program, e),
        },
    }
}

/// Checks whether a modified file event should trigger a test run.
pub fn is_relevant_source_event(path: &Path) -> bool {
    if is_ignored_path(path) {
        return false;
    }
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(
        ext,
        "rs" | "py" | "ts" | "js" | "tsx" | "jsx" | "go" | "c" | "cpp" | "toml" | "json"
    )
}

pub struct OrbitWatcher {
    pub workspace_dir: PathBuf,
    pub is_running: Arc<AtomicBool>,
}

impl OrbitWatcher {
    pub fn new(workspace_dir: &Path) -> Self {
        Self {
            workspace_dir: workspace_dir.to_path_buf(),
            is_running: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Starts the background watching loop in a separate Tokio task.
    pub fn start_background_loop(&self) -> Result<()> {
        let is_running = Arc::clone(&self.is_running);
        if is_running.load(Ordering::SeqCst) {
            return Ok(());
        }
        is_running.store(true, Ordering::SeqCst);

        let workspace = self.workspace_dir.clone();
        let (tx, mut rx) = mpsc::channel(100);

        std::thread::spawn(move || {
            let (event_tx, event_rx) = std::sync::mpsc::channel();
            let mut watcher = match RecommendedWatcher::new(event_tx, Config::default()) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to initialize file watcher: {}", e);
                    return;
                }
            };

            if let Err(e) = watcher.watch(&workspace, RecursiveMode::Recursive) {
                eprintln!("Failed to watch directory: {}", e);
                return;
            }

            while is_running.load(Ordering::SeqCst) {
                if let Ok(Ok(event)) = event_rx.recv_timeout(Duration::from_millis(200)) {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        let should_trigger =
                            event.paths.iter().any(|p| is_relevant_source_event(p));
                        if should_trigger {
                            let _ = tx.blocking_send(());
                        }
                    }
                }
            }
        });

        let ws_clone = self.workspace_dir.clone();
        let running_clone = Arc::clone(&self.is_running);

        tokio::spawn(async move {
            println!(
                "\n{}",
                "🔭 Orbit File Watcher started. Watching for changes..."
                    .bold()
                    .cyan()
            );

            while running_clone.load(Ordering::SeqCst) {
                if rx.recv().await.is_some() {
                    // Debounce rapid bursts: drain any pending notifications in 400ms window
                    tokio::time::sleep(Duration::from_millis(400)).await;
                    while rx.try_recv().is_ok() {}

                    println!(
                        "\n{}",
                        "🔄 Change detected. Running project test suite...".dimmed()
                    );
                    let result = run_project_tests(&ws_clone);

                    if result.success {
                        println!(
                            "{}",
                            format!(
                                "✅ [PASS] {} (completed in {}ms)",
                                result.command, result.duration_ms
                            )
                            .bold()
                            .green()
                        );
                    } else {
                        println!(
                            "{}",
                            format!(
                                "❌ [FAIL] {} (exited with error in {}ms)",
                                result.command, result.duration_ms
                            )
                            .bold()
                            .red()
                        );
                        // Print last 10 lines of error diagnostics
                        let lines: Vec<&str> = result.output.lines().collect();
                        let start_line = lines.len().saturating_sub(12);
                        for l in &lines[start_line..] {
                            println!("   {}", l.red());
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub fn stop(&self) {
        self.is_running.store(false, Ordering::SeqCst);
    }
}

/// Foreground interactive watch mode (for `orbit watch` or `orbit-cli --watch`).
pub async fn run_watch_mode(workspace_dir: &Path) -> Result<()> {
    println!("{}", "=== 🔭 ORBIT BACKGROUND WATCHER ===".bold().cyan());
    println!(
        "Watching directory: {}",
        workspace_dir.display().to_string().yellow()
    );
    let (prog, args) = detect_test_command(workspace_dir);
    println!(
        "Detected test command: {} {}",
        prog.green(),
        args.join(" ").green()
    );
    println!("Press Ctrl+C to stop watching.\n");

    let watcher = OrbitWatcher::new(workspace_dir);
    watcher.start_background_loop()?;

    // Run initial test suite once
    println!("Running initial test suite...");
    let initial_res = run_project_tests(workspace_dir);
    if initial_res.success {
        println!(
            "{}",
            format!(
                "✅ [PASS] Initial tests passed in {}ms",
                initial_res.duration_ms
            )
            .bold()
            .green()
        );
    } else {
        println!(
            "{}",
            format!(
                "❌ [FAIL] Initial tests failed in {}ms",
                initial_res.duration_ms
            )
            .bold()
            .red()
        );
        for line in initial_res.output.lines().take(15) {
            println!("   {}", line.red());
        }
    }

    tokio::signal::ctrl_c().await?;
    println!("\nStopping file watcher...");
    watcher.stop();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_relevant_source_event() {
        assert!(is_relevant_source_event(Path::new("src/main.rs")));
        assert!(is_relevant_source_event(Path::new("app/test.py")));
        assert!(is_relevant_source_event(Path::new("web/index.ts")));
        assert!(!is_relevant_source_event(Path::new("target/debug/app")));
        assert!(!is_relevant_source_event(Path::new(".git/index")));
        assert!(!is_relevant_source_event(Path::new("image.png")));
    }

    #[test]
    fn test_detect_test_command_rust() {
        let (cmd, args) = detect_test_command(Path::new("."));
        assert_eq!(cmd, "cargo");
        assert_eq!(args, vec!["test"]);
    }
}
