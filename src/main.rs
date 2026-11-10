use anyhow::Result;
use colored::*;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

pub mod adr;
pub mod ast_grep;
pub mod benchmark;
pub mod bisect;
pub mod budget;
pub mod checkpoint;
pub mod ci_triage;
pub mod commands;
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
pub mod notify;
pub mod outline;
pub mod patch;
pub mod plan;
pub mod query_cache;
pub mod release_gen;
pub mod repl;
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
pub mod tools;
pub mod tui;
pub mod watcher;
pub mod worktree;

pub use security::SafeLock;
pub use tools::{execute_tool, execute_tool_quiet, TOOL_REGEX_PATTERN};

use mcp::{load_mcp_config, McpClient, McpTool};
use subagent::SubagentManager;

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
    #[serde(default)]
    pub verbosity: highlight::VerbosityLevel,
    #[serde(default)]
    pub sandbox_mode: security::SandboxMode,
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
            verbosity: highlight::VerbosityLevel::default(),
            sandbox_mode: security::SandboxMode::default(),
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
    pub initial_workspace_dir: PathBuf,
    pub active_watcher: Arc<Mutex<Option<Arc<watcher::OrbitWatcher>>>>,
    pub checkpoint_mgr: checkpoint::CheckpointManager,
    pub compile_guard: compile_guard::CompileGuard,
    pub metrics: metrics::SessionMetrics,
    pub active_sandbox: Arc<Mutex<Option<worktree::WorktreeSandbox>>>,
    pub tool_permissions: security::ToolPermissions,
    pub secret_redactor: security::SecretRedactor,
    pub query_cache: query_cache::QueryCache,
    pub project_memory: Option<memory::ProjectMemory>,
    pub working_memory: Mutex<memory::WorkingMemory>,
    pub long_term_memory: Mutex<memory::LongTermMemory>,
    pub budget_enforcer: Mutex<budget::BudgetEnforcer>,
    pub notification_mgr: Mutex<notify::TaskNotificationManager>,
    pub last_compile_error: Mutex<Option<(String, String)>>, // (tool_name, error_output)
}

