use anyhow::Result;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AstFinding {
    pub file: String,
    pub line: usize,
    pub rule: String,
    pub message: String,
}

pub struct AstScanner;

impl AstScanner {
    pub fn is_ast_grep_available() -> bool {
        Command::new("ast-grep")
            .arg("--version")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    pub fn scan_workspace(workspace: &Path) -> Result<Vec<AstFinding>> {
        if Self::is_ast_grep_available() {
            let output = Command::new("ast-grep")
                .args(["scan", "--json"])
                .current_dir(workspace)
                .output()?;

            if let Ok(findings) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) {
                let mut results = Vec::new();
                for f in findings {
                    let file = f["file"].as_str().unwrap_or("unknown").to_string();
                    let line = f["range"]["start"]["line"].as_u64().unwrap_or(1) as usize;
                    let rule = f["ruleId"].as_str().unwrap_or("ast-rule").to_string();
                    let message = f["message"]
                        .as_str()
                        .unwrap_or("Structural defect")
                        .to_string();
                    results.push(AstFinding {
                        file,
                        line,
                        rule,
                        message,
                    });
                }
                return Ok(results);
            }
        }

        // Native fallback scanner for common high-risk patterns
        Self::fallback_pattern_scan(workspace)
    }

    pub fn fallback_pattern_scan(workspace: &Path) -> Result<Vec<AstFinding>> {
        let mut findings = Vec::new();
        for entry in ignore::WalkBuilder::new(workspace).build().flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(ext) = path.extension().and_then(|s| s.to_str()) {
                    if matches!(ext, "rs" | "py" | "js" | "ts") {
                        if let Ok(content) = std::fs::read_to_string(path) {
                            for (idx, line) in content.lines().enumerate() {
                                let line_num = idx + 1;
                                let trimmed = line.trim();
                                if trimmed.starts_with("//") || trimmed.starts_with('#') {
                                    continue;
                                }

                                if ext == "rs" && line.contains(".expect(") && line.contains("TODO")
                                {
                                    findings.push(AstFinding {
                                        file: path.display().to_string(),
                                        line: line_num,
                                        rule: "rust-unimplemented-expect".to_string(),
                                        message: "Found .expect() with TODO in production path"
                                            .to_string(),
                                    });
                                } else if matches!(ext, "py" | "js")
                                    && (line.contains("eval(") || line.contains("exec("))
                                {
                                    findings.push(AstFinding {
                                        file: path.display().to_string(),
                                        line: line_num,
                                        rule: "security-dynamic-eval".to_string(),
                                        message: "Dangerous dynamic code execution detected"
                                            .to_string(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
        }
        Ok(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_fallback_pattern_scan_finds_eval() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let bad_file = ws.join("test.py");
        std::fs::write(&bad_file, "def run(x):\n    eval(x)\n").unwrap();

        let findings = AstScanner::fallback_pattern_scan(ws).unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].rule, "security-dynamic-eval");
        assert_eq!(findings[0].line, 2);
    }
}
