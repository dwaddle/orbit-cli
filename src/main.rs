use anyhow::Result;
use colored::*;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use regex::Regex;
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

pub mod context;
pub mod git_context;
mod mcp;
pub mod outline;
pub mod rules;
pub mod search_index;
mod subagent;
pub mod tui;

use mcp::{load_mcp_config, McpClient, McpTool};
use subagent::{SubagentManager, SubagentStatus};

const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4.5";
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const OPENROUTER_COMPLETIONS_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub api_key: String,
    pub active_model: String,
    pub tools_enabled: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            active_model: DEFAULT_MODEL.to_string(),
            tools_enabled: true,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
pub struct OpenRouterModel {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
}

#[derive(Deserialize, Debug)]
struct ModelsResponse {
    data: Vec<OpenRouterModel>,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Message {
    pub role: String,
    pub content: String,
}

pub struct AgentState {
    pub config: AppConfig,
    pub config_path: PathBuf,
    pub history: Vec<Message>,
    pub available_models: Vec<OpenRouterModel>,
    pub client: reqwest::Client,
    pub subagent_mgr: SubagentManager,
    pub mcp_tools: Vec<McpTool>,
    pub mcp_clients: Vec<McpClient>,
    pub project_rules: Option<rules::ProjectRules>,
    pub code_index: search_index::CodeSearchIndex,
    pub workspace_dir: PathBuf,
}

impl AgentState {
    fn new() -> Result<Self> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("openrouter");
        fs::create_dir_all(&config_dir)?;
        let config_path = config_dir.join("config.json");

        let mut config = if config_path.exists() {
            let data = fs::read_to_string(&config_path)?;
            serde_json::from_str(&data).unwrap_or_default()
        } else {
            AppConfig::default()
        };

        if config.api_key.is_empty() || config.api_key.len() < 60 {
            let env_file = config_dir.join("env");
            if env_file.exists() {
                if let Ok(env_content) = fs::read_to_string(&env_file) {
                    for line in env_content.lines() {
                        if line.starts_with("OPENROUTER_API_KEY=") {
                            config.api_key = line
                                .trim_start_matches("OPENROUTER_API_KEY=")
                                .trim()
                                .to_string();
                        }
                    }
                }
            }
            if config.api_key.is_empty() || config.api_key.len() < 60 {
                if let Ok(key) = std::env::var("OPENROUTER_API_KEY") {
                    config.api_key = key.trim().to_string();
                }
            }
        }

        let workspace_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_rules = rules::ProjectRules::discover(&workspace_dir);
        let code_index = search_index::CodeSearchIndex::build(&workspace_dir);

        Ok(Self {
            config,
            config_path,
            history: Vec::new(),
            available_models: Vec::new(),
            client: reqwest::Client::new(),
            subagent_mgr: SubagentManager::new(),
            mcp_tools: Vec::new(),
            mcp_clients: Vec::new(),
            project_rules,
            code_index,
            workspace_dir,
        })
    }

