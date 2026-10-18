use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct CompileResult {
    pub success: bool,
    pub command: String,
    pub duration_ms: u128,
    pub output: String,
    pub error_lines: Vec<String>,
}

/// Finds the project root directory starting from `start_dir` and walking upward.
pub fn find_project_root(start_dir: &Path) -> Option<(PathBuf, String, Vec<String>)> {
    let mut current = if start_dir.is_file() {
        start_dir.parent()?.to_path_buf()
    } else {
        start_dir.to_path_buf()
    };

    loop {
        if current.join("Cargo.toml").exists() {
            return Some((
                current,
                "cargo".to_string(),
                vec!["check".to_string(), "--message-format=short".to_string()],
            ));
        }
        if current.join("tsconfig.json").exists() {
            let runner = if Path::new("/usr/bin/bun").exists()
                || Path::new("/home/rene/.bun/bin/bun").exists()
            {
                ("bun".to_string(), vec!["check".to_string()])
            } else {
                (
                    "npx".to_string(),
                    vec!["tsc".to_string(), "--noEmit".to_string()],
                )
            };
            return Some((current, runner.0, runner.1));
        }
        if current.join("pyproject.toml").exists() || current.join("setup.py").exists() {
            return Some((
                current,
                "python3".to_string(),
                vec!["-m".to_string(), "py_compile".to_string()],
            ));
        }
        if current.join("go.mod").exists() {
            return Some((
                current,
                "go".to_string(),
                vec!["vet".to_string(), "./...".to_string()],
            ));
        }

        if !current.pop() {
            break;
        }
    }

    None
}

/// Detects the fastest compile/type-check command for the workspace.
pub fn detect_compile_command(workspace_dir: &Path) -> Option<(PathBuf, String, Vec<String>)> {
    find_project_root(workspace_dir)
}

/// Executes the detected compile check command and extracts compiler diagnostics.
pub fn run_compile_check(workspace_dir: &Path, file_path: Option<&Path>) -> CompileResult {
    let target = if let Some(fp) = file_path {
        // Known non-code document formats skip compile checks
        if let Some(ext) = fp.extension().and_then(|s| s.to_str()) {
            match ext.to_ascii_lowercase().as_str() {
                "md" | "org" | "txt" | "log" | "csv" | "json" | "toml" | "yaml" | "yml" | "xml"
                | "html" | "css" => {
                    return CompileResult {
                        success: true,
                        command: "none".to_string(),
                        duration_ms: 0,
                        output: format!("Skipped compile check for document file ({})", ext),
                        error_lines: Vec::new(),
                    };
                }
                "py" => {
                    if let Some(cmd) = find_project_root(fp) {
                        Some(cmd)
                    } else {
                        Some((
                            fp.parent().unwrap_or(workspace_dir).to_path_buf(),
                            "python3".to_string(),
                            vec![
                                "-m".to_string(),
                                "py_compile".to_string(),
                                fp.to_string_lossy().to_string(),
                            ],
                        ))
                    }
                }
                "sh" | "bash" => Some((
                    fp.parent().unwrap_or(workspace_dir).to_path_buf(),
                    "bash".to_string(),
                    vec!["-n".to_string(), fp.to_string_lossy().to_string()],
                )),
                _ => find_project_root(fp).or_else(|| find_project_root(workspace_dir)),
            }
        } else {
            find_project_root(fp).or_else(|| find_project_root(workspace_dir))
        }
    } else {
        find_project_root(workspace_dir)
    };

    let (root_dir, program, args) = match target {
        Some(t) => t,
        None => {
            return CompileResult {
                success: true,
                command: "none".to_string(),
                duration_ms: 0,
                output: "No build system (Cargo.toml, tsconfig.json, pyproject.toml, go.mod) detected. Compile check skipped.".to_string(),
                error_lines: Vec::new(),
            };
        }
    };

    let full_cmd = format!("{} {}", program, args.join(" "));
    let start = Instant::now();

    let output = Command::new(&program)
        .args(&args)
        .current_dir(&root_dir)
        .output();

    let duration_ms = start.elapsed().as_millis();

    match output {
        Ok(out) => {
            let combined = format!(
                "{}\n{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
            let success = out.status.success();
            let mut error_lines = Vec::new();

            for line in combined.lines() {
                let l = line.trim();
                if l.contains("error:")
                    || l.contains("error[E")
                    || l.contains("SyntaxError")
                    || l.contains("FAIL")
                {
                    error_lines.push(l.to_string());
                }
            }

            CompileResult {
                success,
                command: full_cmd,
                duration_ms,
                output: combined.trim().to_string(),
                error_lines,
            }
        }
        Err(e) => CompileResult {
            success: false,
            command: full_cmd,
            duration_ms,
            output: format!("Failed to launch compile runner '{}': {}", program, e),
            error_lines: vec![format!("Failed to execute: {}", e)],
        },
    }
}

pub fn format_compile_banner(res: &CompileResult) -> String {
    if res.command == "none" {
        return "ℹ️ [COMPILE GUARD SKIPPED] No build system detected in workspace".to_string();
    }
    if res.success {
        format!(
            "✅ [COMPILE GUARD PASSED] `{}` in {}ms",
            res.command, res.duration_ms
        )
    } else {
        let mut out = format!(
            "❌ [COMPILE GUARD FAILED] `{}` ({}ms)\n\n",
            res.command, res.duration_ms
        );
        let preview_lines: Vec<&str> = res.output.lines().take(20).collect();
        out.push_str(&preview_lines.join("\n"));
        if res.output.lines().count() > 20 {
            out.push_str("\n... (truncated compiler diagnostics)");
        }
        out
    }
}

pub struct CompileGuard {
    pub workspace_dir: PathBuf,
}

impl CompileGuard {
    pub fn new(workspace_dir: &Path) -> Self {
        Self {
            workspace_dir: workspace_dir.to_path_buf(),
        }
    }

    pub fn check(&self) -> CompileResult {
        run_compile_check(&self.workspace_dir, None)
    }

    pub fn check_for_path(&self, file_path: &Path) -> CompileResult {
        run_compile_check(&self.workspace_dir, Some(file_path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_compile_command_cargo() {
        let res = detect_compile_command(Path::new("."));
        assert!(res.is_some());
        let (_, cmd, args) = res.unwrap();
        assert_eq!(cmd, "cargo");
        assert_eq!(args[0], "check");
    }

    #[test]
    fn test_format_compile_banner_success() {
        let res = CompileResult {
            success: true,
            command: "cargo check".to_string(),
            duration_ms: 45,
            output: "".to_string(),
            error_lines: vec![],
        };
        let b = format_compile_banner(&res);
        assert!(b.contains("COMPILE GUARD PASSED"));
    }

    #[test]
    fn test_format_compile_banner_failure() {
        let res = CompileResult {
            success: false,
            command: "cargo check".to_string(),
            duration_ms: 120,
            output: "error[E0425]: cannot find value `x` in this scope".to_string(),
            error_lines: vec!["error[E0425]: cannot find value `x` in this scope".to_string()],
        };
        let b = format_compile_banner(&res);
        assert!(b.contains("COMPILE GUARD FAILED"));
        assert!(b.contains("cannot find value `x`"));
    }
}
