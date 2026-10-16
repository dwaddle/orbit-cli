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

/// Detects the fastest compile/type-check command for the workspace.
pub fn detect_compile_command(workspace_dir: &Path) -> (String, Vec<String>) {
    if workspace_dir.join("Cargo.toml").exists() {
        (
            "cargo".to_string(),
            vec!["check".to_string(), "--message-format=short".to_string()],
        )
    } else if workspace_dir.join("tsconfig.json").exists() {
        if Path::new("/usr/bin/bun").exists() || Path::new("/home/rene/.bun/bin/bun").exists() {
            ("bun".to_string(), vec!["check".to_string()])
        } else {
            (
                "npx".to_string(),
                vec!["tsc".to_string(), "--noEmit".to_string()],
            )
        }
    } else if workspace_dir.join("pyproject.toml").exists()
        || workspace_dir.join("setup.py").exists()
    {
        (
            "python3".to_string(),
            vec!["-m".to_string(), "py_compile".to_string()],
        )
    } else if workspace_dir.join("go.mod").exists() {
        (
            "go".to_string(),
            vec!["vet".to_string(), "./...".to_string()],
        )
    } else {
        ("cargo".to_string(), vec!["check".to_string()])
    }
}

/// Executes the detected compile check command and extracts compiler diagnostics.
pub fn run_compile_check(workspace_dir: &Path) -> CompileResult {
    let (program, args) = detect_compile_command(workspace_dir);
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
        run_compile_check(&self.workspace_dir)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_compile_command_cargo() {
        let (cmd, args) = detect_compile_command(Path::new("."));
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
