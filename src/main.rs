use anyhow::Result;
use colored::*;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use regex::Regex;
use rustyline::config::Configurer;
use rustyline::error::ReadlineError;
use rustyline::{Config, DefaultEditor, EditMode};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

pub mod adr;
pub mod ast_grep;
pub mod benchmark;
pub mod bisect;
pub mod checkpoint;
pub mod commit;
pub mod compile_guard;
pub mod context;
pub mod diff_review;
pub mod dispatch;
pub mod editor;
pub mod export;
pub mod fetch;
pub mod fuzzy_picker;
pub mod gh_cli;
pub mod git_context;
pub mod grep;
pub mod highlight;
pub mod lsp;
mod mcp;
pub mod memory;
pub mod metrics;
pub mod modal_prompt;
pub mod outline;
pub mod patch;
pub mod plan;
pub mod query_cache;
pub mod repo_map;
pub mod review;
pub mod routing;
pub mod rules;
pub mod search_index;
pub mod security;
pub mod session;
pub mod spinner;
pub mod subagent;
pub mod tdd;
pub mod tui;
pub mod watcher;
pub mod worktree;

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
    #[serde(default = "default_true")]
    pub vim_mode: bool,
    #[serde(default)]
    pub diff_approval_required: bool,
    #[serde(default = "default_true")]
    pub live_hud_enabled: bool,
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
            vim_mode: true,
            diff_approval_required: false,
            live_hud_enabled: true,
        }
    }
}

#[derive(Deserialize, Debug, Clone, Default)]
pub struct ModelPricing {
    pub prompt: Option<String>,
    pub completion: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct OpenRouterModel {
    pub id: String,
    pub name: String,
    pub context_length: Option<u64>,
    #[serde(default)]
    pub pricing: Option<ModelPricing>,
}

impl OpenRouterModel {
    pub fn prompt_price_per_token(&self) -> Option<f64> {
        self.pricing
            .as_ref()
            .and_then(|p| p.prompt.as_deref())
            .and_then(|s| s.parse().ok())
    }