    fn save_config(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.config)?;
        fs::write(&self.config_path, data)?;
        Ok(())
    }

    async fn fetch_models(&mut self) -> Result<()> {
        let resp = self
            .client
            .get(OPENROUTER_MODELS_URL)
            .send()
            .await?
            .json::<ModelsResponse>()
            .await?;
        self.available_models = resp.data;
        Ok(())
    }

    async fn init_mcp(&mut self) {
        if let Some(cfg) = load_mcp_config() {
            println!("Initializing MCP servers from mcp.json...");
            for (name, server_cfg) in &cfg.mcp_servers {
                match McpClient::spawn(name, server_cfg).await {
                    Ok(client) => match client.list_tools().await {
                        Ok(tools) => {
                            println!(
                                "  ✓ Registered {} MCP tools from server '{}'",
                                tools.len(),
                                name
                            );
                            self.mcp_tools.extend(tools);
                            self.mcp_clients.push(client);
                        }
                        Err(e) => {
                            println!("  ⚠️ Failed to list tools for server '{}': {}", name, e)
                        }
                    },
                    Err(e) => println!("  ⚠️ Failed to start MCP server '{}': {}", name, e),
                }
            }
        }
    }

    pub fn system_prompt(&self) -> String {
        let base = "You are Orbit, an autonomous AI coding assistant paired with a developer in an interactive CLI session.\n\
        You have direct system execution tools, codebase intelligence, and subagent invocation capabilities.\n";

        if !self.config.tools_enabled {
            return base.to_string();
        }

        let mut prompt = format!(
            "{}\n\
            ## AVAILABLE TOOLS:\n\
            To run a tool, emit a block formatted EXACTLY like this:\n\n\
            ```tool:run_command\n\
            <command to execute in bash>\n\
            ```\n\n\
            ```tool:view_file\n\
            <path>\n\
            ```\n\n\
            ```tool:write_file\n\
            <path>\n\
            <content>\n\
            ```\n\n\
            ```tool:code_search\n\
            query: <terms or symbol to search across codebase>\n\
            limit: <optional max results, default 5>\n\
            ```\n\n\
            ```tool:code_outline\n\
            path: <optional directory or file path>\n\
            ```\n\n\
            ```tool:git_context\n\
            action: log | blame | diff | search\n\
            path: <optional file path>\n\
            query: <optional pickaxe query or commit query>\n\
            range: <optional start,end lines for blame>\n\
            limit: <optional max commits, default 15>\n\
            ```\n\n\
            ```tool:invoke_subagent\n\
            role: <role name>\n\
            model: <openrouter model slug>\n\
            prompt: <task description>\n\
            ```\n\n",
            base
        );

        if let Some(rules) = &self.project_rules {
            prompt.push_str(&rules.format_for_prompt());
        }

        if !self.mcp_tools.is_empty() {
            prompt.push_str("## MCP TOOLS:\n");
            for t in &self.mcp_tools {
                prompt.push_str(&format!(
                    "• MCP Tool [{}/{}]: {}\n",
                    t.server_name,
                    t.name,
                    t.description.as_deref().unwrap_or("No description")
                ));
            }
            prompt.push_str("\nTo call an MCP tool, use:\n```tool:mcp\nserver: <server_name>\ntool: <tool_name>\narguments: <json_object>\n```\n\n");
        }

        prompt.push_str(
            "Keep all responses direct, technical, direct, and concise in GitHub markdown.",
        );
        prompt
    }

    fn list_models(&self, filter: Option<&str>) {
        println!("{}", "=== 🤖 OPENROUTER MODELS ===".bold().cyan());
        let search = filter.unwrap_or("").to_lowercase();

        let filtered: Vec<&OpenRouterModel> = self
            .available_models
            .iter()
            .filter(|m| {
                search.is_empty()
                    || m.id.to_lowercase().contains(&search)
                    || m.name.to_lowercase().contains(&search)
            })
            .collect();

        if filtered.is_empty() {
            println!("{}", "No models found matching search query.".yellow());
            return;
        }

        println!(
            "Active Model: {}\n",
            self.config.active_model.bold().green()
        );
        println!("{:<4} {:<45} {:<15} NAME", "ID", "MODEL SLUG", "CONTEXT");
        println!("{}", "-".repeat(85));

        for (idx, m) in filtered.iter().take(30).enumerate() {
            let ctx_str = m
                .context_length
                .map(|c| format!("{}k", c / 1024))
                .unwrap_or_else(|| "N/A".to_string());
            let marker = if m.id == self.config.active_model {
                "*"
            } else {
                " "
            };
            println!(
                "{}{:<3} {:<45} {:<15} {}",
                marker.bold().green(),
                idx + 1,
                m.id.cyan(),
                ctx_str.yellow(),
                m.name
            );
        }

        if filtered.len() > 30 {
            println!(
                "\n... and {} more models. Use `/model <search>` to filter.",
                filtered.len() - 30
            );
        }
    }

    fn select_model(&mut self, arg: &str) -> bool {
        let arg = arg.trim();
        if let Ok(idx) = arg.parse::<usize>() {
            if idx > 0 && idx <= self.available_models.len() {
                self.config.active_model = self.available_models[idx - 1].id.clone();
                let _ = self.save_config();
                println!(
                    "✅ Switched model to: {}",
                    self.config.active_model.bold().green()
                );
                return true;
            }
        }
        if let Some(m) = self
            .available_models
            .iter()
            .find(|m| m.id == arg || m.id.ends_with(arg))
        {
            self.config.active_model = m.id.clone();
            let _ = self.save_config();
            println!(
                "✅ Switched model to: {}",
                self.config.active_model.bold().green()
            );
            return true;
        }
        self.config.active_model = arg.to_string();
        let _ = self.save_config();
        println!(
            "✅ Set custom model slug: {}",
            self.config.active_model.bold().green()
        );
        true
    }
}

