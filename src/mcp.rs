use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpServerConfig {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: HashMap<String, String>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpConfig {
    pub mcp_servers: HashMap<String, McpServerConfig>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct McpTool {
    pub server_name: String,
    pub name: String,
    pub description: Option<String>,
    pub input_schema: serde_json::Value,
}

pub struct McpClient {
    pub server_name: String,
    #[allow(dead_code)]
    child: Child,
    stdin: Arc<Mutex<tokio::process::ChildStdin>>,
    reader: Arc<Mutex<BufReader<tokio::process::ChildStdout>>>,
    request_id: Mutex<u64>,
}

impl McpClient {
    pub async fn spawn(server_name: &str, config: &McpServerConfig) -> Result<Self> {
        let mut cmd = Command::new(&config.command);
        cmd.args(&config.args);
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        cmd.stdin(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::null());

        let mut child = cmd
            .spawn()
            .with_context(|| format!("Failed to spawn MCP server '{}'", server_name))?;
        let stdin = Arc::new(Mutex::new(
            child
                .stdin
                .take()
                .context("Failed to open stdin for MCP server")?,
        ));
        let stdout = child
            .stdout
            .take()
            .context("Failed to open stdout for MCP server")?;
        let reader = Arc::new(Mutex::new(BufReader::new(stdout)));

        let client = Self {
            server_name: server_name.to_string(),
            child,
            stdin,
            reader,
            request_id: Mutex::new(1),
        };

        client.initialize().await?;
        Ok(client)
    }

    async fn send_request(
        &self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value> {
        let mut id_guard = self.request_id.lock().await;
        let id = *id_guard;
        *id_guard += 1;

        let req = json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let line = format!("{}\n", serde_json::to_string(&req)?);
        let mut stdin_guard = self.stdin.lock().await;
        stdin_guard.write_all(line.as_bytes()).await?;
        stdin_guard.flush().await?;
        drop(stdin_guard);

        let mut reader_guard = self.reader.lock().await;
        let mut resp_line = String::new();
        reader_guard.read_line(&mut resp_line).await?;

        let resp: serde_json::Value = serde_json::from_str(&resp_line)?;
        if let Some(err) = resp.get("error") {
            return Err(anyhow::anyhow!("MCP Error: {}", err));
        }

        Ok(resp.get("result").cloned().unwrap_or(json!(null)))
    }

    pub async fn initialize(&self) -> Result<()> {
        let params = json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "orbit-cli",
                "version": "0.1.0"
            }
        });
        self.send_request("initialize", params).await?;
        let notif = json!({
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        });
        let line = format!("{}\n", serde_json::to_string(&notif)?);
        let mut stdin_guard = self.stdin.lock().await;
        stdin_guard.write_all(line.as_bytes()).await?;
        stdin_guard.flush().await?;
        Ok(())
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>> {
        let res = self.send_request("tools/list", json!({})).await?;
        let tools_arr = res
            .get("tools")
            .and_then(|t| t.as_array())
            .cloned()
            .unwrap_or_default();
        let mut tools = Vec::new();
        for t in tools_arr {
            let name = t["name"].as_str().unwrap_or("").to_string();
            let description = t["description"].as_str().map(|s| s.to_string());
            let input_schema = t.get("inputSchema").cloned().unwrap_or(json!({}));
            tools.push(McpTool {
                server_name: self.server_name.clone(),
                name,
                description,
                input_schema,
            });
        }
        Ok(tools)
    }

    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> Result<String> {
        let params = json!({
            "name": name,
            "arguments": arguments
        });
        let res = self.send_request("tools/call", params).await?;
        if let Some(content) = res.get("content").and_then(|c| c.as_array()) {
            let texts: Vec<String> = content
                .iter()
                .filter_map(|item| item["text"].as_str().map(|s| s.to_string()))
                .collect();
            return Ok(texts.join("\n"));
        }
        Ok(serde_json::to_string_pretty(&res)?)
    }
}

pub fn load_mcp_config() -> Option<McpConfig> {
    let config_dir = dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("openrouter");
    let mcp_path = config_dir.join("mcp.json");
    if mcp_path.exists() {
        if let Ok(data) = std::fs::read_to_string(&mcp_path) {
            return serde_json::from_str(&data).ok();
        }
    }
    None
}