    pub fn completion_price_per_token(&self) -> Option<f64> {
        self.pricing
            .as_ref()
            .and_then(|p| p.completion.as_deref())
            .and_then(|s| s.parse().ok())
    }
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
    pub metrics: metrics::SessionMetrics,
    pub active_sandbox: Arc<Mutex<Option<worktree::WorktreeSandbox>>>,
    pub tool_permissions: security::ToolPermissions,
    pub secret_redactor: security::SecretRedactor,
    pub query_cache: query_cache::QueryCache,
    pub project_memory: Option<memory::ProjectMemory>,
}

impl AgentState {
    pub fn new(initial_dir: Option<PathBuf>) -> Result<Self> {
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

        let workspace_dir = match initial_dir {
            Some(dir) => {
                let expanded = if dir.to_string_lossy().starts_with("~/") {
                    let home = std::env::var("HOME").unwrap_or_default();
                    PathBuf::from(format!("{}{}", home, &dir.to_string_lossy()[1..]))
                } else if dir.to_string_lossy() == "~" {
                    std::env::var("HOME")
                        .map(PathBuf::from)
                        .unwrap_or_else(|_| PathBuf::from("."))
                } else {
                    dir
                };
                let canonical = fs::canonicalize(&expanded).unwrap_or(expanded);
                let _ = std::env::set_current_dir(&canonical);
                canonical
            }
            None => std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
        };
        let project_rules = rules::ProjectRules::discover(&workspace_dir);
        let checkpoint_mgr = checkpoint::CheckpointManager::new();
        let compile_guard = compile_guard::CompileGuard::new(&workspace_dir);
        let metrics = metrics::SessionMetrics::new();
        let tool_permissions = security::ToolPermissions::default();
        let secret_redactor = security::SecretRedactor::default();
        let query_cache = query_cache::QueryCache::default();
        let project_memory = memory::ProjectMemory::load_or_init(&workspace_dir).ok();

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
            metrics,
            active_sandbox: Arc::new(Mutex::new(None)),
            tool_permissions,
            secret_redactor,
            query_cache,
            project_memory,
        })
    }

    pub fn change_workspace(&mut self, new_path: &Path) -> Result<PathBuf> {
        let expanded = if new_path.to_string_lossy().starts_with("~/") {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(format!("{}{}", home, &new_path.to_string_lossy()[1..]))
        } else if new_path.to_string_lossy() == "~" || new_path.as_os_str().is_empty() {
            std::env::var("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|_| PathBuf::from("."))
        } else {
            new_path.to_path_buf()
        };
        let canonical = fs::canonicalize(&expanded)
            .map_err(|e| anyhow::anyhow!("Directory not found `{}`: {}", new_path.display(), e))?;
        if !canonical.is_dir() {
            anyhow::bail!("Path `{}` is not a directory", canonical.display());
        }
        std::env::set_current_dir(&canonical)?;
        self.workspace_dir = canonical.clone();
        *self.code_index.lock().unwrap() = None;
        self.project_rules = rules::ProjectRules::discover(&self.workspace_dir);
        self.compile_guard = compile_guard::CompileGuard::new(&self.workspace_dir);
        Ok(canonical)
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
        let base = "You are Orbit, an autonomous AI coding and security engineer paired with a developer in an interactive CLI session.\n\
        You run directly on the user's local operating system (Linux) and have full access to execute local CLI tools, scripts, binaries, and system commands via ```tool:run_command.\n\
        When the user asks you to run a program, test an endpoint, scan a target, execute tests, or check system status, USE ```tool:run_command to execute it directly on the local machine.\n\
        Do NOT claim you lack local machine access. You have full native terminal execution capabilities.\n";

        if !self.config.tools_enabled {
            return base.to_string();
        }

        let mut prompt = format!(
            "{}\n\
            ## AVAILABLE TOOLS:\n\
            To run a tool, emit a block formatted EXACTLY like this:\n\n\
            ```tool:run_command\n\
            <command to execute in bash on local machine>\n\
            ```\n\
            Execute any local program, binary, script, test suite, or CLI utility (e.g. bash, python3, curl, nmap, nikto, sqlmap, git, cargo, docker, npm) directly on the host machine. Output (STDOUT/STDERR/exit code) is returned to you.\n\n\
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
            ```\n\n\
            ```tool:worktree_sandbox\n\
            action: create | diff | apply | discard\n\
            task: <task name for new sandbox>\n\
            ```\n\n\
            ```tool:lsp_query\n\
            action: hover | definition | symbols\n\
            path: <file path>\n\
            line: <1-based line number>\n\
            col: <1-based col number>\n\
            ```\n\n\
            ```tool:export_session\n\
            <optional output path for Org-mode report>\n\
            ```\n\n",
            base
        );

        if let Some(rules) = &self.project_rules {
            prompt.push_str(&rules.format_for_prompt());
        }

        if !self.mcp_tools.is_empty() {
            prompt.push_str("## MCP TOOLS:\n");
            for t in &self.mcp_tools {
                let params_hint = t
                    .input_schema
                    .get("properties")
                    .and_then(|p| p.as_object())
                    .map(|obj| {
                        let keys: Vec<&str> = obj.keys().map(|k| k.as_str()).collect();
                        if keys.is_empty() {
                            "{}".to_string()
                        } else {
                            format!("{{ {} }}", keys.join(", "))
                        }
                    })
                    .unwrap_or_else(|| "{}".to_string());
                prompt.push_str(&format!(
                    "• MCP Tool [{}/{}] (args: {}): {}\n",
                    t.server_name,
                    t.name,
                    params_hint,
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

pub const TOOL_REGEX_PATTERN: &str = r"```tool:(run_command|view_file|write_file|apply_patch|tdd_cycle|invoke_subagent|mcp|code_search|code_outline|git_context|adversarial_review|run_dag|code_grep|code_map|worktree_sandbox|lsp_query|export_session)\n([\s\S]*?)```";

pub async fn execute_tool_quiet(tool_name: &str, body: &str, state: &AgentState) -> String {
    execute_tool_internal(tool_name, body, state, true).await
}

async fn execute_tool(tool_name: &str, body: &str, state: &AgentState) -> String {
    execute_tool_internal(tool_name, body, state, false).await
}

async fn execute_tool_internal(
    tool_name: &str,
    body: &str,
    state: &AgentState,
    quiet: bool,
) -> String {
    if !quiet {
        println!(
            "\n{}",
            format!("⚙️ Executing Tool: {}", tool_name).bold().yellow()
        );
    }
    match tool_name {
        "run_command" => {
            let cmd = body.trim();
            if !quiet {
                println!("$ {}", cmd.bold().white());
            }
            let mut cmd_builder = Command::new("bash");
            cmd_builder.arg("-c").arg(cmd);
            if state.workspace_dir.is_dir() {
                cmd_builder.current_dir(&state.workspace_dir);
            }

            if let Ok(path_var) = std::env::var("PATH") {
                let home = std::env::var("HOME").unwrap_or_default();
                cmd_builder.env(
                    "PATH",
                    format!(
                        "{}/bin:{}/.cargo/bin:/usr/local/bin:{}",
                        home, home, path_var
                    ),
                );
            }

            match cmd_builder.output() {
                Ok(out) => {
                    let mut stdout = String::from_utf8_lossy(&out.stdout).to_string();
                    let mut stderr = String::from_utf8_lossy(&out.stderr).to_string();
                    let code = out.status.code().unwrap_or(-1);

                    const MAX_LINES: usize = 300;
                    let stdout_lines: Vec<&str> = stdout.lines().collect();
                    if stdout_lines.len() > MAX_LINES {
                        stdout = format!(
                            "{}\n... (truncated, total {} lines)",
                            stdout_lines[..MAX_LINES].join("\n"),
                            stdout_lines.len()
                        );
                    }
                    let stderr_lines: Vec<&str> = stderr.lines().collect();
                    if stderr_lines.len() > MAX_LINES {
                        stderr = format!(
                            "{}\n... (truncated, total {} lines)",
                            stderr_lines[..MAX_LINES].join("\n"),
                            stderr_lines.len()
                        );
                    }

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

            // Interactive diff approval check
            if state.config.diff_approval_required && !quiet {
                let existing = fs::read_to_string(path).unwrap_or_default();
                let diff = diff_review::generate_unified_diff(&existing, content, path);
                if !diff_review::prompt_diff_approval(&diff, path).unwrap_or(true) {
                    return format!("Modification to {} was rejected by user.", path);
                }
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
                        let comp = state.compile_guard.check_for_path(Path::new(path));
                        if comp.command != "none" {
                            if !quiet {
                                println!("{}", compile_guard::format_compile_banner(&comp));
                            }
                            res.push_str(&format!(
                                "\n\n{}",
                                compile_guard::format_compile_banner(&comp)
                            ));
                            if !comp.success && state.config.self_healing_enabled {
                                if !quiet {
                                    println!(
                                        "{}",
                                        "🩹 Compile Guard detected error. Self-healing triggered..."
                                            .yellow()
                                    );
                                }
                                res.push_str("\n\n[SELF-HEALING REQUIRED]: Compilation failed. Please emit a corrected ```tool:write_file or ```tool:apply_patch immediately to resolve these compiler errors.");
                            }
                        }
                    }

                    if state.config.adversarial_review_enabled && !state.config.api_key.is_empty() {
                        if !quiet {
                            println!("{}", "🛡️ Running automatic adversarial review...".dimmed());
                        }
                        match review::AdversarialReviewer::review_code(
                            path,
                            content,
                            "Automatic post-write safety and regression review",
                            &state.config.api_key,
                        )
                        .await
                        {
                            Ok(report) => {
                                if !quiet {
                                    println!("{}", review::format_review_banner(&report));
                                }
                                res.push_str(&format!(
                                    "\n\n[ADVERSARIAL REVIEW: {} (Score: {}/100)]\n{}",
                                    report.verdict.badge(),
                                    report.score,
                                    report.raw_response
                                ));
                            }
                            Err(e) => {
                                if !quiet {
                                    println!("Adversarial review warning: {}", e);
                                }
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
                        let comp = state
                            .compile_guard
                            .check_for_path(Path::new(&patch_res.path));
                        if comp.command != "none" {
                            if !quiet {
                                println!("{}", compile_guard::format_compile_banner(&comp));
                            }
                            res.push_str(&format!(
                                "\n\n{}",
                                compile_guard::format_compile_banner(&comp)
                            ));
                            if !comp.success && state.config.self_healing_enabled {
                                if !quiet {
                                    println!(
                                        "{}",
                                        "🩹 Compile Guard detected error. Self-healing triggered..."
                                            .yellow()
                                    );
                                }
                                res.push_str("\n\n[SELF-HEALING REQUIRED]: Compilation failed. Please emit a corrected ```tool:apply_patch immediately to fix these compiler errors.");
                            }
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
                                    if !quiet {
                                        println!("{}", review::format_review_banner(&report));
                                    }
                                    res.push_str(&format!(
                                        "\n\n[ADVERSARIAL REVIEW: {} (Score: {}/100)]\n{}",
                                        report.verdict.badge(),
                                        report.score,
                                        report.raw_response
                                    ));
                                }
                                Err(e) => {
                                    if !quiet {
                                        println!("Adversarial review warning: {}", e);
                                    }
                                }
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
        "code_grep" => {
            let pattern = body.trim();
            if !quiet {
                println!("🔎 Grepping codebase for: {}", pattern.bold().white());
            }
            let config = grep::NativeGrepConfig {
                pattern: pattern.to_string(),
                is_regex: false,
                case_sensitive: false,
                path_filter: None,
                max_results: 80,
            };
            match grep::NativeGrep::search(&state.workspace_dir, &config) {
                Ok(matches) => grep::NativeGrep::format_results(&matches, pattern, 80),
                Err(e) => format!("Error searching codebase: {}", e),
            }
        }
        "code_map" => {
            if !quiet {
                println!(
                    "{}",
                    "🗺️ Generating AST Architecture Repo Map...".bold().white()
                );
            }
            match repo_map::RepoMap::generate(&state.workspace_dir, 80) {
                Ok(map) => map,
                Err(e) => format!("Error generating repo map: {}", e),
            }
        }
        "worktree_sandbox" => {
            let mut action = "status".to_string();
            let mut task = "task".to_string();
            for line in body.lines() {
                if let Some(val) = line.strip_prefix("action:") {
                    action = val.trim().to_lowercase();
                } else if let Some(val) = line.strip_prefix("task:") {
                    task = val.trim().to_string();
                }
            }
            match action.as_str() {
                "create" => match worktree::WorktreeSandbox::new(&state.workspace_dir, &task) {
                    Ok(sandbox) => {
                        let banner = worktree::format_sandbox_banner(&sandbox, "Created");
                        let mut lock = state.active_sandbox.lock().unwrap();
                        *lock = Some(sandbox);
                        banner
                    }
                    Err(e) => format!("Failed to create worktree sandbox: {}", e),
                },
                "diff" => {
                    let lock = state.active_sandbox.lock().unwrap();
                    if let Some(sandbox) = lock.as_ref() {
                        match sandbox.diff() {
                            Ok(d) => {
                                if d.trim().is_empty() {
                                    "No changes in active sandbox.".to_string()
                                } else {
                                    d
                                }
                            }
                            Err(e) => format!("Error getting sandbox diff: {}", e),
                        }
                    } else {
                        "No active sandbox. Use action: create to start one.".to_string()
                    }
                }
                "apply" => {
                    let lock = state.active_sandbox.lock().unwrap();
                    if let Some(sandbox) = lock.as_ref() {
                        match sandbox.apply_to_main() {
                            Ok(msg) => msg,
                            Err(e) => format!("Failed to apply sandbox to main: {}", e),
                        }
                    } else {
                        "No active sandbox to apply.".to_string()
                    }
                }
                "discard" => {
                    let mut lock = state.active_sandbox.lock().unwrap();
                    if let Some(sandbox) = lock.take() {
                        match sandbox.discard() {
                            Ok(_) => format!("Discarded sandbox branch {}.", sandbox.branch),
                            Err(e) => format!("Error discarding sandbox: {}", e),
                        }
                    } else {
                        "No active sandbox to discard.".to_string()
                    }
                }
                _ => "Usage: action: create | diff | apply | discard".to_string(),
            }
        }
        "lsp_query" => {
            let mut action = "hover".to_string();
            let mut path_str = "".to_string();
            let mut line: u32 = 1;
            let mut col: u32 = 1;
            for l in body.lines() {
                if let Some(val) = l.strip_prefix("action:") {
                    action = val.trim().to_lowercase();
                } else if let Some(val) = l.strip_prefix("path:") {
                    path_str = val.trim().to_string();
                } else if let Some(val) = l.strip_prefix("line:") {
                    line = val.trim().parse().unwrap_or(1);
                } else if let Some(val) = l.strip_prefix("col:") {
                    col = val.trim().parse().unwrap_or(1);
                }
            }
            if path_str.is_empty() {
                return "Error: Missing path: for lsp_query".to_string();
            }
            match lsp::execute_lsp_query(
                &state.workspace_dir,
                Path::new(&path_str),
                line,
                col,
                &action,
            ) {
                Ok(res) => res,
                Err(e) => format!("LSP Query Error: {}", e),
            }
        }
        "export_session" => {
            let target = body.trim();
            let target_path = if target.is_empty() {
                None
            } else {
                Some(Path::new(target))
            };
            let snap = state.metrics.snapshot().await;
            let export_turns: Vec<export::ExportTurn> = state
                .history
                .iter()
                .map(|m| export::ExportTurn {
                    role: m.role.clone(),
                    content: m.content.clone(),
                })
                .collect();
            match export::export_session_to_org(
                &export_turns,
                snap.total_prompt_tokens + snap.total_completion_tokens,
                snap.total_cached_tokens,
                snap.total_cost_usd,
                &state.workspace_dir,
                &state.config.active_model,
                target_path,
            ) {
                Ok(p) => format!("Session exported to Org-mode: {}", p.display()),
                Err(e) => format!("Export failed: {}", e),
            }
        }
        _ => format!("Unknown tool: {}", tool_name),
    }
}

