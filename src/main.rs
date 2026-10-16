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
use std::sync::{Arc, Mutex};

pub mod checkpoint;
pub mod compile_guard;
pub mod context;
pub mod git_context;
pub mod highlight;
mod mcp;
pub mod outline;
pub mod patch;
pub mod review;
pub mod routing;
pub mod rules;
pub mod search_index;
pub mod subagent;
pub mod tdd;
pub mod tui;
pub mod watcher;

use mcp::{load_mcp_config, McpClient, McpTool};
use subagent::{SubagentManager, SubagentStatus};

const DEFAULT_MODEL: &str = "anthropic/claude-sonnet-4.5";
const OPENROUTER_MODELS_URL: &str = "https://openrouter.ai/api/v1/models";
const OPENROUTER_COMPLETIONS_URL: &str = "https://openrouter.ai/api/v1/chat/completions";

fn default_true() -> bool {
    true
}

fn default_fast_model() -> String {
    "google/gemini-2.5-flash".to_string()
}

fn default_heavy_model() -> String {
    DEFAULT_MODEL.to_string()
}

fn default_fallback_models() -> Vec<String> {
    vec![
        "anthropic/claude-sonnet-4.5".to_string(),
        "openai/gpt-4o".to_string(),
        "deepseek/deepseek-r1".to_string(),
        "google/gemini-2.5-flash".to_string(),
    ]
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct AppConfig {
    pub api_key: String,
    pub active_model: String,
    pub tools_enabled: bool,
    #[serde(default = "default_true")]
    pub prompt_caching: bool,
    #[serde(default)]
    pub cascading_enabled: bool,
    #[serde(default = "default_fast_model")]
    pub fast_model: String,
    #[serde(default = "default_heavy_model")]
    pub heavy_model: String,
    #[serde(default = "default_fallback_models")]
    pub fallback_models: Vec<String>,
    #[serde(default)]
    pub adversarial_review_enabled: bool,
    #[serde(default = "default_true")]
    pub compile_guard_enabled: bool,
    #[serde(default = "default_true")]
    pub self_healing_enabled: bool,
    #[serde(default = "default_true")]
    pub syntax_highlighting: bool,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
            active_model: DEFAULT_MODEL.to_string(),
            tools_enabled: true,
            prompt_caching: true,
            cascading_enabled: false,
            fast_model: default_fast_model(),
            heavy_model: default_heavy_model(),
            fallback_models: default_fallback_models(),
            adversarial_review_enabled: false,
            compile_guard_enabled: true,
            self_healing_enabled: true,
            syntax_highlighting: true,
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
    pub code_index: Mutex<Option<Arc<search_index::CodeSearchIndex>>>,
    pub workspace_dir: PathBuf,
    pub active_watcher: Arc<Mutex<Option<Arc<watcher::OrbitWatcher>>>>,
    pub checkpoint_mgr: checkpoint::CheckpointManager,
    pub compile_guard: compile_guard::CompileGuard,
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

        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .connect_timeout(std::time::Duration::from_secs(5))
            .build()?;

        let workspace_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let project_rules = rules::ProjectRules::discover(&workspace_dir);
        let checkpoint_mgr = checkpoint::CheckpointManager::new();
        let compile_guard = compile_guard::CompileGuard::new(&workspace_dir);

        Ok(Self {
            config,
            config_path,
            history: Vec::new(),
            available_models: Vec::new(),
            client,
            subagent_mgr: SubagentManager::new(),
            mcp_tools: Vec::new(),
            mcp_clients: Vec::new(),
            project_rules,
            code_index: Mutex::new(None),
            workspace_dir,
            active_watcher: Arc::new(Mutex::new(None)),
            checkpoint_mgr,
            compile_guard,
        })
    }

    pub fn get_code_index(&self) -> Arc<search_index::CodeSearchIndex> {
        let mut guard = self.code_index.lock().unwrap();
        if guard.is_none() {
            println!("{}", "Building codebase search index...".dimmed());
            let idx = search_index::CodeSearchIndex::build(&self.workspace_dir);
            println!(
                "Code Search: {} workspace files indexed.",
                idx.documents.len().to_string().cyan()
            );
            *guard = Some(Arc::new(idx));
        }
        guard.as_ref().unwrap().clone()
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
            ```\n\n\
            ```tool:adversarial_review\n\
            target: <file path or target name>\n\
            context: <spec or focus requirements>\n\
            content:\n\
            <code or diff to review>\n\
            ```\n\n\
            ```tool:run_dag\n\
            audit <target> OR feature <feature spec>\n\
            ```\n\n\
            ```tool:apply_patch\n\
            path: <file path>\n\
            <<<<<<< SEARCH\n\
            <exact code to find>\n\
            =======\n\
            <replacement code>\n\
            >>>>>>> REPLACE\n\
            ```\n\n\
            ```tool:tdd_cycle\n\
            <feature specification>\n\
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

            // Snapshot before write
            state
                .checkpoint_mgr
                .record_before_modify(Path::new(path), "write_file");

            match fs::write(path, content) {
                Ok(_) => {
                    let mut res = format!("Successfully wrote {} bytes to {}", content.len(), path);

                    // Compile Guard check
                    if state.config.compile_guard_enabled {
                        let comp = state.compile_guard.check();
                        println!("{}", compile_guard::format_compile_banner(&comp));
                        res.push_str(&format!(
                            "\n\n{}",
                            compile_guard::format_compile_banner(&comp)
                        ));
                        if !comp.success && state.config.self_healing_enabled {
                            println!(
                                "{}",
                                "🩹 Compile Guard detected error. Self-healing triggered..."
                                    .yellow()
                            );
                            res.push_str("\n\n[SELF-HEALING REQUIRED]: Compilation failed. Please emit a corrected ```tool:write_file or ```tool:apply_patch immediately to resolve these compiler errors.");
                        }
                    }

                    if state.config.adversarial_review_enabled && !state.config.api_key.is_empty() {
                        println!("{}", "🛡️ Running automatic adversarial review...".dimmed());
                        match review::AdversarialReviewer::review_code(
                            path,
                            content,
                            "Automatic post-write safety and regression review",
                            &state.config.api_key,
                        )
                        .await
                        {
                            Ok(report) => {
                                println!("{}", review::format_review_banner(&report));
                                res.push_str(&format!(
                                    "\n\n[ADVERSARIAL REVIEW: {} (Score: {}/100)]\n{}",
                                    report.verdict.badge(),
                                    report.score,
                                    report.raw_response
                                ));
                            }
                            Err(e) => {
                                println!("Adversarial review warning: {}", e);
                            }
                        }
                    }
                    res
                }
                Err(e) => format!("Failed to write file {}: {}", path, e),
            }
        }
        "apply_patch" => {
            let mut parts = body.trim_start().splitn(2, '\n');
            let first_line = parts.next().unwrap_or("").trim();
            let (target_path, patch_payload) = if first_line.starts_with("path:") {
                let p = first_line.strip_prefix("path:").unwrap().trim();
                (p.to_string(), parts.next().unwrap_or("").to_string())
            } else if first_line.starts_with("file:") {
                let p = first_line.strip_prefix("file:").unwrap().trim();
                (p.to_string(), parts.next().unwrap_or("").to_string())
            } else {
                ("".to_string(), body.to_string())
            };

            // Snapshot target file before patch
            if !target_path.is_empty() {
                state
                    .checkpoint_mgr
                    .record_before_modify(Path::new(&target_path), "apply_patch");
            }

            match patch::apply_patch_to_file(&target_path, &patch_payload) {
                Ok(patch_res) => {
                    let mut res = format!(
                        "Successfully applied {} search/replace block(s) to {} (bytes: {} -> {})",
                        patch_res.blocks_applied,
                        patch_res.path,
                        patch_res.original_len,
                        patch_res.new_len
                    );

                    // Compile Guard check
                    if state.config.compile_guard_enabled {
                        let comp = state.compile_guard.check();
                        println!("{}", compile_guard::format_compile_banner(&comp));
                        res.push_str(&format!(
                            "\n\n{}",
                            compile_guard::format_compile_banner(&comp)
                        ));
                        if !comp.success && state.config.self_healing_enabled {
                            println!(
                                "{}",
                                "🩹 Compile Guard detected error. Self-healing triggered..."
                                    .yellow()
                            );
                            res.push_str("\n\n[SELF-HEALING REQUIRED]: Compilation failed. Please emit a corrected ```tool:apply_patch immediately to fix these compiler errors.");
                        }
                    }

                    // Optional Adversarial Review
                    if state.config.adversarial_review_enabled && !state.config.api_key.is_empty() {
                        if let Ok(content) = fs::read_to_string(&patch_res.path) {
                            match review::AdversarialReviewer::review_code(
                                &patch_res.path,
                                &content,
                                "Automatic post-patch safety review",
                                &state.config.api_key,
                            )
                            .await
                            {
                                Ok(report) => {
                                    println!("{}", review::format_review_banner(&report));
                                    res.push_str(&format!(
                                        "\n\n[ADVERSARIAL REVIEW: {} (Score: {}/100)]\n{}",
                                        report.verdict.badge(),
                                        report.score,
                                        report.raw_response
                                    ));
                                }
                                Err(e) => println!("Adversarial review warning: {}", e),
                            }
                        }
                    }
                    res
                }
                Err(e) => format!("Failed to apply patch: {}", e),
            }
        }
        "tdd_cycle" => {
            let spec = body.trim();
            if spec.is_empty() {
                "Error: Missing feature specification for tdd_cycle".to_string()
            } else {
                match tdd::TddOrchestrator::run_tdd_cycle(
                    spec,
                    &state.workspace_dir,
                    &state.config.api_key,
                    &state.config.active_model,
                )
                .await
                {
                    Ok(rep) => tdd::format_tdd_banner(&rep),
                    Err(e) => format!("TDD cycle execution error: {}", e),
                }
            }
        }
        "adversarial_review" => {
            let mut target = "code_patch".to_string();
            let mut context = "".to_string();
            let mut content = "".to_string();
            let mut in_content = false;

            for line in body.lines() {
                if !in_content {
                    if let Some(val) = line.strip_prefix("target:") {
                        target = val.trim().to_string();
                        continue;
                    } else if let Some(val) = line.strip_prefix("context:") {
                        context = val.trim().to_string();
                        continue;
                    } else if line.trim() == "content:" || line.trim() == "---" {
                        in_content = true;
                        continue;
                    }
                }
                if in_content {
                    content.push_str(line);
                    content.push('\n');
                }
            }

            if content.trim().is_empty() {
                content = body.to_string();
            }

            match review::AdversarialReviewer::review_code(
                &target,
                &content,
                &context,
                &state.config.api_key,
            )
            .await
            {
                Ok(report) => review::format_review_banner(&report),
                Err(e) => format!("Adversarial Review failed: {}", e),
            }
        }
        "run_dag" => {
            let trimmed = body.trim();
            if trimmed.starts_with("audit") {
                let target = trimmed.strip_prefix("audit").unwrap_or("").trim();
                let target = if target.is_empty() { "." } else { target };
                let dag = subagent::create_audit_dag(target);
                match state.subagent_mgr.run_dag(dag, &state.config.api_key).await {
                    Ok(rep) => rep,
                    Err(e) => format!("DAG execution error: {}", e),
                }
            } else if trimmed.starts_with("feature") {
                let spec = trimmed.strip_prefix("feature").unwrap_or("").trim();
                let dag = subagent::create_feature_dag(spec);
                match state.subagent_mgr.run_dag(dag, &state.config.api_key).await {
                    Ok(rep) => rep,
                    Err(e) => format!("DAG execution error: {}", e),
                }
            } else {
                "Usage: run_dag audit <target> OR run_dag feature <spec>".to_string()
            }
        }
        "code_search" => search_index::execute_code_search_tool(&state.get_code_index(), body),
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
    // 1. Hybrid Cascading Model Resolution:
    let last_user_prompt = state
        .history
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .map(|m| m.content.as_str())
        .unwrap_or("");

    let (target_model, tier_note) = if state.config.cascading_enabled {
        let tier = routing::classify_prompt_tier(last_user_prompt);
        match tier {
            routing::ModelTier::Fast => (state.config.fast_model.clone(), " [Fast-Tier]"),
            routing::ModelTier::Heavy => (state.config.heavy_model.clone(), " [Heavy-Tier]"),
        }
    } else {
        (state.config.active_model.clone(), "")
    };

    // 2. Prompt Caching Header Construction:
    let req_messages = routing::build_cached_messages(
        &state.system_prompt(),
        &state.history,
        state.config.prompt_caching,
    );

    // 3. Failover Candidate Chain Resolution:
    let candidates = routing::get_failover_candidates(&target_model, &state.config.fallback_models);

    for (candidate_idx, model_candidate) in candidates.iter().enumerate() {
        let payload = json!({
            "model": model_candidate,
            "messages": req_messages,
            "temperature": 0.2,
            "stream": true
        });

        let resp_result = state
            .client
            .post(OPENROUTER_COMPLETIONS_URL)
            .header("Authorization", format!("Bearer {}", state.config.api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/dwaddle/orbit-cli")
            .header("X-Title", "Orbit-CLI")
            .json(&payload)
            .send()
            .await;

        let resp = match resp_result {
            Ok(r) => r,
            Err(e) => {
                if candidate_idx + 1 < candidates.len() {
                    let next = &candidates[candidate_idx + 1];
                    println!(
                        "\n{}",
                        format!(
                            "⚠️ Network error requesting model '{}': {}. Auto-failing over to fallback '{}'...",
                            model_candidate, e, next
                        )
                        .yellow()
                    );
                    continue;
                } else {
                    return Err(anyhow::anyhow!("OpenRouter Network Error: {}", e));
                }
            }
        };

        let status = resp.status();
        if !status.is_success() {
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            if routing::is_retryable_status(status.as_u16()) && candidate_idx + 1 < candidates.len()
            {
                let next = &candidates[candidate_idx + 1];
                println!(
                    "\n{}",
                    format!(
                        "⚠️ Model '{}' returned HTTP {} ({}). Auto-failing over to fallback '{}'...",
                        model_candidate,
                        status.as_u16(),
                        err_text.trim(),
                        next
                    )
                    .yellow()
                );
                continue;
            } else {
                return Err(anyhow::anyhow!(
                    "OpenRouter API Error (HTTP {}): {}",
                    status.as_u16(),
                    err_text
                ));
            }
        }

        // Successfully connected to model candidate
        let is_failover = candidate_idx > 0;
        let failover_badge = if is_failover {
            " [Failover Active]"
        } else {
            ""
        };
        let cache_badge = if state.config.prompt_caching {
            " [Cached]"
        } else {
            ""
        };

        print!(
            "\n{} {}{}{}{} ",
            "🤖".bold(),
            model_candidate.cyan(),
            tier_note.magenta(),
            failover_badge.yellow(),
            cache_badge.dimmed()
        );
        io::stdout().flush()?;

        let mut stream = resp.bytes_stream().eventsource();
        let mut full_text = String::new();
        let mut highlighter =
            highlight::StreamingHighlighter::new(state.config.syntax_highlighting);

        while let Some(event) = stream.next().await {
            match event {
                Ok(evt) => {
                    if evt.data == "[DONE]" {
                        break;
                    }
                    if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(&evt.data) {
                        if let Some(content) = chunk["choices"][0]["delta"]["content"].as_str() {
                            highlighter.feed(content);
                            full_text.push_str(content);
                        }
                    }
                }
                Err(_) => break,
            }
        }
        highlighter.flush();
        println!("\n");
        return Ok(full_text);
    }

    Err(anyhow::anyhow!(
        "All candidate models in failover chain were exhausted without success."
    ))
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
    if args
        .iter()
        .any(|a| a == "watch" || a == "--watch" || a == "-w")
    {
        return watcher::run_watch_mode(&state.workspace_dir).await;
    }

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
        "Workspace: {}",
        state.workspace_dir.display().to_string().cyan()
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
    println!(
        "Routing: Caching: {} | Cascading: {} | Auto-Failover: {} routes",
        if state.config.prompt_caching {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        },
        if state.config.cascading_enabled {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        },
        state.config.fallback_models.len().to_string().cyan()
    );
    println!(
        "Orchestration: Adversarial Review: {} | DAG Workflows: READY | Background Watcher: READY",
        if state.config.adversarial_review_enabled {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        }
    );
    println!(
        "Engine: Compile Guard: {} | Self-Healing: {} | Checkpoints: READY | Patching: SURGICAL (Aider-style)",
        if state.config.compile_guard_enabled {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        },
        if state.config.self_healing_enabled {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        }
    );
    println!("Type `/help` for commands or `/model` to view/select models.\n");

    let mut rl = DefaultEditor::new()?;
    let _ = rl.load_history(&state.config_path.with_file_name("history.txt"));

    let tool_re = Regex::new(
        r"```tool:(run_command|view_file|write_file|apply_patch|tdd_cycle|invoke_subagent|mcp|code_search|code_outline|git_context|adversarial_review|run_dag)\n([\s\S]*?)```",
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
                                "  /undo                  - Instant checkpoint rollback of last modified file"
                            );
                            println!(
                                "  /checkpoints           - List recent file checkpoints and snapshot stack"
                            );
                            println!(
                                "  /compile / /check      - Run instant workspace compile & AST type check"
                            );
                            println!(
                                "  /guard                 - Toggle AST Compile Guard (auto cargo check) ON/OFF"
                            );
                            println!(
                                "  /heal                  - Toggle compiler diagnostic self-healing loop ON/OFF"
                            );
                            println!(
                                "  /tdd <spec>            - Autonomous Red -> Green -> Refactor test-driven loop"
                            );
                            println!(
                                "  /dag [audit|feature]   - Orchestrate concurrent subagent DAG workflows"
                            );
                            println!(
                                "  /review [file|diff]    - Run adversarial code review on diff or file"
                            );
                            println!(
                                "  /adversarial           - Toggle auto adversarial review on file writes"
                            );
                            println!(
                                "  /watch [start|stop|run]- Background file watcher & debounced test loop"
                            );
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
                                "  /cache                 - Toggle prompt caching headers ON/OFF"
                            );
                            println!(
                                "  /cascade               - Toggle hybrid model cascading ON/OFF"
                            );
                            println!("  /routing               - View active routing, tiers, and failover status");
                            println!(
                                "  /fallback <m1,m2,...>  - Set failover model priority chain"
                            );
                            println!(
                                "  /tools                 - Toggle auto tool execution ON/OFF"
                            );
                            println!(
                                "  /highlight / /syntax   - Toggle real-time streaming syntax highlighting ON/OFF"
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
                                let idx = state.get_code_index();
                                let results = idx.search(arg, 8);
                                println!("{}", idx.format_results(arg, &results));
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
                        "/highlight" | "/syntax" => {
                            state.config.syntax_highlighting = !state.config.syntax_highlighting;
                            let _ = state.save_config();
                            println!(
                                "Real-time streaming syntax highlighting set to: {}",
                                if state.config.syntax_highlighting {
                                    "ENABLED (Rust, Python, TS/JS, Go, Bash, SQL, JSON, TOML, C/C++)".bold().green()
                                } else {
                                    "DISABLED (Raw stream)".yellow()
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
                        "/cache" => {
                            state.config.prompt_caching = !state.config.prompt_caching;
                            let _ = state.save_config();
                            println!(
                                "Prompt Caching set to: {}",
                                if state.config.prompt_caching {
                                    "ENABLED (Anthropic/DeepSeek ephemeral headers active)"
                                        .bold()
                                        .green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            continue;
                        }
                        "/cascade" => {
                            state.config.cascading_enabled = !state.config.cascading_enabled;
                            let _ = state.save_config();
                            println!(
                                "Hybrid Model Cascading set to: {}",
                                if state.config.cascading_enabled {
                                    format!(
                                        "ENABLED (Fast: {}, Heavy: {})",
                                        state.config.fast_model.cyan(),
                                        state.config.heavy_model.cyan()
                                    )
                                    .bold()
                                    .green()
                                } else {
                                    "DISABLED (Direct active model routing)".yellow()
                                }
                            );
                            continue;
                        }
                        "/routing" => {
                            println!(
                                "{}",
                                "=== 🛰️ MULTI-MODEL ROUTING & CACHING STATUS ==="
                                    .bold()
                                    .cyan()
                            );
                            println!(
                                "• Active Model: {}",
                                state.config.active_model.bold().green()
                            );
                            println!(
                                "• Prompt Caching: {}",
                                if state.config.prompt_caching {
                                    "ENABLED (up to 90% cost reduction on Claude/DeepSeek)".green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            println!(
                                "• Hybrid Cascading: {}",
                                if state.config.cascading_enabled {
                                    "ENABLED (Tiered auto-routing)".green()
                                } else {
                                    "DISABLED (Single model mode)".yellow()
                                }
                            );
                            println!(
                                "  - Fast Tier (Exploration): {}",
                                state.config.fast_model.cyan()
                            );
                            println!(
                                "  - Heavy Tier (Synthesis):   {}",
                                state.config.heavy_model.cyan()
                            );
                            println!("• Failover Recovery Chain:");
                            let candidates = routing::get_failover_candidates(
                                &state.config.active_model,
                                &state.config.fallback_models,
                            );
                            for (i, m) in candidates.iter().enumerate() {
                                println!("  {}. {}", i + 1, m.bold());
                            }
                            continue;
                        }
                        "/fallback" => {
                            if arg.is_empty() {
                                println!("Usage: /fallback <model1,model2,...>");
                                println!(
                                    "Current fallback chain: {}",
                                    state.config.fallback_models.join(", ").cyan()
                                );
                            } else {
                                let models: Vec<String> = arg
                                    .split(',')
                                    .map(|s| s.trim().to_string())
                                    .filter(|s| !s.is_empty())
                                    .collect();
                                if !models.is_empty() {
                                    state.config.fallback_models = models;
                                    let _ = state.save_config();
                                    println!(
                                        "✅ Updated fallback models chain: {}",
                                        state.config.fallback_models.join(", ").green()
                                    );
                                }
                            }
                            continue;
                        }
                        "/dag" => {
                            if state.config.api_key.is_empty() {
                                println!(
                                    "{}",
                                    "Error: OpenRouter API key is required to run DAG subagents."
                                        .red()
                                );
                                continue;
                            }
                            let trimmed = arg.trim();
                            let dag = if trimmed.starts_with("audit") {
                                let target = trimmed.strip_prefix("audit").unwrap_or("").trim();
                                let target = if target.is_empty() { "." } else { target };
                                subagent::create_audit_dag(target)
                            } else if trimmed.starts_with("feature") {
                                let spec = trimmed.strip_prefix("feature").unwrap_or("").trim();
                                if spec.is_empty() {
                                    println!("Usage: /dag feature <feature description>");
                                    continue;
                                }
                                subagent::create_feature_dag(spec)
                            } else {
                                println!(
                                    "Usage: /dag audit [path] OR /dag feature <specification>"
                                );
                                continue;
                            };

                            match state.subagent_mgr.run_dag(dag, &state.config.api_key).await {
                                Ok(rep) => {
                                    println!("\n{}", rep);
                                    state.history.push(Message {
                                        role: "assistant".to_string(),
                                        content: rep,
                                    });
                                }
                                Err(e) => println!("❌ DAG execution failed: {}", e),
                            }
                            continue;
                        }
                        "/review" => {
                            if state.config.api_key.is_empty() {
                                println!(
                                    "{}",
                                    "Error: OpenRouter API key is required for adversarial review."
                                        .red()
                                );
                                continue;
                            }
                            let target = arg.trim();
                            let (target_name, content, notes) = if target.is_empty()
                                || target == "diff"
                            {
                                match git_context::get_git_diff(&state.workspace_dir, None, false) {
                                    Ok(diff) => {
                                        if diff.trim().is_empty() {
                                            println!("No uncommitted git diff found. Specify a file: `/review <path>`");
                                            continue;
                                        }
                                        (
                                            "git diff (workspace)".to_string(),
                                            diff,
                                            "Active workspace uncommitted changes".to_string(),
                                        )
                                    }
                                    Err(e) => {
                                        println!("Failed to read git diff: {}", e);
                                        continue;
                                    }
                                }
                            } else {
                                match fs::read_to_string(target) {
                                    Ok(c) => {
                                        (target.to_string(), c, format!("Review file: {}", target))
                                    }
                                    Err(e) => {
                                        println!("Failed to read file '{}': {}", target, e);
                                        continue;
                                    }
                                }
                            };

                            println!(
                                "{}",
                                format!("🛡️ Running adversarial review on '{}'...", target_name)
                                    .dimmed()
                            );
                            match review::AdversarialReviewer::review_code(
                                &target_name,
                                &content,
                                &notes,
                                &state.config.api_key,
                            )
                            .await
                            {
                                Ok(rep) => {
                                    let banner = review::format_review_banner(&rep);
                                    println!("\n{}", banner);
                                    state.history.push(Message {
                                        role: "assistant".to_string(),
                                        content: banner,
                                    });
                                }
                                Err(e) => println!("❌ Adversarial review failed: {}", e),
                            }
                            continue;
                        }
                        "/adversarial" => {
                            state.config.adversarial_review_enabled =
                                !state.config.adversarial_review_enabled;
                            let _ = state.save_config();
                            println!(
                                "Automatic Adversarial Review on file write: {}",
                                if state.config.adversarial_review_enabled {
                                    "ENABLED".bold().green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            continue;
                        }
                        "/watch" => {
                            let subcmd = arg.trim();
                            match subcmd {
                                "start" => {
                                    let mut guard = state.active_watcher.lock().unwrap();
                                    if guard.is_some() {
                                        println!("Background watcher is already active.");
                                    } else {
                                        let watcher = Arc::new(watcher::OrbitWatcher::new(
                                            &state.workspace_dir,
                                        ));
                                        if let Err(e) = watcher.start_background_loop() {
                                            println!("Failed to start watcher: {}", e);
                                        } else {
                                            *guard = Some(watcher);
                                        }
                                    }
                                }
                                "stop" => {
                                    let mut guard = state.active_watcher.lock().unwrap();
                                    if let Some(w) = guard.take() {
                                        w.stop();
                                        println!("✅ Background file watcher stopped.");
                                    } else {
                                        println!("No background watcher is currently active.");
                                    }
                                }
                                "run" | "" => {
                                    println!("Running project tests once...");
                                    let res = watcher::run_project_tests(&state.workspace_dir);
                                    if res.success {
                                        println!(
                                            "{}",
                                            format!(
                                                "✅ [PASS] {} (completed in {}ms)",
                                                res.command, res.duration_ms
                                            )
                                            .bold()
                                            .green()
                                        );
                                    } else {
                                        println!(
                                            "{}",
                                            format!(
                                                "❌ [FAIL] {} (exited with error in {}ms)",
                                                res.command, res.duration_ms
                                            )
                                            .bold()
                                            .red()
                                        );
                                        let lines: Vec<&str> = res.output.lines().collect();
                                        let start = lines.len().saturating_sub(15);
                                        for l in &lines[start..] {
                                            println!("   {}", l.red());
                                        }
                                    }
                                }
                                _ => {
                                    println!("Usage: /watch [start|stop|run]");
                                }
                            }
                            continue;
                        }
                        "/undo" => {
                            match state.checkpoint_mgr.rollback_last() {
                                Ok(res) => {
                                    if res.was_deleted {
                                        println!(
                                            "{}",
                                            format!(
                                                "⏪ [UNDO] Removed newly created file: {}",
                                                res.path.display()
                                            )
                                            .bold()
                                            .green()
                                        );
                                    } else {
                                        println!(
                                            "{}",
                                            format!(
                                                "⏪ [UNDO] Restored {} bytes to {} (undid '{}')",
                                                res.restored_bytes,
                                                res.path.display(),
                                                res.action
                                            )
                                            .bold()
                                            .green()
                                        );
                                    }
                                }
                                Err(e) => println!("{}", format!("❌ Undo failed: {}", e).red()),
                            }
                            continue;
                        }
                        "/checkpoints" => {
                            let snapshots = state.checkpoint_mgr.list_checkpoints();
                            println!("{}", "=== ⏱️ CHECKPOINTS & UNDO STACK ===".bold().cyan());
                            if snapshots.is_empty() {
                                println!("No checkpoints recorded yet.");
                            } else {
                                for s in snapshots.iter().rev().take(10) {
                                    println!(
                                        "• Snapshot #{} [{}] {} (has previous content: {})",
                                        s.id,
                                        s.action.yellow(),
                                        s.path.display().to_string().bold(),
                                        s.previous_content.is_some()
                                    );
                                }
                            }
                            continue;
                        }
                        "/guard" => {
                            state.config.compile_guard_enabled =
                                !state.config.compile_guard_enabled;
                            let _ = state.save_config();
                            println!(
                                "AST Compile Guard set to: {}",
                                if state.config.compile_guard_enabled {
                                    "ENABLED (automatic compile check after edits)"
                                        .bold()
                                        .green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            continue;
                        }
                        "/heal" => {
                            state.config.self_healing_enabled = !state.config.self_healing_enabled;
                            let _ = state.save_config();
                            println!(
                                "Compiler Diagnostic Self-Healing loop set to: {}",
                                if state.config.self_healing_enabled {
                                    "ENABLED (automatic self-repair on compiler errors)"
                                        .bold()
                                        .green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            continue;
                        }
                        "/compile" | "/check" => {
                            println!("Running workspace AST compile & type check...");
                            let res = state.compile_guard.check();
                            println!("{}", compile_guard::format_compile_banner(&res));
                            continue;
                        }
                        "/tdd" => {
                            if state.config.api_key.is_empty() {
                                println!(
                                    "{}",
                                    "Error: OpenRouter API key is required to run TDD cycle.".red()
                                );
                                continue;
                            }
                            let spec = arg.trim();
                            if spec.is_empty() {
                                println!("Usage: /tdd <feature specification>");
                                continue;
                            }
                            match tdd::TddOrchestrator::run_tdd_cycle(
                                spec,
                                &state.workspace_dir,
                                &state.config.api_key,
                                &state.config.active_model,
                            )
                            .await
                            {
                                Ok(rep) => {
                                    let banner = tdd::format_tdd_banner(&rep);
                                    println!("\n{}", banner);
                                    state.history.push(Message {
                                        role: "assistant".to_string(),
                                        content: banner,
                                    });
                                }
                                Err(e) => println!("❌ TDD cycle failed: {}", e),
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