impl AgentState {
    pub fn new(initial_dir: Option<PathBuf>) -> Result<Self> {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("openrouter");
        fs::create_dir_all(&config_dir)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(&config_dir) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o700);
                let _ = fs::set_permissions(&config_dir, perms);
            }
        }
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
                fs::canonicalize(&expanded).unwrap_or(expanded)
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
        let working_memory = Mutex::new(memory::WorkingMemory::new());
        let long_term_memory =
            Mutex::new(memory::LongTermMemory::load_or_init(&workspace_dir).unwrap_or_default());
        let budget_enforcer =
            Mutex::new(budget::BudgetEnforcer::new(budget::BudgetConfig::default()));
        let notification_mgr = Mutex::new(notify::TaskNotificationManager::new(
            notify::NotificationConfig::default(),
        ));
        let last_compile_error = Mutex::new(None);

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
            initial_workspace_dir: workspace_dir.clone(),
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
            working_memory,
            long_term_memory,
            budget_enforcer,
            notification_mgr,
            last_compile_error,
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
        let full_path = if expanded.is_relative() {
            self.workspace_dir.join(&expanded)
        } else {
            expanded
        };
        let canonical = fs::canonicalize(&full_path)
            .map_err(|e| anyhow::anyhow!("Directory not found `{}`: {}", new_path.display(), e))?;
        if !canonical.is_dir() {
            anyhow::bail!("Path `{}` is not a directory", canonical.display());
        }

        // Validate boundary against pinned initial_workspace_dir under Strict and Workspace modes
        match self.config.sandbox_mode {
            security::SandboxMode::Strict => {
                if !canonical.starts_with(&self.initial_workspace_dir) {
                    anyhow::bail!(
                        "Security Sandbox (Strict): Cannot change directory outside initial workspace boundary `{}`",
                        self.initial_workspace_dir.display()
                    );
                }
            }
            security::SandboxMode::Workspace => {
                self.initial_workspace_dir = canonical.clone();
            }
            security::SandboxMode::User => {
                let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("/home"));
                if let Ok(canon_home) = home.canonicalize() {
                    if !canonical.starts_with(&canon_home) {
                        anyhow::bail!(
                            "Security Sandbox (User): Cannot change directory outside home `{}`",
                            canon_home.display()
                        );
                    }
                }
                self.initial_workspace_dir = canonical.clone();
            }
            security::SandboxMode::Off => {
                self.initial_workspace_dir = canonical.clone();
            }
        }

        std::env::set_current_dir(&canonical)?;
        self.workspace_dir = canonical.clone();
        *self.code_index.lock_safe() = None;
        self.project_rules = rules::ProjectRules::discover(&self.workspace_dir);
        self.compile_guard = compile_guard::CompileGuard::new(&self.workspace_dir);
        self.project_memory = memory::ProjectMemory::load_or_init(&self.workspace_dir).ok();
        self.working_memory = Mutex::new(memory::WorkingMemory::new());
        self.long_term_memory = Mutex::new(
            memory::LongTermMemory::load_or_init(&self.workspace_dir).unwrap_or_default(),
        );
        Ok(canonical)
    }

    pub fn get_code_index(&self) -> Arc<search_index::CodeSearchIndex> {
        let mut guard = self.code_index.lock_safe();
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

    pub fn save_config(&self) -> Result<()> {
        let data = serde_json::to_string_pretty(&self.config)?;
        fs::write(&self.config_path, data)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(metadata) = fs::metadata(&self.config_path) {
                let mut perms = metadata.permissions();
                perms.set_mode(0o600);
                let _ = fs::set_permissions(&self.config_path, perms);
            }
        }
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
            ```\n\n\
            ```tool:store_insight\n\
            category: architecture | bug_pattern | user_preference | build_quirk\n\
            keywords: <comma-separated triggers>\n\
            insight: <permanent lesson or rule to remember>\n\
            ```\n\n",
            base
        );

        if let Some(rules) = &self.project_rules {
            prompt.push_str(&rules.format_for_prompt());
        }

        {
            let stm = self.working_memory.lock_safe();
            let scratchpad = stm.format_scratchpad();
            if !scratchpad.trim().is_empty() {
                prompt.push_str("\n## WORKING MEMORY (Active Task Scratchpad):\n");
                prompt.push_str(&scratchpad);
                prompt.push('\n');
            }
        }

        {
            let ltm = self.long_term_memory.lock_safe();
            let context_snippet = self
                .history
                .last()
                .map(|m| m.content.as_str())
                .unwrap_or("");
            if let Some(injection) = ltm.format_prompt_injection(context_snippet) {
                prompt.push_str("\n## LEARNED PROJECT INSIGHTS (Long-Term Memory):\n");
                prompt.push_str(&injection);
                prompt.push('\n');
            }
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
            "Never duplicate or repeat the full source code of written or modified files in your conversational markdown prose. Explain your changes in 1-2 concise bullet points; let ```tool:write_file or ```tool:apply_patch perform the file modifications directly.\nKeep all responses direct, technical, direct, and concise in GitHub markdown.",
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
            } => Some(highlight::StreamingHighlighter::with_verbosity(
                *syntax_highlighting,
                state.config.verbosity,
            )),
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
    let mut cli_verbosity: Option<highlight::VerbosityLevel> = None;
    let mut cli_sandbox: Option<security::SandboxMode> = None;
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
        } else if arg == "-q" || arg == "--quiet" {
            cli_verbosity = Some(highlight::VerbosityLevel::Quiet);
        } else if arg == "-v" || arg == "--verbose" {
            cli_verbosity = Some(highlight::VerbosityLevel::Verbose);
        } else if let Some(val) = arg.strip_prefix("--verbosity=") {
            if let Some(level) = highlight::VerbosityLevel::from_str_loose(val) {
                cli_verbosity = Some(level);
            }
        } else if arg == "--verbosity" && i + 1 < args.len() {
            if let Some(level) = highlight::VerbosityLevel::from_str_loose(&args[i + 1]) {
                cli_verbosity = Some(level);
                i += 1;
            }
        } else if let Some(val) = arg.strip_prefix("--sandbox=") {
            if let Some(mode) = security::SandboxMode::parse_mode(val) {
                cli_sandbox = Some(mode);
            }
        } else if arg == "--sandbox" && i + 1 < args.len() {
            if let Some(mode) = security::SandboxMode::parse_mode(&args[i + 1]) {
                cli_sandbox = Some(mode);
                i += 1;
            }
        } else if arg == "--no-sandbox" {
            cli_sandbox = Some(security::SandboxMode::Off);
        } else if !arg.starts_with('-') && arg != "watch" {
            let p = PathBuf::from(arg);
            if p.is_dir() {
                target_dir = Some(p);
            }
        }
        i += 1;
    }

    let mut state = AgentState::new(target_dir)?;
    let _ = std::env::set_current_dir(&state.workspace_dir);
    if let Some(v) = cli_verbosity {
        state.config.verbosity = v;
    }
    if let Some(sb) = cli_sandbox {
        state.config.sandbox_mode = sb;
    }

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
        println!("  -q, --quiet           Quiet mode (suppress full file write buffer dumps)");
        println!("  -v, --verbose         Verbose mode (full debug output)");
        println!("  --sandbox <MODE>      Filesystem sandbox mode (strict, workspace, user, off)");
        println!("  --no-sandbox          Disable filesystem sandbox (alias for --sandbox off)");
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
        "Engine: Compile Guard: {} | Self-Healing: {} | Verbosity: {} | Patching: SURGICAL (Aider-style)",
        if state.config.compile_guard_enabled {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        },
        if state.config.self_healing_enabled {
            "ENABLED".green()
        } else {
            "DISABLED".dimmed()
        },
        state.config.verbosity.as_str().to_uppercase().bold().cyan()
    );
    println!("Type `/help` for commands or `/model` to view/select models.\n");

    repl::run_repl(&mut state).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_run_command_execution() {
        let mut state = AgentState::new(Some(std::env::temp_dir())).unwrap();
        state
            .tool_permissions
            .set_policy("run_command", security::ToolPolicy::Auto);
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
    async fn test_tool_permission_gate_blocks_ask_in_quiet_mode() {
        let state = AgentState::new(Some(std::env::temp_dir())).unwrap();
        let res = execute_tool_quiet("run_command", "echo 'fail_closed'", &state).await;
        assert!(
            res.contains("requires interactive confirmation (ToolPolicy::Ask)"),
            "Expected fail-closed block in quiet mode: {}",
            res
        );
    }

    #[tokio::test]
    async fn test_system_prompt_includes_local_execution() {
        let state = AgentState::new(Some(std::env::temp_dir())).unwrap();
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

    #[tokio::test]
    async fn test_tool_permission_gate_denies_execution() {
        let mut state = AgentState::new(Some(std::env::temp_dir())).unwrap();
        state
            .tool_permissions
            .set_policy("run_command", security::ToolPolicy::Deny);

        let res = execute_tool_quiet("run_command", "echo 'should_not_run'", &state).await;
        assert!(res.contains("DENIED"), "Expected denial message: {}", res);
    }

    #[tokio::test]
    async fn test_sandbox_blocks_escaping_file_tools() {
        let temp_dir =
            std::env::temp_dir().join(format!("orbit_sandbox_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut state = AgentState::new(Some(temp_dir.clone())).unwrap();
        state
            .tool_permissions
            .set_policy("write_file", security::ToolPolicy::Auto);
        state.config.sandbox_mode = security::SandboxMode::Strict;

        // 1. view_file escaping boundary (under Strict mode)
        let view_res = execute_tool_quiet("view_file", "../../etc/passwd", &state).await;
        assert!(
            view_res.contains("Security Sandbox Error"),
            "Expected sandbox error on view_file in strict mode: {}",
            view_res
        );

        // 2. write_file escaping boundary (under Strict mode)
        let write_res = execute_tool_quiet(
            "write_file",
            "../../etc/evil.txt\nmalicious payload",
            &state,
        )
        .await;
        assert!(
            write_res.contains("Security Sandbox Error"),
            "Expected sandbox error on write_file in strict mode: {}",
            write_res
        );

        // 3. Workspace mode allows reading external readable files without sandbox error
        state.config.sandbox_mode = security::SandboxMode::Workspace;
        let view_res_ws = execute_tool_quiet("view_file", "/etc/hosts", &state).await;
        assert!(
            !view_res_ws.contains("Security Sandbox Error"),
            "Workspace mode should allow view_file on readable external paths: {}",
            view_res_ws
        );

        // 4. Off mode allows write without sandbox boundary error
        state.config.sandbox_mode = security::SandboxMode::Off;
        let dummy_out = temp_dir
            .parent()
            .unwrap()
            .join(format!("orbit_off_test_{}.txt", std::process::id()));
        let write_res_off = execute_tool_quiet(
            "write_file",
            &format!("{}\nunrestricted write", dummy_out.display()),
            &state,
        )
        .await;
        assert!(
            !write_res_off.contains("Security Sandbox Error"),
            "Off mode should not return sandbox error on writes: {}",
            write_res_off
        );
        let _ = std::fs::remove_file(&dummy_out);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_config_file_permissions_unix() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let temp_cfg =
                std::env::temp_dir().join(format!("orbit_cfg_perm_{}", std::process::id()));
            let _ = std::fs::remove_file(&temp_cfg);

            let mut state = AgentState::new(Some(std::env::temp_dir())).unwrap();
            state.config_path = temp_cfg.clone();
            state.save_config().unwrap();

            let perms = std::fs::metadata(&temp_cfg).unwrap().permissions();
            assert_eq!(perms.mode() & 0o777, 0o600);

            let _ = std::fs::remove_file(&temp_cfg);
        }
    }

    #[tokio::test]
    async fn test_store_insight_tool_and_injection() {
        let temp_dir =
            std::env::temp_dir().join(format!("orbit_memory_test_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&temp_dir);
        std::fs::create_dir_all(&temp_dir).unwrap();

        let mut state = AgentState::new(Some(temp_dir.clone())).unwrap();

        // 1. Execute store_insight tool
        let tool_input = "category: architecture\nkeywords: sqlite, pool, thread\ninsight: Use r2d2 connection pooling with max 5 connections for SQLite.";
        let res = execute_tool_quiet("store_insight", tool_input, &state).await;
        assert!(
            res.contains("Consolidated insight #1 [architecture]"),
            "Expected success result from store_insight: {}",
            res
        );

        // 2. Verify stored in LTM
        {
            let ltm = state.long_term_memory.lock_safe();
            assert_eq!(ltm.insights.len(), 1);
            assert_eq!(
                ltm.insights[0].category,
                memory::InsightCategory::Architecture
            );
        }

        // 3. Set a message context that triggers relevance match
        state.history.push(Message {
            role: "user".to_string(),
            content: "We need to connect to sqlite pool concurrently".to_string(),
        });

        // 4. Verify system prompt injection contains the insight
        let sys_prompt = state.system_prompt();
        assert!(
            sys_prompt.contains("LEARNED PROJECT INSIGHTS (Long-Term Memory)"),
            "System prompt should include LTM section: {}",
            sys_prompt
        );
        assert!(
            sys_prompt.contains("r2d2 connection pooling"),
            "System prompt should include relevant insight: {}",
            sys_prompt
        );

        // 5. Test Working Memory scratchpad injection
        {
            let mut stm = state.working_memory.lock_safe();
            stm.set_goal("Refactor SQLite database connector");
            stm.set_subtask("Implement connection pool factory");
            stm.record_attempt("direct sqlite open", Some("database is locked"), false);
        }
        let sys_prompt_stm = state.system_prompt();
        assert!(
            sys_prompt_stm.contains("WORKING MEMORY (Active Task Scratchpad)"),
            "System prompt should include STM section: {}",
            sys_prompt_stm
        );
        assert!(
            sys_prompt_stm.contains("🎯 Active Goal: Refactor SQLite database connector"),
            "Expected goal in STM scratchpad: {}",
            sys_prompt_stm
        );
        assert!(
            sys_prompt_stm.contains("database is locked"),
            "Expected failed attempt in STM scratchpad: {}",
            sys_prompt_stm
        );

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