async fn execute_tool(tool_name: &str, body: &str, state: &AgentState) -> String {
    println!(
        "\n{}",
        format!("⚙️ Executing Tool: {}", tool_name).bold().yellow()
    );
    match tool_name {
        "run_command" => {
            let cmd = body.trim();
            println!("$ {}", cmd.bold().white());
            let output = Command::new("bash").arg("-c").arg(cmd).output();
            match output {
                Ok(out) => {
                    let stdout = String::from_utf8_lossy(&out.stdout);
                    let stderr = String::from_utf8_lossy(&out.stderr);
                    let code = out.status.code().unwrap_or(-1);
                    format!(
                        "Exit Code: {}\nSTDOUT:\n{}\nSTDERR:\n{}",
                        code, stdout, stderr
                    )
                }
                Err(e) => format!("Failed to execute command: {}", e),
            }
        }
        "view_file" => {
            let path = body.trim();
            match fs::read_to_string(path) {
                Ok(content) => {
                    let lines: Vec<&str> = content.lines().collect();
                    let preview = if lines.len() > 200 {
                        format!(
                            "{}\n... (truncated, total {} lines)",
                            lines[..200].join("\n"),
                            lines.len()
                        )
                    } else {
                        content
                    };
                    format!("File Content ({}):\n{}", path, preview)
                }
                Err(e) => format!("Failed to read file {}: {}", path, e),
            }
        }
        "write_file" => {
            let mut parts = body.trim_start().splitn(2, '\n');
            let path = parts.next().unwrap_or("").trim();
            let content = parts.next().unwrap_or("");
            if path.is_empty() {
                return "Error: Empty path specified for write_file".to_string();
            }
            match fs::write(path, content) {
                Ok(_) => format!("Successfully wrote {} bytes to {}", content.len(), path),
                Err(e) => format!("Failed to write file {}: {}", path, e),
            }
        }
        "code_search" => search_index::execute_code_search_tool(&state.code_index, body),
        "code_outline" => {
            let mut target = "";
            for line in body.lines() {
                let trimmed = line.trim();
                if let Some(val) = trimmed.strip_prefix("path:") {
                    target = val.trim();
                }
            }
            if target.is_empty() {
                target = body.trim();
            }
            let target_path = if target.is_empty() {
                &state.workspace_dir
            } else {
                Path::new(target)
            };
            outline::generate_codebase_outline(target_path, 2000)
        }
        "git_context" => git_context::execute_git_tool(&state.workspace_dir, body),
        "invoke_subagent" => {
            let mut role = "Assistant".to_string();
            let mut model = state.config.active_model.clone();
            let mut prompt = "".to_string();

            for line in body.lines() {
                if let Some(val) = line.strip_prefix("role:") {
                    role = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("model:") {
                    model = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("prompt:") {
                    prompt = val.trim().to_string();
                }
            }

            if prompt.is_empty() {
                return "Error: Missing prompt for subagent".to_string();
            }

            let task_id = state
                .subagent_mgr
                .spawn_subagent(
                    role.clone(),
                    model.clone(),
                    prompt.clone(),
                    state.config.api_key.clone(),
                )
                .await;

            format!("Subagent Task #{} spawned in background (Role: {}, Model: {}). Use /subagents to check progress.", task_id, role, model)
        }
        "mcp" => {
            let mut server = "".to_string();
            let mut tool = "".to_string();
            let mut args_json = json!({});

            for line in body.lines() {
                if let Some(val) = line.strip_prefix("server:") {
                    server = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("tool:") {
                    tool = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("arguments:") {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(val.trim()) {
                        args_json = v;
                    }
                }
            }

            if let Some(mcp_client) = state.mcp_clients.iter().find(|c| c.server_name == server) {
                match mcp_client.call_tool(&tool, args_json).await {
                    Ok(out) => out,
                    Err(e) => format!("MCP Error calling {}/{}: {}", server, tool, e),
                }
            } else {
                format!("Error: MCP Server '{}' not found", server)
            }
        }
        _ => format!("Unknown tool: {}", tool_name),
    }
}

pub async fn stream_completion(state: &AgentState) -> Result<String> {
    let mut req_messages = vec![json!({
        "role": "system",
        "content": state.system_prompt()
    })];

    for m in &state.history {
        req_messages.push(json!({
            "role": m.role,
            "content": m.content
        }));
    }

    let payload = json!({
        "model": state.config.active_model,
        "messages": req_messages,
        "temperature": 0.2,
        "stream": true
    });

    let resp = state
        .client
        .post(OPENROUTER_COMPLETIONS_URL)
        .header("Authorization", format!("Bearer {}", state.config.api_key))
        .header("Content-Type", "application/json")
        .header("HTTP-Referer", "https://github.com/dwaddle/orbit-cli")
        .header("X-Title", "Orbit-CLI")
        .json(&payload)
        .send()
        .await?;

    if !resp.status().is_success() {
        let err_text = resp.text().await?;
        return Err(anyhow::anyhow!("OpenRouter API Error: {}", err_text));
    }

    let mut stream = resp.bytes_stream().eventsource();
    let mut full_text = String::new();

    print!("\n{} ", "🤖".bold());
    io::stdout().flush()?;

    while let Some(event) = stream.next().await {
        match event {
            Ok(evt) => {
                if evt.data == "[DONE]" {
                    break;
                }
                if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(&evt.data) {
                    if let Some(content) = chunk["choices"][0]["delta"]["content"].as_str() {
                        print!("{}", content);
                        io::stdout().flush()?;
                        full_text.push_str(content);
                    }
                }
            }
            Err(_) => break,
        }
    }
    println!("\n");
    Ok(full_text)
}

#[tokio::main]
async fn main() -> Result<()> {
    println!(
        "{}",
        "=========================================================".cyan()
    );
    println!(
        "{}",
        "   🛰️  ORBIT-CLI v0.2: Autonomous OpenRouter Agent (Rust)   "
            .bold()
            .cyan()
    );
    println!(
        "{}",
        "   Supports: Context Pruning, Code Search, Outline, Git & MCP ".dimmed()
    );
    println!(
        "{}",
        "=========================================================".cyan()
    );

    let mut state = AgentState::new()?;

    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--tui" || a == "-t") {
        state.init_mcp().await;
        if state.config.api_key.is_empty() {
            println!("{}", "⚠️ OpenRouter API Key not configured.".yellow());
            print!("Enter your OpenRouter API Key (sk-or-v1-...): ");
            io::stdout().flush()?;
            let mut input_key = String::new();
            io::stdin().read_line(&mut input_key)?;
            state.config.api_key = input_key.trim().to_string();
            state.save_config()?;
        }
        let _ = state.fetch_models().await;
        return tui::run_tui(&mut state).await;
    }

    state.init_mcp().await;

    if let Some(rules) = &state.project_rules {
        println!(
            "Project Rules: Loaded from {}",
            rules.path.display().to_string().bold().cyan()
        );
    }
    println!(
        "Code Search: {} workspace files indexed.",
        state.code_index.documents.len().to_string().cyan()
    );

    if state.config.api_key.is_empty() {
        println!("{}", "⚠️ OpenRouter API Key not configured.".yellow());
        print!("Enter your OpenRouter API Key (sk-or-v1-...): ");
        io::stdout().flush()?;
        let mut input_key = String::new();
        io::stdin().read_line(&mut input_key)?;
        state.config.api_key = input_key.trim().to_string();
        state.save_config()?;
    }

    print!("Fetching available models from OpenRouter... ");
    io::stdout().flush()?;
    match state.fetch_models().await {
        Ok(_) => println!(
            "Loaded {} models.",
            state.available_models.len().to_string().green()
        ),
        Err(e) => println!("Warning (offline/cache): {}", e),
    }

    println!("Active Model: {}", state.config.active_model.bold().green());
    println!(
        "Tools Status: {}",
        if state.config.tools_enabled {
            "ENABLED".bold().green()
        } else {
            "DISABLED".yellow()
        }
    );
    println!("Type `/help` for commands or `/model` to view/select models.\n");

    let mut rl = DefaultEditor::new()?;
    let _ = rl.load_history(&state.config_path.with_file_name("history.txt"));

    let tool_re = Regex::new(
        r"```tool:(run_command|view_file|write_file|invoke_subagent|mcp|code_search|code_outline|git_context)\n([\s\S]*?)```",
    )?;

    loop {
        let prompt = format!("{} > ", "orbit".bold().magenta());
        let readline = rl.readline(&prompt);

        match readline {
            Ok(line) => {
                let input = line.trim();
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(input);

                if input.starts_with('/') {
                    let mut parts = input.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let arg = parts.next().unwrap_or("");

                    match cmd {
                        "/exit" | "/quit" => {
                            println!("Bye!");
                            break;
                        }
                        "/help" => {
                            println!("{}", "=== ORBIT-CLI COMMANDS ===".bold().cyan());
                            println!(
                                "  /model                 - List top models & active selection"
                            );
                            println!("  /model <search|id|num> - Filter or switch active OpenRouter model");
                            println!(
                                "  /subagents             - List active & completed subagent tasks"
                            );
                            println!("  /mcp                   - List loaded MCP servers and available tools");
                            println!("  /search <query>        - Search local codebase symbols and full-text");
                            println!("  /outline [path]        - Generate codebase symbol map and outline");
                            println!("  /git <log|blame|diff>  - Inspect git commit history, blame or diff");
                            println!("  /rules                 - View or verify active project .orbitrules");
                            println!(
                                "  /context               - View context token usage and budget"
                            );
                            println!("  /prune                 - Manually compress large tool outputs & compact turns");
                            println!("  /file <path>           - Attach file content into context");
                            println!(
                                "  /tools                 - Toggle auto tool execution ON/OFF"
                            );
                            println!("  /tui                   - Launch Ratatui fullscreen split TUI mode");
                            println!("  /clear                 - Clear chat history");
                            println!("  /key <sk-or-v1-...>    - Update OpenRouter API Key");
                            println!("  /help                  - Show this help menu");
                            println!("  /exit                  - Exit application");
                            continue;
                        }
                        "/git" => {
                            if !git_context::is_git_repo(&state.workspace_dir) {
                                println!(
                                    "Error: Workspace `{}` is not a git repository.",
                                    state.workspace_dir.display()
                                );
                                continue;
                            }
                            let git_parts: Vec<&str> = arg.split_whitespace().collect();
                            let action = git_parts.first().copied().unwrap_or("log");
                            match action {
                                "log" => {
                                    let query = if git_parts.len() > 1 {
                                        Some(git_parts[1..].join(" "))
                                    } else {
                                        None
                                    };
                                    match git_context::get_git_log(
                                        &state.workspace_dir,
                                        None,
                                        query.as_deref(),
                                        15,
                                    ) {
                                        Ok(out) => println!("{}", out),
                                        Err(e) => println!("Error: {}", e),
                                    }
                                }
                                "blame" => {
                                    if git_parts.len() < 2 {
                                        println!("Usage: /git blame <file_path> [start,end]");
                                    } else {
                                        let file = git_parts[1];
                                        let range = if git_parts.len() > 2 {
                                            let r_parts: Vec<&str> =
                                                git_parts[2].split(',').collect();
                                            if r_parts.len() == 2 {
                                                match (
                                                    r_parts[0].parse::<usize>(),
                                                    r_parts[1].parse::<usize>(),
                                                ) {
                                                    (Ok(s), Ok(e)) => Some((s, e)),
                                                    _ => None,
                                                }
                                            } else {
                                                None
                                            }
                                        } else {
                                            None
                                        };
                                        match git_context::get_git_blame(
                                            &state.workspace_dir,
                                            file,
                                            range,
                                        ) {
                                            Ok(out) => println!("{}", out),
                                            Err(e) => println!("Error: {}", e),
                                        }
                                    }
                                }
                                "diff" => {
                                    match git_context::get_git_diff(
                                        &state.workspace_dir,
                                        None,
                                        false,
                                    ) {
                                        Ok(out) => println!("{}", out),
                                        Err(e) => println!("Error: {}", e),
                                    }
                                }
                                "search" => {
                                    if git_parts.len() < 2 {
                                        println!("Usage: /git search <query>");
                                    } else {
                                        let q = git_parts[1..].join(" ");
                                        match git_context::get_git_log(
                                            &state.workspace_dir,
                                            None,
                                            Some(&q),
                                            15,
                                        ) {
                                            Ok(out) => println!("{}", out),
                                            Err(e) => println!("Error: {}", e),
                                        }
                                    }
                                }
                                _ => {
                                    println!("Usage: /git <log|blame|diff|search> [args]");
                                }
                            }
                            continue;
                        }
                        "/search" => {
                            if arg.is_empty() {
                                println!("Usage: /search <query>");
                            } else {
                                let results = state.code_index.search(arg, 8);
                                println!("{}", state.code_index.format_results(arg, &results));
                            }
                            continue;
                        }
                        "/outline" => {
                            let target = if arg.is_empty() {
                                &state.workspace_dir
                            } else {
                                Path::new(arg)
                            };
                            let outline_text = outline::generate_codebase_outline(target, 2000);
                            println!("{}", outline_text);
                            continue;
                        }
                        "/rules" => {
                            match &state.project_rules {
                                Some(r) => {
                                    println!(
                                        "=== 📜 PROJECT RULES ({}) ===\n{}",
                                        r.path.display().to_string().cyan(),
                                        r.content
                                    );
                                }
                                None => {
                                    println!(
                                        "No .orbitrules or AGENTS.md found in workspace hierarchy."
                                    );
                                }
                            }
                            continue;
                        }
                        "/context" => {
                            let active_limit = state
                                .available_models
                                .iter()
                                .find(|m| m.id == state.config.active_model)
                                .and_then(|m| m.context_length)
                                .map(|c| c as usize)
                                .unwrap_or(context::DEFAULT_CONTEXT_LIMIT);
                            println!(
                                "{}",
                                context::format_context_report(
                                    &state.history,
                                    &state.system_prompt(),
                                    active_limit
                                )
                            );
                            continue;
                        }
                        "/prune" => {
                            let active_limit = state
                                .available_models
                                .iter()
                                .find(|m| m.id == state.config.active_model)
                                .and_then(|m| m.context_length)
                                .map(|c| c as usize)
                                .unwrap_or(context::DEFAULT_CONTEXT_LIMIT);
                            let sys_prompt = state.system_prompt();
                            let res = context::auto_prune_context(
                                &mut state.history,
                                &sys_prompt,
                                active_limit,
                                0.0,
                            );
                            if res.pruned {
                                println!(
                                    "{}",
                                    format!(
                                        "✅ Context pruned: {} tool outputs compressed, {} turns compacted (tokens: {} -> {}).",
                                        res.tool_outputs_compressed,
                                        res.turns_compacted,
                                        res.tokens_before,
                                        res.tokens_after
                                    )
                                    .green()
                                );
                            } else {
                                println!("Context is already compact. No items required pruning.");
                            }
                            continue;
                        }
                        "/subagents" => {
                            let tasks = state.subagent_mgr.list_subagents().await;
                            println!("{}", "=== 🤖 SUBAGENT TASKS ===".bold().cyan());
                            if tasks.is_empty() {
                                println!("No subagents spawned yet.");
                            } else {
                                for t in tasks {
                                    let status_str = match &t.status {
                                        SubagentStatus::Running => "RUNNING".bold().yellow(),
                                        SubagentStatus::Completed => "COMPLETED".bold().green(),
                                        SubagentStatus::Failed(err) => {
                                            format!("FAILED ({})", err).bold().red()
                                        }
                                    };
                                    println!(
                                        "• Subagent #{} [{}] Status: {}",
                                        t.id,
                                        t.role.bold(),
                                        status_str
                                    );
                                    println!("  Model: {} | Prompt: {}", t.model, t.prompt);
                                    if let Some(res) = &t.result {
                                        let lines: Vec<&str> = res.lines().collect();
                                        println!(
                                            "  Result: {} ({} lines)",
                                            lines.first().unwrap_or(&""),
                                            lines.len()
                                        );
                                    }
                                }
                            }
                            continue;
                        }
                        "/mcp" => {
                            println!("{}", "=== 🔌 LOADED MCP TOOLS ===".bold().cyan());
                            if state.mcp_tools.is_empty() {
                                println!("No MCP servers loaded. Add config to ~/.config/openrouter/mcp.json");
                            } else {
                                for t in &state.mcp_tools {
                                    println!(
                                        "• [{}/{}] {}",
                                        t.server_name.bold().green(),
                                        t.name.cyan(),
                                        t.description.as_deref().unwrap_or("")
                                    );
                                }
                            }
                            continue;
                        }
                        "/model" => {
                            if arg.is_empty() {
                                state.list_models(None);
                            } else if arg.chars().all(|c| c.is_numeric()) || arg.contains('/') {
                                state.select_model(arg);
                            } else {
                                state.list_models(Some(arg));
                            }
                            continue;
                        }
                        "/tools" => {
                            state.config.tools_enabled = !state.config.tools_enabled;
                            let _ = state.save_config();
                            println!(
                                "Tools auto-execution set to: {}",
                                if state.config.tools_enabled {
                                    "ENABLED".bold().green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            continue;
                        }
                        "/tui" => {
                            println!("Launching Ratatui fullscreen TUI mode...");
                            if let Err(e) = tui::run_tui(&mut state).await {
                                println!("TUI error: {}", e);
                            }
                            continue;
                        }
                        "/clear" => {
                            state.history.clear();
                            println!("✅ Conversation history cleared.");
                            continue;
                        }
                        "/key" => {
                            if !arg.is_empty() {
                                state.config.api_key = arg.to_string();
                                let _ = state.save_config();
                                println!("✅ OpenRouter API key updated.");
                            }
                            continue;
                        }
                        "/file" => {
                            if !arg.is_empty() {
                                if let Ok(content) = fs::read_to_string(arg) {
                                    state.history.push(Message {
                                        role: "user".to_string(),
                                        content: format!("[ATTACHED FILE: {}]\n{}", arg, content),
                                    });
                                    println!("✅ Attached file context ({})", arg.bold().green());
                                }
                            }
                            continue;
                        }
                        _ => {
                            println!(
                                "Unknown command {}. Type `/help` for options.",
                                cmd.yellow()
                            );
                            continue;
                        }
                    }
                }

                state.history.push(Message {
                    role: "user".to_string(),
                    content: input.to_string(),
                });

                loop {
                    // Check and auto-prune context at 80% threshold
                    let active_limit = state
                        .available_models
                        .iter()
                        .find(|m| m.id == state.config.active_model)
                        .and_then(|m| m.context_length)
                        .map(|c| c as usize)
                        .unwrap_or(context::DEFAULT_CONTEXT_LIMIT);

                    let sys_prompt = state.system_prompt();
                    let prune_res = context::auto_prune_context(
                        &mut state.history,
                        &sys_prompt,
                        active_limit,
                        context::DEFAULT_PRUNE_THRESHOLD,
                    );
                    if prune_res.pruned {
                        println!(
                            "{}",
                            format!(
                                "🧹 Auto-pruned context: {} tool outputs compressed, {} turns compacted (tokens: {} -> {}).",
                                prune_res.tool_outputs_compressed,
                                prune_res.turns_compacted,
                                prune_res.tokens_before,
                                prune_res.tokens_after
                            )
                            .cyan()
                        );
                    }

                    match stream_completion(&state).await {
                        Ok(response_text) => {
                            state.history.push(Message {
                                role: "assistant".to_string(),
                                content: response_text.clone(),
                            });

                            if state.config.tools_enabled {
                                let mut tool_executed = false;
                                for cap in tool_re.captures_iter(&response_text) {
                                    let tool_name = &cap[1];
                                    let tool_body = &cap[2];

                                    let tool_result =
                                        execute_tool(tool_name, tool_body, &state).await;
                                    println!("{}\n{}", "Result:".dimmed(), tool_result);

                                    state.history.push(Message {
                                        role: "user".to_string(),
                                        content: format!(
                                            "[TOOL OUTPUT ({})]:\n{}",
                                            tool_name, tool_result
                                        ),
                                    });
                                    tool_executed = true;
                                }

                                if tool_executed {
                                    println!(
                                        "{}",
                                        "🔄 Feeding tool execution results back to model...".cyan()
                                    );
                                    continue;
                                }
                            }
                            break;
                        }
                        Err(e) => {
                            println!("❌ Error: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                println!("Bye!");
                break;
            }
            Err(err) => {
                println!("Error: {:?}", err);
                break;
            }
        }
    }

    let _ = rl.save_history(&state.config_path.with_file_name("history.txt"));
    Ok(())
}
