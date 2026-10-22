use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspLanguage {
    Rust,
    Python,
    TypeScript,
    Go,
}

pub struct LspDetector;

impl LspDetector {
    pub fn detect(workspace: &Path) -> Option<(LspLanguage, String, Vec<String>)> {
        // Rust check
        if workspace.join("Cargo.toml").exists() && Self::has_binary("rust-analyzer") {
            return Some((LspLanguage::Rust, "rust-analyzer".to_string(), vec![]));
        }

        // Python check
        if workspace.join("pyproject.toml").exists()
            || workspace.join("setup.py").exists()
            || workspace.join("requirements.txt").exists()
        {
            if Self::has_binary("pyright-langserver") {
                return Some((
                    LspLanguage::Python,
                    "pyright-langserver".to_string(),
                    vec!["--stdio".to_string()],
                ));
            } else if Self::has_binary("pylsp") {
                return Some((LspLanguage::Python, "pylsp".to_string(), vec![]));
            }
        }

        // TS/JS check
        if (workspace.join("package.json").exists() || workspace.join("tsconfig.json").exists())
            && Self::has_binary("typescript-language-server")
        {
            return Some((
                LspLanguage::TypeScript,
                "typescript-language-server".to_string(),
                vec!["--stdio".to_string()],
            ));
        }

        // Go check
        if workspace.join("go.mod").exists() && Self::has_binary("gopls") {
            return Some((LspLanguage::Go, "gopls".to_string(), vec![]));
        }

        // Fallback checks based on file extensions in workspace
        if Self::has_binary("rust-analyzer") {
            return Some((LspLanguage::Rust, "rust-analyzer".to_string(), vec![]));
        }

        None
    }

    fn has_binary(name: &str) -> bool {
        Command::new("which")
            .arg(name)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }
}

pub fn encode_lsp_message(payload: &Value) -> Result<Vec<u8>> {
    let json_bytes = serde_json::to_vec(payload)?;
    let header = format!("Content-Length: {}\r\n\r\n", json_bytes.len());
    let mut out = header.into_bytes();
    out.extend(json_bytes);
    Ok(out)
}

pub fn decode_lsp_message<R: Read>(reader: &mut BufReader<R>) -> Result<Option<Value>> {
    let mut content_length: Option<usize> = None;

    loop {
        let mut line = String::new();
        let bytes_read = reader.read_line(&mut line)?;
        if bytes_read == 0 {
            return Ok(None);
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            // End of headers
            break;
        }

        if let Some(val) = trimmed.strip_prefix("Content-Length:") {
            content_length = val.trim().parse::<usize>().ok();
        }
    }

    let len = content_length.ok_or_else(|| anyhow!("Missing Content-Length in LSP header"))?;
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;

    let val: Value = serde_json::from_slice(&buf)?;
    Ok(Some(val))
}

pub struct LspClient {
    child: Child,
    request_id: u64,
}

impl LspClient {
    pub fn spawn(program: &str, args: &[String], workspace_dir: &Path) -> Result<Self> {
        let child = Command::new(program)
            .args(args)
            .current_dir(workspace_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        Ok(Self {
            child,
            request_id: 1,
        })
    }

    pub fn send_request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.request_id;
        self.request_id += 1;

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let data = encode_lsp_message(&req)?;
        if let Some(stdin) = self.child.stdin.as_mut() {
            stdin.write_all(&data)?;
            stdin.flush()?;
        }

        Ok(id)
    }

    pub fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        let notif = json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        let data = encode_lsp_message(&notif)?;
        if let Some(stdin) = self.child.stdin.as_mut() {
            stdin.write_all(&data)?;
            stdin.flush()?;
        }

        Ok(())
    }
}

impl Drop for LspClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