pub enum StreamOutputMode {
    TerminalRepl { syntax_highlighting: bool },
    Channel(tokio::sync::mpsc::UnboundedSender<String>),
    Silent,
}

pub async fn stream_completion(state: &AgentState) -> Result<String> {
    stream_completion_internal(
        state,
        StreamOutputMode::TerminalRepl {
            syntax_highlighting: state.config.syntax_highlighting,
        },
    )
    .await
}

pub async fn stream_completion_channel(
    state: &AgentState,
    sender: tokio::sync::mpsc::UnboundedSender<String>,
) -> Result<String> {
    stream_completion_internal(state, StreamOutputMode::Channel(sender)).await
}

pub async fn stream_completion_internal(
    state: &AgentState,
    mode: StreamOutputMode,
) -> Result<String> {
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
    let is_terminal = matches!(mode, StreamOutputMode::TerminalRepl { .. });

    for (candidate_idx, model_candidate) in candidates.iter().enumerate() {
        let mut spinner = if is_terminal {
            Some(spinner::Spinner::start(format!(
                "Thinking [{}]{}...",
                model_candidate, tier_note
            )))
        } else {
            None
        };

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
                if let Some(ref mut s) = spinner {
                    s.stop_and_clear();
                }
                if candidate_idx + 1 < candidates.len() {
                    let next = &candidates[candidate_idx + 1];
                    if is_terminal {
                        println!(
                            "\n{}",
                            format!(
                                "⚠️ Network error requesting model '{}': {}. Auto-failing over to fallback '{}'...",
                                model_candidate, e, next
                            )
                            .yellow()
                        );
                    }
                    continue;
                } else {
                    return Err(anyhow::anyhow!("OpenRouter Network Error: {}", e));
                }
            }
        };

        let status = resp.status();
        if !status.is_success() {
            if let Some(ref mut s) = spinner {
                s.stop_and_clear();
            }
            let err_text = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            if routing::is_retryable_status(status.as_u16()) && candidate_idx + 1 < candidates.len()
            {
                let next = &candidates[candidate_idx + 1];
                if is_terminal {
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
                }
                continue;
            } else {
                return Err(anyhow::anyhow!(
                    "OpenRouter API Error (HTTP {}): {}",
                    status.as_u16(),
                    err_text
                ));
            }
        }

        if let Some(ref mut s) = spinner {
            s.set_message(format!("Streaming from {}...", model_candidate));
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

        let start_time = std::time::Instant::now();
        let mut ttft_ms: Option<u128> = None;
        let mut parsed_usage: Option<metrics::TokenUsage> = None;

        let mut stream = resp.bytes_stream().eventsource();
        let mut full_text = String::new();
        let mut highlighter = match &mode {
            StreamOutputMode::TerminalRepl {
                syntax_highlighting,
            } => Some(highlight::StreamingHighlighter::new(*syntax_highlighting)),
            _ => None,
        };

        while let Some(event) = stream.next().await {
            match event {
                Ok(evt) => {
                    if evt.data == "[DONE]" {
                        break;
                    }
                    if let Ok(chunk) = serde_json::from_str::<serde_json::Value>(&evt.data) {
                        if let Some(content) = chunk["choices"][0]["delta"]["content"].as_str() {
                            if ttft_ms.is_none() {
                                ttft_ms = Some(start_time.elapsed().as_millis());
                                if let Some(ref mut s) = spinner {
                                    s.stop_and_clear();
                                }
                                if is_terminal {
                                    print!(
                                        "\n{} {}{}{}{} ",
                                        "🤖".bold(),
                                        model_candidate.cyan(),
                                        tier_note.magenta(),
                                        failover_badge.yellow(),
                                        cache_badge.dimmed()
                                    );
                                    io::stdout().flush()?;
                                }
                            }
                            if let Some(h) = &mut highlighter {
                                h.feed(content);
                            }
                            if let StreamOutputMode::Channel(ref tx) = mode {
                                let _ = tx.send(content.to_string());
                            }
                            full_text.push_str(content);
                        }
                        if let Some(usage_val) = chunk.get("usage") {
                            let p = usage_val["prompt_tokens"].as_u64().unwrap_or(0);
                            let c = usage_val["completion_tokens"].as_u64().unwrap_or(0);
                            let t = usage_val["total_tokens"].as_u64().unwrap_or(p + c);
                            let cached = usage_val["prompt_tokens_details"]["cached_tokens"]
                                .as_u64()
                                .or_else(|| usage_val["cached_tokens"].as_u64())
                                .unwrap_or(0);
                            parsed_usage = Some(metrics::TokenUsage {
                                prompt_tokens: p,
                                completion_tokens: c,
                                total_tokens: t,
                                cached_tokens: cached,
                            });
                        }
                    }
                }
                Err(_) => break,
            }
        }

        let duration_ms = start_time.elapsed().as_millis();
        let usage = parsed_usage.unwrap_or_else(|| {
            let p = context::estimate_tokens(&format!("{:?}", req_messages)) as u64;
            let c = context::estimate_tokens(&full_text) as u64;
            let cached = if state.config.prompt_caching {
                p.saturating_mul(8) / 10
            } else {
                0
            };
            metrics::TokenUsage {
                prompt_tokens: p,
                completion_tokens: c,
                total_tokens: p + c,
                cached_tokens: cached,
            }
        });

        let (prompt_price, comp_price) = state
            .available_models
            .iter()
            .find(|m| m.id == *model_candidate)
            .map(|m| (m.prompt_price_per_token(), m.completion_price_per_token()))
            .unwrap_or((None, None));

        state
            .metrics
            .record_turn(
                model_candidate,
                usage,
                duration_ms,
                ttft_ms,
                prompt_price,
                comp_price,
            )
            .await;

        if let Some(mut h) = highlighter {
            h.flush();
            println!("\n");
            let badge = state.metrics.format_badge().await;
            println!("{}", badge.dimmed());
            if state.config.live_hud_enabled {
                let snap = state.metrics.snapshot().await;
                if let Some(last_turn) = snap.turns.last() {
                    println!(
                        "{}",
                        state.metrics.format_live_turn_hud(last_turn).await.cyan()
                    );
                }
            }
        }
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

    let args: Vec<String> = std::env::args().collect();
    let mut target_dir: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        let arg = &args[i];
        if arg == "-C" || arg == "--dir" || arg == "--workspace" {
            if i + 1 < args.len() {
                target_dir = Some(PathBuf::from(&args[i + 1]));
                i += 1;
            }
        } else if let Some(val) = arg.strip_prefix("-C=") {
            target_dir = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--dir=") {
            target_dir = Some(PathBuf::from(val));
        } else if let Some(val) = arg.strip_prefix("--workspace=") {
            target_dir = Some(PathBuf::from(val));
        } else if !arg.starts_with('-') && arg != "watch" {
            let p = PathBuf::from(arg);
            if p.is_dir() {
                target_dir = Some(p);
            }
        }
        i += 1;
    }

    let mut state = AgentState::new(target_dir)?;

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

    if args.iter().any(|a| a == "--help" || a == "-h") {
        println!(
            "{}",
            "🛰️  ORBIT-CLI - Autonomous OpenRouter Coding Agent in Rust"
                .bold()
                .cyan()
        );
        println!("Usage: orbit [OPTIONS] [WORKSPACE_DIR]\n");
        println!("Options:");
        println!("  -C, --dir <DIR>       Set active workspace directory");
        println!("  -t, --tui             Launch Ratatui fullscreen TUI mode");
        println!("  -w, --watch           Start background file watcher & test runner");
        println!(
            "  -p, --pipe <PROMPT>   Non-interactive pipe mode (reads stdin and streams response)"
        );
        println!("  -h, --help            Print help information\n");
        println!("In interactive mode, type `/help` for the full command list.");
        return Ok(());
    }

    state.init_mcp().await;

    // Stdin Pipe Mode (-p / --pipe / --prompt)
    let pipe_flag_idx = args
        .iter()
        .position(|a| a == "-p" || a == "--pipe" || a == "--prompt");
    let has_pipe_flag = pipe_flag_idx.is_some();

    if has_pipe_flag {
        use std::io::IsTerminal;
        use std::io::Read;
        let mut stdin_content = String::new();
        if !io::stdin().is_terminal() {
            let _ = io::stdin().read_to_string(&mut stdin_content);
        }

        let user_prompt = if let Some(idx) = pipe_flag_idx {
            args.get(idx + 1)
                .cloned()
                .unwrap_or_else(|| "Analyze and review the input.".to_string())
        } else {
            "Analyze and review the input.".to_string()
        };

        let combined = if !stdin_content.trim().is_empty() {
            format!(
                "[STDIN INPUT]:\n```\n{}\n```\n\n[INSTRUCTION]:\n{}",
                stdin_content.trim(),
                user_prompt
            )
        } else {
            user_prompt
        };

        if state.config.api_key.is_empty() {
            eprintln!("Error: OPENROUTER_API_KEY is not configured for non-interactive pipe mode.");
            std::process::exit(1);
        }

        state.history.push(Message {
            role: "user".to_string(),
            content: combined,
        });

        match stream_completion(&state).await {
            Ok(_) => std::process::exit(0),
            Err(e) => {
                eprintln!("Pipe execution error: {}", e);
                std::process::exit(1);
            }
        }
    }

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

    let rl_config = if state.config.vim_mode {
        Config::builder().edit_mode(EditMode::Vi).build()
    } else {
        Config::builder().edit_mode(EditMode::Emacs).build()
    };
    let history_file = state.config_path.with_file_name("history.txt");
    let mut history_lines = modal_prompt::load_history_file(&history_file);

    let mut rl = DefaultEditor::with_config(rl_config)?;
    let _ = rl.load_history(&history_file);

    let tool_re = Regex::new(TOOL_REGEX_PATTERN)?;

    let mut multi_line_acc = String::new();

    loop {
        let is_continuation = !multi_line_acc.is_empty();
        let readline = if state.config.vim_mode {
            modal_prompt::read_modal_line(&mut history_lines, Some(&history_file), is_continuation)
        } else {
            let prompt = if is_continuation {
                format!("{} > ", "...  ".dimmed())
            } else {
                format!("{} > ", "orbit".bold().magenta())
            };
            rl.readline(&prompt)
        };

        match readline {
            Ok(line) => {
                if modal_prompt::is_line_continuation(&line) {
                    let stripped = modal_prompt::strip_line_continuation(&line);
                    multi_line_acc.push_str(stripped);
                    multi_line_acc.push('\n');
                    continue;
                }

                let full_input = if !multi_line_acc.is_empty() {
                    multi_line_acc.push_str(&line);
                    std::mem::take(&mut multi_line_acc)
                } else {
                    line
                };

                let input = full_input.trim();
                if input.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(input);

                if input.starts_with('/') || input.starts_with('!') {
                    let mut parts = input.splitn(2, ' ');
                    let cmd = parts.next().unwrap_or("");
                    let arg = parts.next().unwrap_or("");

                    match cmd {
                        "/exit" | "/quit" => {
                            println!("Bye!");
                            break;
                        }
                        "/cd" => {
                            let target_str = arg.trim();
                            let target_path = if target_str.is_empty() || target_str == "~" {
                                std::env::var("HOME")
                                    .map(PathBuf::from)
                                    .unwrap_or_else(|_| PathBuf::from("."))
                            } else if target_str.starts_with("~/") {
                                let home = std::env::var("HOME").unwrap_or_default();
                                PathBuf::from(format!("{}{}", home, &target_str[1..]))
                            } else {
                                PathBuf::from(target_str)
                            };
                            match state.change_workspace(&target_path) {
                                Ok(new_ws) => {
                                    println!(
                                        "Workspace changed to: {}",
                                        new_ws.display().to_string().bold().green()
                                    );
                                    if let Some(r) = &state.project_rules {
                                        println!(
                                            "Project Rules: Loaded from {}",
                                            r.path.display().to_string().bold().cyan()
                                        );
                                    }
                                }
                                Err(e) => {
                                    println!("{} {}", "Error changing directory:".red(), e);
                                }
                            }
                            continue;
                        }
                        "/pwd" => {
                            println!(
                                "Workspace: {}",
                                state.workspace_dir.display().to_string().cyan()
                            );
                            continue;
                        }
                        cmd if cmd.starts_with('!') || cmd == "/sh" || cmd == "/exec" => {
                            let shell_cmd = if cmd.starts_with('!') {
                                input.strip_prefix('!').unwrap_or("").trim()
                            } else {
                                arg.trim()
                            };
                            if shell_cmd.is_empty() {
                                println!("Usage: !<command> or /sh <command>");
                                continue;
                            }
                            println!("$ {}", shell_cmd.bold().white());
                            let mut cmd_builder = Command::new("bash");
                            cmd_builder
                                .arg("-c")
                                .arg(shell_cmd)
                                .current_dir(&state.workspace_dir);
                            if let Ok(path_var) = std::env::var("PATH") {
                                let home = std::env::var("HOME").unwrap_or_default();
                                cmd_builder.env(
                                    "PATH",
                                    format!(
                                        "{}/bin:{}/.cargo/bin:/usr/local/bin:{}",
                                        home, home, path_var
                                    ),
                                );
                            }
                            match cmd_builder.output() {
                                Ok(out) => {
                                    let stdout = String::from_utf8_lossy(&out.stdout);
                                    let stderr = String::from_utf8_lossy(&out.stderr);
                                    if !stdout.is_empty() {
                                        print!("{}", stdout);
                                    }
                                    if !stderr.is_empty() {
                                        eprint!("{}", stderr.red());
                                    }
                                }
                                Err(e) => println!("Failed to execute command: {}", e),
                            }
                            continue;
                        }
                        "/help" => {
                            println!("{}", "=== ORBIT-CLI COMMANDS ===".bold().cyan());
                            println!(
                                "  /cd <path>             - Change active workspace directory at runtime"
                            );
                            println!(
                                "  /pwd                   - Print current workspace directory"
                            );
                            println!(
                                "  /sh <cmd> / !<cmd>     - Execute local shell command directly in workspace"
                            );
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
                            println!(
                                "  /stats / /cost         - View real-time token economics, latency & spend"
                            );
                            println!(
                                "  /grep <pattern> [glob] - Blazing fast native ripgrep across codebase"
                            );
                            println!(
                                "  /map                   - Generate compact AST PageRank architecture skeleton"
                            );
                            println!(
                                "  /find <query|@symbol>  - Instant fuzzy file and symbol finder"
                            );
                            println!("  /search <query>        - Search local codebase symbols and full-text");
                            println!("  /outline [path]        - Generate codebase symbol map and outline");
                            println!("  /git <log|blame|diff>  - Inspect git commit history, blame or diff");
                            println!(
                                "  /worktree [new|diff|apply|discard] - Speculative Git worktree sandboxing"
                            );
                            println!(
                                "  /diff [file]           - View current git diff or unified diff"
                            );
                            println!(
                                "  /diff_guard            - Toggle interactive git diff approval ON/OFF"
                            );
                            println!(
                                "  /lsp [status|hover|def|symbols] - Query local Language Server (rust-analyzer/pyright)"
                            );
                            println!(
                                "  /hud                   - Toggle live token & USD cost HUD banner ON/OFF"
                            );
                            println!(
                                "  /export [path]         - Export session conversation to Org-mode document"
                            );
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
                            println!(
                                "  /vim                   - Toggle Vim edit mode (Normal/Insert Vi bindings) ON/OFF"
                            );
                            println!("  /tui                   - Launch Ratatui fullscreen split TUI mode");
                            println!("  /clear                 - Clear chat history");
                            println!("  /key <sk-or-v1-...>    - Update OpenRouter API Key");
                            println!("  /help                  - Show this help menu");
                            println!(
                                "  \\ (at line end)        - Continue prompt on next line (multi-line input)"
                            );
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
                        "/worktree" => {
                            let root_opt = worktree::get_repo_root(&state.workspace_dir);
                            let repo_root = match root_opt {
                                Some(r) => r,
                                None => {
                                    println!(
                                        "Error: Workspace `{}` is not inside a git repository.",
                                        state.workspace_dir.display()
                                    );
                                    continue;
                                }
                            };

                            let parts: Vec<&str> = arg.split_whitespace().collect();
                            let action = parts.first().copied().unwrap_or("status");
                            match action {
                                "new" | "create" => {
                                    let task = if parts.len() > 1 {
                                        parts[1..].join("-")
                                    } else {
                                        "sandbox".to_string()
                                    };
                                    match worktree::WorktreeSandbox::new(&repo_root, &task) {
                                        Ok(sandbox) => {
                                            println!(
                                                "{}",
                                                worktree::format_sandbox_banner(
                                                    &sandbox, "Created"
                                                )
                                            );
                                            let mut lock = state.active_sandbox.lock().unwrap();
                                            *lock = Some(sandbox);
                                        }
                                        Err(e) => println!("Error creating sandbox: {}", e),
                                    }
                                }
                                "diff" => {
                                    let lock = state.active_sandbox.lock().unwrap();
                                    if let Some(sandbox) = lock.as_ref() {
                                        match sandbox.diff() {
                                            Ok(d) => {
                                                if d.trim().is_empty() {
                                                    println!("No changes in active sandbox.");
                                                } else {
                                                    print!(
                                                        "{}",
                                                        diff_review::format_colored_diff(&d)
                                                    );
                                                }
                                            }
                                            Err(e) => println!("Error getting sandbox diff: {}", e),
                                        }
                                    } else {
                                        println!(
                                            "No active sandbox. Use `/worktree new <task>` to create one."
                                        );
                                    }
                                }
                                "apply" => {
                                    let lock = state.active_sandbox.lock().unwrap();
                                    if let Some(sandbox) = lock.as_ref() {
                                        match sandbox.apply_to_main() {
                                            Ok(msg) => println!("{}", msg.green()),
                                            Err(e) => println!("Error applying sandbox: {}", e),
                                        }
                                    } else {
                                        println!("No active sandbox to apply.");
                                    }
                                }
                                "discard" => {
                                    let mut lock = state.active_sandbox.lock().unwrap();
                                    if let Some(sandbox) = lock.take() {
                                        match sandbox.discard() {
                                            Ok(_) => println!(
                                                "Successfully discarded sandbox `{}`.",
                                                sandbox.branch.yellow()
                                            ),
                                            Err(e) => println!("Error discarding sandbox: {}", e),
                                        }
                                    } else {
                                        println!("No active sandbox to discard.");
                                    }
                                }
                                "list" | "status" => {
                                    let active = state.active_sandbox.lock().unwrap();
                                    if let Some(sb) = active.as_ref() {
                                        println!(
                                            "{}",
                                            worktree::format_sandbox_banner(sb, "Active")
                                        );
                                    } else {
                                        println!(
                                            "No active speculative worktree sandbox attached."
                                        );
                                    }
                                    let all_sandboxes = worktree::list_active_sandboxes(&repo_root);
                                    if !all_sandboxes.is_empty() {
                                        println!("\nExisting sandboxes on disk:");
                                        for sb in all_sandboxes {
                                            println!("  • {}", sb.display());
                                        }
                                    }
                                }
                                _ => println!(
                                    "Usage: /worktree <new <task> | diff | apply | discard | list>"
                                ),
                            }
                            continue;
                        }
                        "/diff" => {
                            let file_filter = if arg.trim().is_empty() {
                                None
                            } else {
                                Some(arg.trim())
                            };
                            match diff_review::get_git_diff(&state.workspace_dir, file_filter) {
                                Ok(d) => {
                                    if d.trim().is_empty() {
                                        println!("No uncommitted git changes detected.");
                                    } else {
                                        print!("{}", diff_review::format_colored_diff(&d));
                                    }
                                }
                                Err(e) => println!("git diff error: {}", e),
                            }
                            continue;
                        }
                        "/diff_guard" => {
                            state.config.diff_approval_required =
                                !state.config.diff_approval_required;
                            let _ = state.save_config();
                            println!(
                                "Interactive Diff Approval Guard: {}",
                                if state.config.diff_approval_required {
                                    "ENABLED (modifications require manual confirmation)"
                                        .bold()
                                        .green()
                                } else {
                                    "DISABLED (autonomous writes enabled)".yellow()
                                }
                            );
                            continue;
                        }
                        "/lsp" => {
                            let parts: Vec<&str> = arg.split_whitespace().collect();
                            let subcmd = parts.first().copied().unwrap_or("status");
                            match subcmd {
                                "status" => {
                                    match lsp::LspDetector::detect(&state.workspace_dir) {
                                        Some((lang, bin, args)) => {
                                            println!(
                                                "🎯 LSP Server Detected: {} (Language: {:?}, Args: {:?})",
                                                bin.bold().green(),
                                                lang,
                                                args
                                            );
                                        }
                                        None => {
                                            println!(
                                                "⚠️ No Language Server (rust-analyzer, pyright, gopls) found in PATH or workspace."
                                            );
                                        }
                                    }
                                }
                                "hover" => {
                                    if parts.len() < 4 {
                                        println!("Usage: /lsp hover <file> <line> <col>");
                                    } else {
                                        let file = parts[1];
                                        let line = parts[2].parse().unwrap_or(1);
                                        let col = parts[3].parse().unwrap_or(1);
                                        match lsp::execute_lsp_query(
                                            &state.workspace_dir,
                                            Path::new(file),
                                            line,
                                            col,
                                            "hover",
                                        ) {
                                            Ok(res) => println!("{}", res),
                                            Err(e) => println!("LSP Hover Error: {}", e),
                                        }
                                    }
                                }
                                "def" | "definition" => {
                                    if parts.len() < 4 {
                                        println!("Usage: /lsp def <file> <line> <col>");
                                    } else {
                                        let file = parts[1];
                                        let line = parts[2].parse().unwrap_or(1);
                                        let col = parts[3].parse().unwrap_or(1);
                                        match lsp::execute_lsp_query(
                                            &state.workspace_dir,
                                            Path::new(file),
                                            line,
                                            col,
                                            "definition",
                                        ) {
                                            Ok(res) => println!("{}", res),
                                            Err(e) => println!("LSP Definition Error: {}", e),
                                        }
                                    }
                                }
                                "symbols" => {
                                    if parts.len() < 2 {
                                        println!("Usage: /lsp symbols <file>");
                                    } else {
                                        let file = parts[1];
                                        match lsp::execute_lsp_query(
                                            &state.workspace_dir,
                                            Path::new(file),
                                            1,
                                            1,
                                            "symbols",
                                        ) {
                                            Ok(res) => println!("{}", res),
                                            Err(e) => println!("LSP Symbols Error: {}", e),
                                        }
                                    }
                                }
                                _ => println!(
                                    "Usage: /lsp <status | hover <f> <l> <c> | def <f> <l> <c> | symbols <f>>"
                                ),
                            }
                            continue;
                        }
                        "/hud" => {
                            state.config.live_hud_enabled = !state.config.live_hud_enabled;
                            let _ = state.save_config();
                            println!(
                                "Live Token & Cost Economics HUD: {}",
                                if state.config.live_hud_enabled {
                                    "ENABLED".bold().green()
                                } else {
                                    "DISABLED".yellow()
                                }
                            );
                            continue;
                        }
                        "/export" => {
                            let target = arg.trim();
                            let target_path = if target.is_empty() {
                                None
                            } else {
                                Some(Path::new(target))
                            };
                            let snap = state.metrics.snapshot().await;
                            let export_turns: Vec<export::ExportTurn> = state
                                .history
                                .iter()
                                .map(|m| export::ExportTurn {
                                    role: m.role.clone(),
                                    content: m.content.clone(),
                                })
                                .collect();
                            match export::export_session_to_org(
                                &export_turns,
                                snap.total_prompt_tokens + snap.total_completion_tokens,
                                snap.total_cached_tokens,
                                snap.total_cost_usd,
                                &state.workspace_dir,
                                &state.config.active_model,
                                target_path,
                            ) {
                                Ok(p) => println!(
                                    "{} {}",
                                    "✅ Session successfully exported to:".bold().green(),
                                    p.display().to_string().cyan()
                                ),
                                Err(e) => println!("❌ Export failed: {}", e),
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
                        "/vim" => {
                            state.config.vim_mode = !state.config.vim_mode;
                            let _ = state.save_config();
                            if state.config.vim_mode {
                                rl.set_edit_mode(EditMode::Vi);
                                println!(
                                    "Vim edit mode: {}",
                                    "ENABLED (Modal [NORMAL]/[INSERT]/[VISUAL] prompt active in REPL & TUI)"
                                        .bold()
                                        .green()
                                );
                            } else {
                                rl.set_edit_mode(EditMode::Emacs);
                                println!(
                                    "Vim edit mode: {}",
                                    "DISABLED (Standard Emacs mode)".yellow()
                                );
                            }
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
                        "/stats" | "/cost" => {
                            let summary = state.metrics.format_summary().await;
                            println!("\n{}", summary.cyan());
                            continue;
                        }
                        "/grep" => {
                            let pattern = arg.trim();
                            if pattern.is_empty() {
                                println!("Usage: /grep <pattern> [glob]");
                            } else {
                                let mut parts = pattern.splitn(2, ' ');
                                let pat = parts.next().unwrap_or("").trim();
                                let filter = parts.next().map(|s| s.trim().to_string());
                                let config = grep::NativeGrepConfig {
                                    pattern: pat.to_string(),
                                    is_regex: false,
                                    case_sensitive: false,
                                    path_filter: filter,
                                    max_results: 50,
                                };
                                match grep::NativeGrep::search(&state.workspace_dir, &config) {
                                    Ok(matches) => {
                                        println!(
                                            "\n{}",
                                            grep::NativeGrep::format_results(&matches, pat, 50)
                                        );
                                    }
                                    Err(e) => println!("❌ Error searching: {}", e),
                                }
                            }
                            continue;
                        }
                        "/map" => {
                            match repo_map::RepoMap::generate(&state.workspace_dir, 100) {
                                Ok(map) => println!("\n{}", map.cyan()),
                                Err(e) => println!("❌ Error generating repo map: {}", e),
                            }
                            continue;
                        }
                        "/find" => {
                            let query = arg.trim();
                            let matches = if let Some(stripped) = query.strip_prefix('@') {
                                fuzzy_picker::FuzzyPicker::find_symbols(
                                    &state.workspace_dir,
                                    stripped,
                                    15,
                                )
                            } else {
                                fuzzy_picker::FuzzyPicker::find_files(
                                    &state.workspace_dir,
                                    query,
                                    15,
                                )
                            };
                            match matches {
                                Ok(m) => println!(
                                    "\n{}",
                                    fuzzy_picker::FuzzyPicker::format_results(&m, query)
                                ),
                                Err(e) => println!("❌ Error in fuzzy search: {}", e),
                            }
                            continue;
                        }
                        "/adr" => {
                            let title = arg.trim();
                            if title.is_empty() {
                                match adr::AdrManager::list_adrs(&state.workspace_dir) {
                                    Ok(list) => {
                                        if list.is_empty() {
                                            println!("No ADRs found in docs/adr/. Usage: /adr <Title of Decision>");
                                        } else {
                                            println!(
                                                "{}",
                                                "=== ARCHITECTURE DECISION RECORDS ==="
                                                    .bold()
                                                    .cyan()
                                            );
                                            for (num, t, path) in list {
                                                println!(
                                                    "  [{:04}] {} ({})",
                                                    num,
                                                    t.bold(),
                                                    path.display().to_string().cyan()
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => println!("Error listing ADRs: {}", e),
                                }
                            } else {
                                match adr::AdrManager::create_adr(&state.workspace_dir, title) {
                                    Ok((path, _)) => println!(
                                        "Created ADR: {}",
                                        path.display().to_string().bold().green()
                                    ),
                                    Err(e) => println!("Error creating ADR: {}", e),
                                }
                            }
                            continue;
                        }
                        "/commit" => {
                            match commit::CommitGenerator::get_diff(&state.workspace_dir) {
                                Ok(diff) => {
                                    match commit::CommitGenerator::analyze_and_generate(&diff) {
                                        Ok(c) => {
                                            let msg = c.format();
                                            println!(
                                                "\n{}\n{}\n",
                                                "=== SUGGESTED CONVENTIONAL COMMIT ==="
                                                    .bold()
                                                    .cyan(),
                                                msg
                                            );
                                            println!("Run `/sh git commit -m \"...\"` or review with `/diff`.");
                                        }
                                        Err(e) => println!("Cannot generate commit: {}", e),
                                    }
                                }
                                Err(e) => println!("Error getting git diff: {}", e),
                            }
                            continue;
                        }
                        "/memory" => {
                            let entry = arg.trim();
                            if entry.is_empty() {
                                if let Some(mem) = &state.project_memory {
                                    println!(
                                        "\n{}\n{}",
                                        "=== PROJECT MEMORY (.orbit/memory.md) ===".bold().cyan(),
                                        mem.content
                                    );
                                } else {
                                    println!("Project memory not loaded.");
                                }
                            } else if let Some(mem) = &mut state.project_memory {
                                match mem.append_entry("Note", entry) {
                                    Ok(_) => println!("Appended to .orbit/memory.md"),
                                    Err(e) => println!("Error saving memory: {}", e),
                                }
                            }
                            continue;
                        }
                        "/plan" => {
                            let goal = arg.trim();
                            if goal.is_empty() {
                                println!("Usage: /plan <task description>");
                            } else {
                                match plan::PlanGenerator::generate_plan(&state.workspace_dir, goal)
                                {
                                    Ok((path, content)) => {
                                        println!(
                                            "{}\n{}",
                                            format!(
                                                "Execution Plan generated at {}",
                                                path.display()
                                            )
                                            .bold()
                                            .green(),
                                            content
                                        );
                                    }
                                    Err(e) => println!("Error generating plan: {}", e),
                                }
                            }
                            continue;
                        }
                        "/benchmark" => {
                            let cmd_str = arg.trim();
                            if cmd_str.is_empty() {
                                println!("Usage: /benchmark <shell command>");
                            } else {
                                println!("Benchmarking '{}' (5 runs)...", cmd_str.bold());
                                match benchmark::BenchmarkRunner::run(
                                    cmd_str,
                                    &state.workspace_dir,
                                    5,
                                ) {
                                    Ok(stats) => println!("\n{}", stats.format_report()),
                                    Err(e) => println!("Benchmark error: {}", e),
                                }
                            }
                            continue;
                        }
                        "/edit" => {
                            let path_str = arg.trim();
                            if path_str.is_empty() {
                                println!("Usage: /edit <file_path>");
                            } else {
                                match editor::ExternalEditor::edit_file(
                                    Path::new(path_str),
                                    &state.workspace_dir,
                                ) {
                                    Ok(_) => println!("Edited file: {}", path_str),
                                    Err(e) => println!("Error spawning editor: {}", e),
                                }
                            }
                            continue;
                        }
                        "/issue" => {
                            let limit = arg.trim().parse::<usize>().unwrap_or(15);
                            match gh_cli::GhCli::list_issues(&state.workspace_dir, limit) {
                                Ok(res) => {
                                    println!("\n{}\n{}", "=== GITHUB ISSUES ===".bold().cyan(), res)
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                            continue;
                        }
                        "/pr" => {
                            let p_arg = arg.trim();
                            if let Some(num) = p_arg.strip_prefix("view ") {
                                match gh_cli::GhCli::view_pr(&state.workspace_dir, num.trim()) {
                                    Ok(res) => println!("\n{}", res),
                                    Err(e) => println!("Error: {}", e),
                                }
                            } else {
                                let limit = p_arg.parse::<usize>().unwrap_or(15);
                                match gh_cli::GhCli::list_prs(&state.workspace_dir, limit) {
                                    Ok(res) => println!(
                                        "\n{}\n{}",
                                        "=== GITHUB PULL REQUESTS ===".bold().cyan(),
                                        res
                                    ),
                                    Err(e) => println!("Error: {}", e),
                                }
                            }
                            continue;
                        }
                        "/bisect" => {
                            let parts: Vec<&str> = arg.split_whitespace().collect();
                            match parts.first().copied().unwrap_or("help") {
                                "start" => {
                                    let bad = parts.get(1).copied();
                                    let good = parts.get(2).copied();
                                    match bisect::GitBisect::start(&state.workspace_dir, bad, good) {
                                        Ok(out) => println!("{}", out),
                                        Err(e) => println!("Bisect error: {}", e),
                                    }
                                }
                                "run" => {
                                    let test_cmd = parts[1..].join(" ");
                                    match bisect::GitBisect::run_automated(&state.workspace_dir, &test_cmd) {
                                        Ok(out) => println!("{}", out),
                                        Err(e) => println!("Bisect run error: {}", e),
                                    }
                                }
                                "reset" => {
                                    match bisect::GitBisect::reset(&state.workspace_dir) {
                                        Ok(out) => println!("{}", out),
                                        Err(e) => println!("Bisect reset error: {}", e),
                                    }
                                }
                                _ => println!("Usage: /bisect start [bad] [good] | /bisect run <cmd> | /bisect reset"),
                            }
                            continue;
                        }
                        "/ast" => {
                            println!("Running structural AST scan across workspace...");
                            match ast_grep::AstScanner::scan_workspace(&state.workspace_dir) {
                                Ok(findings) => {
                                    if findings.is_empty() {
                                        println!(
                                            "{}",
                                            "No structural or security defects found."
                                                .bold()
                                                .green()
                                        );
                                    } else {
                                        println!(
                                            "{}",
                                            format!("Found {} AST defects:", findings.len())
                                                .bold()
                                                .yellow()
                                        );
                                        for f in findings {
                                            println!(
                                                "  {}:{} [{}] {}",
                                                f.file.cyan(),
                                                f.line,
                                                f.rule.bold().red(),
                                                f.message
                                            );
                                        }
                                    }
                                }
                                Err(e) => println!("AST scanner error: {}", e),
                            }
                            continue;
                        }
                        "/fetch" => {
                            let url = arg.trim();
                            if url.is_empty() {
                                println!("Usage: /fetch <url>");
                            } else {
                                println!(
                                    "Fetching and cleaning documentation from {}...",
                                    url.cyan()
                                );
                                match fetch::DocFetcher::fetch_and_clean(url).await {
                                    Ok(clean_doc) => {
                                        state.history.push(Message {
                                            role: "user".to_string(),
                                            content: format!(
                                                "[Fetched Documentation from {}]:\n{}",
                                                url, clean_doc
                                            ),
                                        });
                                        println!(
                                            "{}",
                                            format!(
                                                "Successfully loaded {} chars into context.",
                                                clean_doc.len()
                                            )
                                            .bold()
                                            .green()
                                        );
                                    }
                                    Err(e) => println!("Fetch error: {}", e),
                                }
                            }
                            continue;
                        }
                        "/permissions" => {
                            let parts: Vec<&str> = arg.split_whitespace().collect();
                            if parts.is_empty() {
                                println!("{}", "=== TOOL EXECUTION POLICIES ===".bold().cyan());
                                for (tool, policy) in state.tool_permissions.list_policies() {
                                    let color = match policy {
                                        security::ToolPolicy::Auto => "auto".green(),
                                        security::ToolPolicy::Ask => "ask".yellow(),
                                        security::ToolPolicy::Deny => "deny".red(),
                                    };
                                    println!("  {:20} -> {}", tool, color);
                                }
                            } else if parts.len() >= 2 && parts[0] == "set" {
                                let tool = parts[1];
                                let policy_str = parts.get(2).copied().unwrap_or("ask");
                                if let Some(policy) = security::ToolPolicy::parse_policy(policy_str)
                                {
                                    state.tool_permissions.set_policy(tool, policy);
                                    println!(
                                        "Updated tool '{}' policy to {}",
                                        tool,
                                        policy.as_str().bold()
                                    );
                                } else {
                                    println!(
                                        "Invalid policy '{}'. Options: auto, ask, deny",
                                        policy_str
                                    );
                                }
                            } else {
                                println!(
                                    "Usage: /permissions | /permissions set <tool> <auto|ask|deny>"
                                );
                            }
                            continue;
                        }
                        "/session" => {
                            let parts: Vec<&str> = arg.split_whitespace().collect();
                            match parts.first().copied().unwrap_or("list") {
                                "save" => {
                                    let id = parts.get(1).copied().unwrap_or("current");
                                    let saved_msgs: Vec<session::SavedMessage> = state
                                        .history
                                        .iter()
                                        .map(|m| session::SavedMessage {
                                            role: m.role.clone(),
                                            content: m.content.clone(),
                                        })
                                        .collect();
                                    match session::SessionManager::save_session(
                                        id,
                                        &state.config.active_model,
                                        &state.workspace_dir,
                                        saved_msgs,
                                    ) {
                                        Ok(p) => println!(
                                            "Session saved to {}",
                                            p.display().to_string().bold().green()
                                        ),
                                        Err(e) => println!("Error saving session: {}", e),
                                    }
                                }
                                "list" => match session::SessionManager::list_sessions() {
                                    Ok(sessions) => {
                                        if sessions.is_empty() {
                                            println!(
                                                "No saved sessions found in ~/.orbit/sessions/."
                                            );
                                        } else {
                                            println!("{}", "=== SAVED SESSIONS ===".bold().cyan());
                                            for (id, date, model, turns) in sessions {
                                                println!(
                                                    "  [{}] {} ({} turns, model: {})",
                                                    id.bold().yellow(),
                                                    date,
                                                    turns,
                                                    model
                                                );
                                            }
                                        }
                                    }
                                    Err(e) => println!("Error listing sessions: {}", e),
                                },
                                _ => println!("Usage: /session save [id] | /session list"),
                            }
                            continue;
                        }
                        "/resume" => {
                            let id = arg.trim();
                            if id.is_empty() {
                                println!("Usage: /resume <session_id>");
                            } else {
                                match session::SessionManager::load_session(id) {
                                    Ok(snap) => {
                                        state.history = snap
                                            .messages
                                            .into_iter()
                                            .map(|m| Message {
                                                role: m.role,
                                                content: m.content,
                                            })
                                            .collect();
                                        state.config.active_model = snap.model;
                                        println!(
                                            "{}",
                                            format!(
                                                "Resumed session '{}' with {} messages.",
                                                id,
                                                state.history.len()
                                            )
                                            .bold()
                                            .green()
                                        );
                                    }
                                    Err(e) => println!("Error resuming session: {}", e),
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

                let turn_start_time = std::time::Instant::now();

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
                                let mut parsed_tools = Vec::new();
                                for cap in tool_re.captures_iter(&response_text) {
                                    parsed_tools.push(dispatch::ParsedTool {
                                        name: cap[1].to_string(),
                                        body: cap[2].to_string(),
                                    });
                                }

                                if !parsed_tools.is_empty() {
                                    let tool_summary = parsed_tools
                                        .iter()
                                        .map(|t| t.name.as_str())
                                        .collect::<Vec<_>>()
                                        .join(", ");
                                    let mut tool_spinner = spinner::Spinner::start(format!(
                                        "Executing {} tool(s) [{}]...",
                                        parsed_tools.len(),
                                        tool_summary
                                    ));

                                    let results =
                                        dispatch::ToolDispatch::execute_batch(
                                            parsed_tools,
                                            |name, body| {
                                                let state_ref = &state;
                                                async move {
                                                    execute_tool(&name, &body, state_ref).await
                                                }
                                            },
                                        )
                                        .await;

                                    tool_spinner.stop_and_clear();

                                    for (tool_name, tool_result) in results {
                                        println!("{}\n{}", "Result:".dimmed(), tool_result);
                                        state.history.push(Message {
                                            role: "user".to_string(),
                                            content: format!(
                                                "[TOOL OUTPUT ({})]:\n{}",
                                                tool_name, tool_result
                                            ),
                                        });
                                    }

                                    println!(
                                        "{}",
                                        "🔄 Feeding tool execution results back to model...".cyan()
                                    );
                                    continue;
                                }
                            }
                            println!(
                                "{}",
                                format!(
                                    "✨ Finished in {:.1}s. Ready for input.",
                                    turn_start_time.elapsed().as_secs_f32()
                                )
                                .green()
                                .dimmed()
                            );
                            break;
                        }
                        Err(e) => {
                            println!("❌ Error: {}", e);
                            break;
                        }
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                if !multi_line_acc.is_empty() {
                    multi_line_acc.clear();
                    println!("{}", "Multi-line input canceled.".dimmed());
                    continue;
                }
                println!("Bye!");
                break;
            }
            Err(ReadlineError::Eof) => {
                if !multi_line_acc.is_empty() {
                    multi_line_acc.clear();
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_command_execution() {
        let state = AgentState::new(None).unwrap();
        let res = execute_tool_quiet("run_command", "echo 'orbit_local_ok'", &state).await;
        assert!(
            res.contains("Exit Code: 0"),
            "Expected 0 exit code: {}",
            res
        );
        assert!(
            res.contains("orbit_local_ok"),
            "Expected stdout output: {}",
            res
        );
    }

    #[tokio::test]
    async fn test_system_prompt_includes_local_execution() {
        let state = AgentState::new(None).unwrap();
        let prompt = state.system_prompt();
        assert!(prompt.contains("run_command"));
        assert!(prompt.contains("local operating system"));
    }

    #[tokio::test]
    async fn test_change_workspace_success_and_error() {
        let orig_dir = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let temp_dir_1 = std::env::temp_dir().join("orbit_test_ws_1");
        let temp_dir_2 = std::env::temp_dir().join("orbit_test_ws_2");
        let _ = std::fs::remove_dir_all(&temp_dir_1);
        let _ = std::fs::remove_dir_all(&temp_dir_2);
        std::fs::create_dir_all(&temp_dir_1).unwrap();
        std::fs::create_dir_all(&temp_dir_2).unwrap();

        let mut state = AgentState::new(Some(temp_dir_1.clone())).unwrap();
        let canonical_1 = std::fs::canonicalize(&temp_dir_1).unwrap();
        assert_eq!(state.workspace_dir, canonical_1);

        let res = state.change_workspace(&temp_dir_2).unwrap();
        let canonical_2 = std::fs::canonicalize(&temp_dir_2).unwrap();
        assert_eq!(res, canonical_2);
        assert_eq!(state.workspace_dir, canonical_2);

        let bad_path = std::env::temp_dir().join("orbit_non_existent_dir_12345");
        let err = state.change_workspace(&bad_path);
        assert!(err.is_err());

        let _ = std::env::set_current_dir(&orig_dir);
        let _ = std::fs::remove_dir_all(&temp_dir_1);
        let _ = std::fs::remove_dir_all(&temp_dir_2);
    }
}