pub fn execute_lsp_query(
    workspace_dir: &Path,
    file_path: &Path,
    line: u32,
    col: u32,
    query_type: &str,
) -> Result<String> {
    let (lang, program, args) = LspDetector::detect(workspace_dir).ok_or_else(|| {
        anyhow!("No supported Language Server (rust-analyzer, pyright, gopls) found in PATH")
    })?;

    let abs_file = if file_path.is_absolute() {
        file_path.to_path_buf()
    } else {
        workspace_dir.join(file_path)
    };

    if !abs_file.exists() {
        return Err(anyhow!(
            "Target file does not exist: {}",
            abs_file.display()
        ));
    }

    let file_uri = format!("file://{}", abs_file.display());
    let file_content = std::fs::read_to_string(&abs_file)?;

    let mut client = LspClient::spawn(&program, &args, workspace_dir)?;

    // 1. Initialize
    let init_params = json!({
        "processId": std::process::id(),
        "rootUri": format!("file://{}", workspace_dir.display()),
        "capabilities": {
            "textDocument": {
                "hover": { "contentFormat": ["markdown", "plaintext"] },
                "definition": { "dynamicRegistration": false },
                "documentSymbol": { "dynamicRegistration": false }
            }
        }
    });
    let _init_id = client.send_request("initialize", init_params)?;

    let stdout = client
        .child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("Failed to capture LSP stdout"))?;
    let mut reader = BufReader::new(stdout);

    // Read initialize response
    let _ = decode_lsp_message(&mut reader)?;

    // 2. Initialized notification
    client.send_notification("initialized", json!({}))?;

    // 3. textDocument/didOpen
    let lang_id = match lang {
        LspLanguage::Rust => "rust",
        LspLanguage::Python => "python",
        LspLanguage::TypeScript => "typescript",
        LspLanguage::Go => "go",
    };
    client.send_notification(
        "textDocument/didOpen",
        json!({
            "textDocument": {
                "uri": file_uri,
                "languageId": lang_id,
                "version": 1,
                "text": file_content
            }
        }),
    )?;

    // 4. Query
    let query_id = match query_type {
        "hover" => client.send_request(
            "textDocument/hover",
            json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) }
            }),
        )?,
        "definition" => client.send_request(
            "textDocument/definition",
            json!({
                "textDocument": { "uri": file_uri },
                "position": { "line": line.saturating_sub(1), "character": col.saturating_sub(1) }
            }),
        )?,
        "symbols" => client.send_request(
            "textDocument/documentSymbol",
            json!({
                "textDocument": { "uri": file_uri }
            }),
        )?,
        _ => return Err(anyhow!("Unsupported LSP query type: {}", query_type)),
    };

    // 5. Read responses until we find matching query_id
    let mut response_result = None;
    for _ in 0..15 {
        if let Ok(Some(msg)) = decode_lsp_message(&mut reader) {
            if let Some(id) = msg.get("id").and_then(|v| v.as_u64()) {
                if id == query_id {
                    response_result = msg.get("result").cloned();
                    break;
                }
            }
        }
    }

    match response_result {
        Some(res) => {
            if res.is_null() {
                Ok(format!(
                    "LSP query `{}` returned no results at {}:{}:{}.",
                    query_type,
                    file_path.display(),
                    line,
                    col
                ))
            } else if query_type == "hover" {
                if let Some(contents) = res.get("contents") {
                    let text = if let Some(val) = contents.get("value").and_then(|v| v.as_str()) {
                        val.to_string()
                    } else {
                        contents.to_string()
                    };
                    Ok(format!("🎯 === LSP HOVER INFORMATION ===\n{}", text))
                } else {
                    Ok(res.to_string())
                }
            } else {
                Ok(serde_json::to_string_pretty(&res)?)
            }
        }
        None => Ok(format!(
            "LSP query `{}` timed out or returned no response.",
            query_type
        )),
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn test_encode_and_decode_lsp_message() {
        let payload = json!({
            "jsonrpc": "2.0",
            "id": 42,
            "method": "testMethod",
            "params": {"foo": "bar"}
        });

        let encoded = encode_lsp_message(&payload).unwrap();
        let mut reader = BufReader::new(&encoded[..]);
        let decoded = decode_lsp_message(&mut reader).unwrap().unwrap();

        assert_eq!(decoded["id"], 42);
        assert_eq!(decoded["method"], "testMethod");
        assert_eq!(decoded["params"]["foo"], "bar");
    }

    #[test]
    fn test_lsp_detector_has_binary_logic() {
        // 'which' or 'sh' should definitely be found on Linux
        assert!(LspDetector::has_binary("sh"));
        assert!(!LspDetector::has_binary("non_existent_fake_binary_xyz_123"));
    }
}
