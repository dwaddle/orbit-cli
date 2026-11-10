use crate::{
    adr, ast_grep, benchmark, bisect, ci_triage, commit, compile_guard, context, diff_review,
    editor, export, fetch, fuzzy_picker, gh_cli, git_context, grep, highlight, lsp, memory,
    outline, plan, release_gen, repo_map, review, routing,
    security::{self, SafeLock},
    session,
    subagent::{self, SubagentStatus},
    tdd, tui, watcher, worktree, AgentState, Message,
};
use anyhow::Result;
use colored::*;
use rustyline::config::Configurer;
use rustyline::{DefaultEditor, EditMode};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandAction {
    Handled,
    Exit,
    NotACommand,
}

/// Dispatches interactive REPL slash commands (e.g. `/model`, `/help`, `/cost`, `/history`)
/// or shell execution prefixes (`!<cmd>`, `/sh`).
pub async fn handle_slash_command(
    input: &str,
    state: &mut AgentState,
    rl: &mut DefaultEditor,
) -> Result<CommandAction> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') && !trimmed.starts_with('!') {
        return Ok(CommandAction::NotACommand);
    }

    let mut parts = trimmed.splitn(2, ' ');
    let cmd = parts.next().unwrap_or("");
    let arg = parts.next().unwrap_or("");

    match cmd {
        "/exit" | "/quit" => {
            println!("Bye!");
            Ok(CommandAction::Exit)
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
            Ok(CommandAction::Handled)
        }
        "/pwd" => {
            println!(
                "Workspace: {}",
                state.workspace_dir.display().to_string().cyan()
            );
            Ok(CommandAction::Handled)
        }
        cmd if cmd.starts_with('!') || cmd == "/sh" || cmd == "/exec" => {
            let shell_cmd = if cmd.starts_with('!') {
                input.strip_prefix('!').unwrap_or("").trim()
            } else {
                arg.trim()
            };
            if shell_cmd.is_empty() {
                println!("Usage: !<command> or /sh <command>");
                return Ok(CommandAction::Handled);
            }
            if let Err(e) =
                security::WorkspaceSandbox::validate_command(shell_cmd, state.config.sandbox_mode)
            {
                println!("{} {}", "Security Sandbox Error:".red(), e);
                return Ok(CommandAction::Handled);
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
            Ok(CommandAction::Handled)
        }
        "/help" => {
            println!("{}", "=== ORBIT-CLI COMMANDS ===".bold().cyan());
            println!("  /cd <path>             - Change active workspace directory at runtime");
            println!("  /pwd                   - Print current workspace directory");
            println!(
                "  /sh <cmd> / !<cmd>     - Execute local shell command directly in workspace"
            );
            println!("  /model                 - List top models & active selection");
            println!("  /model <search|id|num> - Filter or switch active OpenRouter model");
            println!(
                "  /undo                  - Instant checkpoint rollback of last modified file"
            );
            println!("  /checkpoints           - List recent file checkpoints and snapshot stack");
            println!("  /compile / /check      - Run instant workspace compile & AST type check");
            println!(
                "  /guard                 - Toggle AST Compile Guard (auto cargo check) ON/OFF"
            );
            println!(
                "  /heal                  - Toggle compiler diagnostic self-healing loop ON/OFF"
            );
            println!(
                "  /tdd <spec>            - Autonomous Red -> Green -> Refactor test-driven loop"
            );
            println!("  /dag [audit|feature]   - Orchestrate concurrent subagent DAG workflows");
            println!("  /review [file|diff]    - Run adversarial code review on diff or file");
            println!("  /adversarial           - Toggle auto adversarial review on file writes");
            println!("  /watch [start|stop|run]- Background file watcher & debounced test loop");
            println!("  /subagents             - List active & completed subagent tasks");
            println!("  /mcp                   - List loaded MCP servers and available tools");
            println!("  /stats / /cost         - View real-time token economics, latency & spend");
            println!(
                "  /budget [amount|status|off] - Enforce session USD budget cap & auto-fallback"
            );
            println!(
                                "  /notify [status|on|off|bell] - Terminal audio bell and desktop notification on task finish"
                            );
            println!(
                                "  /release [dry-run|apply] - Conventional commits semver bump & CHANGELOG.md generator"
                            );
            println!(
                "  /ci [list|triage [run_id]] - GitHub Actions CI run inspector & failure triager"
            );
            println!("  /grep <pattern> [glob] - Blazing fast native ripgrep across codebase");
            println!(
                "  /map                   - Generate compact AST PageRank architecture skeleton"
            );
            println!("  /find <query|@symbol>  - Instant fuzzy file and symbol finder");
            println!("  /search <query>        - Search local codebase symbols and full-text");
            println!("  /outline [path]        - Generate codebase symbol map and outline");
            println!("  /git <log|blame|diff>  - Inspect git commit history, blame or diff");
            println!("  /worktree [new|diff|apply|discard] - Speculative Git worktree sandboxing");
            println!("  /diff [file]           - View current git diff or unified diff");
            println!("  /diff_guard            - Toggle interactive git diff approval ON/OFF");
            println!(
                                "  /lsp [status|hover|def|symbols] - Query local Language Server (rust-analyzer/pyright)"
                            );
            println!("  /hud                   - Toggle live token & USD cost HUD banner ON/OFF");
            println!("  /export [path]         - Export session conversation to Org-mode document");
            println!("  /rules                 - View or verify active project .orbitrules");
            println!(
                                "  /memory [note]         - View consolidated memory overview or append project note"
                            );
            println!(
                                "  /remember <insight>    - Explicitly store a rule/user preference in Long-Term Memory"
                            );
            println!(
                                "  /insights [clear]      - Inspect or clear persistent insights (.orbit/insights.json)"
                            );
            println!(
                "  /stm [goal|subtask|clear] - View or adjust Short-Term Working Memory scratchpad"
            );
            println!("  /consolidate           - Manually trigger STM -> LTM memory consolidation");
            println!("  /context               - View context token usage and budget");
            println!(
                "  /prune                 - Manually compress large tool outputs & compact turns"
            );
            println!("  /file <path>           - Attach file content into context");
            println!("  /cache                 - Toggle prompt caching headers ON/OFF");
            println!(
                                "  /sandbox [mode]        - View or configure filesystem sandbox (strict, workspace, user, off)"
                            );
            println!(
                                "  /permissions           - View or configure tool execution policies (auto, ask, deny)"
                            );
            println!(
                "  /verbosity [level]     - Set or view output verbosity (quiet, normal, verbose)"
            );
            println!("  /cascade               - Toggle hybrid model cascading ON/OFF");
            println!("  /routing               - View active routing, tiers, and failover status");
            println!("  /fallback <m1,m2,...>  - Set failover model priority chain");
            println!("  /tools                 - Toggle auto tool execution ON/OFF");
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
            println!("  \\ (at line end)        - Continue prompt on next line (multi-line input)");
            println!("  /exit                  - Exit application");
            Ok(CommandAction::Handled)
        }
        "/git" => {
            if !git_context::is_git_repo(&state.workspace_dir) {
                println!(
                    "Error: Workspace `{}` is not a git repository.",
                    state.workspace_dir.display()
                );
                return Ok(CommandAction::Handled);
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
                    match git_context::get_git_log(&state.workspace_dir, None, query.as_deref(), 15)
                    {
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
                            let r_parts: Vec<&str> = git_parts[2].split(',').collect();
                            if r_parts.len() == 2 {
                                match (r_parts[0].parse::<usize>(), r_parts[1].parse::<usize>()) {
                                    (Ok(s), Ok(e)) => Some((s, e)),
                                    _ => None,
                                }
                            } else {
                                None
                            }
                        } else {
                            None
                        };
                        match git_context::get_git_blame(&state.workspace_dir, file, range) {
                            Ok(out) => println!("{}", out),
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                }
                "diff" => match git_context::get_git_diff(&state.workspace_dir, None, false) {
                    Ok(out) => println!("{}", out),
                    Err(e) => println!("Error: {}", e),
                },
                "search" => {
                    if git_parts.len() < 2 {
                        println!("Usage: /git search <query>");
                    } else {
                        let q = git_parts[1..].join(" ");
                        match git_context::get_git_log(&state.workspace_dir, None, Some(&q), 15) {
                            Ok(out) => println!("{}", out),
                            Err(e) => println!("Error: {}", e),
                        }
                    }
                }
                _ => {
                    println!("Usage: /git <log|blame|diff|search> [args]");
                }
            }
            Ok(CommandAction::Handled)
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
                    return Ok(CommandAction::Handled);
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
                            println!("{}", worktree::format_sandbox_banner(&sandbox, "Created"));
                            let mut lock = state.active_sandbox.lock_safe();
                            *lock = Some(sandbox);
                        }
                        Err(e) => println!("Error creating sandbox: {}", e),
                    }
                }
                "diff" => {
                    let lock = state.active_sandbox.lock_safe();
                    if let Some(sandbox) = lock.as_ref() {
                        match sandbox.diff() {
                            Ok(d) => {
                                if d.trim().is_empty() {
                                    println!("No changes in active sandbox.");
                                } else {
                                    print!("{}", diff_review::format_colored_diff(&d));
                                }
                            }
                            Err(e) => println!("Error getting sandbox diff: {}", e),
                        }
                    } else {
                        println!("No active sandbox. Use `/worktree new <task>` to create one.");
                    }
                }
                "apply" => {
                    let lock = state.active_sandbox.lock_safe();
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
                    let mut lock = state.active_sandbox.lock_safe();
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
                    let active = state.active_sandbox.lock_safe();
                    if let Some(sb) = active.as_ref() {
                        println!("{}", worktree::format_sandbox_banner(sb, "Active"));
                    } else {
                        println!("No active speculative worktree sandbox attached.");
                    }
                    let all_sandboxes = worktree::list_active_sandboxes(&repo_root);
                    if !all_sandboxes.is_empty() {
                        println!("\nExisting sandboxes on disk:");
                        for sb in all_sandboxes {
                            println!("  • {}", sb.display());
                        }
                    }
                }
                _ => println!("Usage: /worktree <new <task> | diff | apply | discard | list>"),
            }
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/diff_guard" => {
            state.config.diff_approval_required = !state.config.diff_approval_required;
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
            Ok(CommandAction::Handled)
        }
        "/lsp" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            let subcmd = parts.first().copied().unwrap_or("status");
            match subcmd {
                "status" => match lsp::LspDetector::detect(&state.workspace_dir) {
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
                },
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/search" => {
            if arg.is_empty() {
                println!("Usage: /search <query>");
            } else {
                let idx = state.get_code_index();
                let results = idx.search(arg, 8);
                println!("{}", idx.format_results(arg, &results));
            }
            Ok(CommandAction::Handled)
        }
        "/outline" => {
            let target = if arg.is_empty() {
                &state.workspace_dir
            } else {
                Path::new(arg)
            };
            let outline_text = outline::generate_codebase_outline(target, 2000);
            println!("{}", outline_text);
            Ok(CommandAction::Handled)
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
                    println!("No .orbitrules or AGENTS.md found in workspace hierarchy.");
                }
            }
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
            let res =
                context::auto_prune_context(&mut state.history, &sys_prompt, active_limit, 0.0);
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
            Ok(CommandAction::Handled)
        }
        "/verbosity" => {
            let arg_clean = arg.trim();
            if arg_clean.is_empty() {
                println!("{}", "=== 📢 ORBIT VERBOSITY SETTINGS ===".bold().cyan());
                println!(
                    "Current Level: {}",
                    state.config.verbosity.as_str().bold().green()
                );
                println!("\nAvailable levels:");
                println!("  quiet   - Suppress streaming file writes & tool dumps in chat; concise status only");
                println!("  normal  - Balanced: concise file write summaries, standard command results (default)");
                println!("  verbose - Stream all raw tool payloads and full execution outputs");
                println!("\nUsage: /verbosity <quiet|normal|verbose>");
            } else if let Some(level) = highlight::VerbosityLevel::from_str_loose(arg_clean) {
                state.config.verbosity = level;
                let _ = state.save_config();
                println!("✅ Verbosity set to: {}", level.as_str().bold().green());
            } else {
                println!(
                    "{}",
                    format!(
                        "❌ Unknown verbosity level '{}'. Use: quiet, normal, verbose",
                        arg_clean
                    )
                    .red()
                );
            }
            Ok(CommandAction::Handled)
        }
        "/subagents" => {
            let tasks = state.subagent_mgr.list_subagents();
            println!("{}", "=== 🤖 SUBAGENT TASKS ===".bold().cyan());
            if tasks.is_empty() {
                println!("No subagents spawned yet.");
            } else {
                for t in tasks {
                    let status_str = match &t.status {
                        SubagentStatus::Running => "RUNNING".bold().yellow(),
                        SubagentStatus::Completed => "COMPLETED".bold().green(),
                        SubagentStatus::Failed(err) => format!("FAILED ({})", err).bold().red(),
                    };
                    println!(
                        "• Subagent #{} [{}] Status: {}{}",
                        t.id,
                        t.role.bold(),
                        status_str,
                        if t.memory_primed {
                            " 🧠 [Primed]".magenta()
                        } else {
                            "".normal()
                        }
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/model" => {
            if arg.is_empty() {
                state.list_models(None);
            } else if arg.chars().all(|c| c.is_numeric()) || arg.contains('/') {
                state.select_model(arg);
            } else {
                state.list_models(Some(arg));
            }
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/highlight" | "/syntax" => {
            state.config.syntax_highlighting = !state.config.syntax_highlighting;
            let _ = state.save_config();
            println!(
                "Real-time streaming syntax highlighting set to: {}",
                if state.config.syntax_highlighting {
                    "ENABLED (Rust, Python, TS/JS, Go, Bash, SQL, JSON, TOML, C/C++)"
                        .bold()
                        .green()
                } else {
                    "DISABLED (Raw stream)".yellow()
                }
            );
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/tui" => {
            println!("Launching Ratatui fullscreen TUI mode...");
            if let Err(e) = tui::run_tui(state).await {
                println!("TUI error: {}", e);
            }
            Ok(CommandAction::Handled)
        }
        "/clear" => {
            state.history.clear();
            println!("✅ Conversation history cleared.");
            Ok(CommandAction::Handled)
        }
        "/key" => {
            if !arg.is_empty() {
                state.config.api_key = arg.to_string();
                let _ = state.save_config();
                println!("✅ OpenRouter API key updated.");
            }
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/dag" => {
            if state.config.api_key.is_empty() {
                println!(
                    "{}",
                    "Error: OpenRouter API key is required to run DAG subagents.".red()
                );
                return Ok(CommandAction::Handled);
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
                    return Ok(CommandAction::Handled);
                }
                subagent::create_feature_dag(spec)
            } else {
                println!("Usage: /dag audit [path] OR /dag feature <specification>");
                return Ok(CommandAction::Handled);
            };

            let initial_count = { state.long_term_memory.lock_safe().insights.len() };
            let rep_res = state
                .subagent_mgr
                .run_dag_with_memory(
                    dag,
                    &state.config.api_key,
                    Some(&state.long_term_memory),
                    Some(&state.working_memory),
                )
                .await;

            let should_save = { state.long_term_memory.lock_safe().insights.len() > initial_count };
            if should_save {
                let ltm_guard = state.long_term_memory.lock_safe();
                let _ = ltm_guard.save(&state.workspace_dir);
            }

            match rep_res {
                Ok(rep) => {
                    println!("\n{}", rep);
                    state.history.push(Message {
                        role: "assistant".to_string(),
                        content: rep,
                    });
                }
                Err(e) => println!("❌ DAG execution failed: {}", e),
            }
            Ok(CommandAction::Handled)
        }
        "/review" => {
            if state.config.api_key.is_empty() {
                println!(
                    "{}",
                    "Error: OpenRouter API key is required for adversarial review.".red()
                );
                return Ok(CommandAction::Handled);
            }
            let target = arg.trim();
            let (target_name, content, notes) = if target.is_empty() || target == "diff" {
                match git_context::get_git_diff(&state.workspace_dir, None, false) {
                    Ok(diff) => {
                        if diff.trim().is_empty() {
                            println!(
                                "No uncommitted git diff found. Specify a file: `/review <path>`"
                            );
                            return Ok(CommandAction::Handled);
                        }
                        (
                            "git diff (workspace)".to_string(),
                            diff,
                            "Active workspace uncommitted changes".to_string(),
                        )
                    }
                    Err(e) => {
                        println!("Failed to read git diff: {}", e);
                        return Ok(CommandAction::Handled);
                    }
                }
            } else {
                match fs::read_to_string(target) {
                    Ok(c) => (target.to_string(), c, format!("Review file: {}", target)),
                    Err(e) => {
                        println!("Failed to read file '{}': {}", target, e);
                        return Ok(CommandAction::Handled);
                    }
                }
            };

            println!(
                "{}",
                format!("🛡️ Running adversarial review on '{}'...", target_name).dimmed()
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
            Ok(CommandAction::Handled)
        }
        "/adversarial" => {
            state.config.adversarial_review_enabled = !state.config.adversarial_review_enabled;
            let _ = state.save_config();
            println!(
                "Automatic Adversarial Review on file write: {}",
                if state.config.adversarial_review_enabled {
                    "ENABLED".bold().green()
                } else {
                    "DISABLED".yellow()
                }
            );
            Ok(CommandAction::Handled)
        }
        "/watch" => {
            let subcmd = arg.trim();
            match subcmd {
                "start" => {
                    let mut guard = state.active_watcher.lock_safe();
                    if guard.is_some() {
                        println!("Background watcher is already active.");
                    } else {
                        let watcher = Arc::new(watcher::OrbitWatcher::new(&state.workspace_dir));
                        if let Err(e) = watcher.start_background_loop() {
                            println!("Failed to start watcher: {}", e);
                        } else {
                            *guard = Some(watcher);
                        }
                    }
                }
                "stop" => {
                    let mut guard = state.active_watcher.lock_safe();
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/checkpoints" => {
            let snapshots = state.checkpoint_mgr.list_checkpoints();
            println!("{}", "=== ⏱️ CHECKPOINTS & UNDO STACK ===".bold().cyan());
            if snapshots.is_empty() {
                println!("No checkpoints recorded yet.");
            } else {
                for s in snapshots.iter().rev().take(10) {
                    println!(
                        "• Snapshot #{} [{}] {} (existed before: {})",
                        s.id,
                        s.action.yellow(),
                        s.path.display().to_string().bold(),
                        s.existed_before
                    );
                }
            }
            Ok(CommandAction::Handled)
        }
        "/guard" => {
            state.config.compile_guard_enabled = !state.config.compile_guard_enabled;
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/compile" | "/check" => {
            println!("Running workspace AST compile & type check...");
            let res = state.compile_guard.check();
            println!("{}", compile_guard::format_compile_banner(&res));
            Ok(CommandAction::Handled)
        }
        "/tdd" => {
            if state.config.api_key.is_empty() {
                println!(
                    "{}",
                    "Error: OpenRouter API key is required to run TDD cycle.".red()
                );
                return Ok(CommandAction::Handled);
            }
            let spec = arg.trim();
            if spec.is_empty() {
                println!("Usage: /tdd <feature specification>");
                return Ok(CommandAction::Handled);
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
            Ok(CommandAction::Handled)
        }
        "/stats" | "/cost" => {
            let summary = state.metrics.format_summary().await;
            println!("\n{}", summary.cyan());
            Ok(CommandAction::Handled)
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
                        println!("\n{}", grep::NativeGrep::format_results(&matches, pat, 50));
                    }
                    Err(e) => println!("❌ Error searching: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/map" => {
            match repo_map::RepoMap::generate(&state.workspace_dir, 100) {
                Ok(map) => println!("\n{}", map.cyan()),
                Err(e) => println!("❌ Error generating repo map: {}", e),
            }
            Ok(CommandAction::Handled)
        }
        "/find" => {
            let query = arg.trim();
            let matches = if let Some(stripped) = query.strip_prefix('@') {
                fuzzy_picker::FuzzyPicker::find_symbols(&state.workspace_dir, stripped, 15)
            } else {
                fuzzy_picker::FuzzyPicker::find_files(&state.workspace_dir, query, 15)
            };
            match matches {
                Ok(m) => println!("\n{}", fuzzy_picker::FuzzyPicker::format_results(&m, query)),
                Err(e) => println!("❌ Error in fuzzy search: {}", e),
            }
            Ok(CommandAction::Handled)
        }
        "/adr" => {
            let title = arg.trim();
            if title.is_empty() {
                match adr::AdrManager::list_adrs(&state.workspace_dir) {
                    Ok(list) => {
                        if list.is_empty() {
                            println!("No ADRs found in docs/adr/. Usage: /adr <Title of Decision>");
                        } else {
                            println!("{}", "=== ARCHITECTURE DECISION RECORDS ===".bold().cyan());
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
                    Ok((path, _)) => {
                        println!("Created ADR: {}", path.display().to_string().bold().green())
                    }
                    Err(e) => println!("Error creating ADR: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/commit" => {
            match commit::CommitGenerator::get_diff(&state.workspace_dir) {
                Ok(diff) => match commit::CommitGenerator::analyze_and_generate(&diff) {
                    Ok(c) => {
                        let msg = c.format();
                        println!(
                            "\n{}\n{}\n",
                            "=== SUGGESTED CONVENTIONAL COMMIT ===".bold().cyan(),
                            msg
                        );
                        println!("Run `/sh git commit -m \"...\"` or review with `/diff`.");
                    }
                    Err(e) => println!("Cannot generate commit: {}", e),
                },
                Err(e) => println!("Error getting git diff: {}", e),
            }
            Ok(CommandAction::Handled)
        }
        "/memory" => {
            let entry = arg.trim();
            if entry == "eval" || entry == "benchmark" || entry == "test" {
                println!(
                    "\n{}",
                    "Running in-depth memory evaluation and diagnostic benchmark..."
                        .cyan()
                        .bold()
                );
                let ltm_guard = state.long_term_memory.lock_safe();
                let report = memory::MemoryBenchmarkRunner::run_benchmark(Some(&ltm_guard));
                println!("\n{}", report.format_banner().green());
            } else if entry.is_empty() {
                println!("\n{}", "=== 🧠 ORBIT MEMORY ARCHITECTURE ===".bold().cyan());
                {
                    let stm = state.working_memory.lock_safe();
                    println!(
                        "\n{}",
                        "--- [SHORT-TERM WORKING MEMORY] ---".bold().yellow()
                    );
                    let scratch = stm.format_scratchpad();
                    if scratch.trim().is_empty() {
                        println!("Working memory scratchpad is empty.");
                    } else {
                        println!("{}", scratch);
                    }
                }
                {
                    let ltm = state.long_term_memory.lock_safe();
                    println!(
                        "\n{}",
                        "--- [LONG-TERM MEMORY (INSIGHTS)] ---".bold().green()
                    );
                    println!("Stored Insights: {}", ltm.insights.len());
                    for ins in ltm.insights.iter().take(10) {
                        println!(
                            "  • #{} [{}] ({:.1}) {} (used {}x)",
                            ins.id,
                            ins.category.as_str().cyan(),
                            ins.confidence,
                            ins.insight.bold(),
                            ins.occurrence_count
                        );
                    }
                    if ltm.insights.len() > 10 {
                        println!(
                            "  ... and {} more (use `/insights` to view all)",
                            ltm.insights.len() - 10
                        );
                    }
                }
                if let Some(mem) = &state.project_memory {
                    println!(
                        "\n{}\n{}",
                        "--- [PROJECT MEMORY (.orbit/memory.md)] ---"
                            .bold()
                            .magenta(),
                        mem.content
                    );
                }
            } else if let Some(mem) = &mut state.project_memory {
                match mem.append_entry("Note", entry) {
                    Ok(_) => println!("Appended to .orbit/memory.md"),
                    Err(e) => println!("Error saving memory: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/remember" => {
            let entry = arg.trim();
            if entry.is_empty() {
                println!("Usage: /remember <rule, user preference or permanent lesson>");
            } else {
                let mut ltm = state.long_term_memory.lock_safe();
                let keywords = vec!["user_preference".to_string(), "manual".to_string()];
                let id = ltm.add_or_reinforce(
                    memory::InsightCategory::UserPreference,
                    keywords,
                    entry,
                    1.0,
                );
                match ltm.save(&state.workspace_dir) {
                    Ok(_) => println!(
                        "{}",
                        format!(
                            "🧠 Remembered insight #{} [UserPreference] permanently: '{}'",
                            id, entry
                        )
                        .green()
                        .bold()
                    ),
                    Err(e) => println!("Error persisting insight: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/insights" => {
            let subcmd = arg.trim();
            let mut ltm = state.long_term_memory.lock_safe();
            if subcmd == "clear" {
                ltm.insights.clear();
                let _ = ltm.save(&state.workspace_dir);
                println!("🗑️ Cleared all long-term insights.");
            } else {
                println!(
                    "{}",
                    "=== 💡 LONG-TERM INSIGHTS (.orbit/insights.json) ==="
                        .bold()
                        .cyan()
                );
                if ltm.insights.is_empty() {
                    println!("No insights recorded yet. Store with `/remember <text>` or tool `store_insight`.");
                } else {
                    for ins in &ltm.insights {
                        println!(
                            "• #{} [{}] (conf: {:.1}, used: {}x)\n  Insight: {}\n  Keywords: {}\n",
                            ins.id,
                            ins.category.as_str().cyan().bold(),
                            ins.confidence,
                            ins.occurrence_count,
                            ins.insight,
                            ins.trigger_keywords.join(", ").dimmed()
                        );
                    }
                }
            }
            Ok(CommandAction::Handled)
        }
        "/stm" => {
            let subcmd = arg.trim();
            let mut stm = state.working_memory.lock_safe();
            if subcmd.is_empty() {
                println!("{}", "=== ⚡ SHORT-TERM WORKING MEMORY ===".bold().yellow());
                println!("{}", stm.format_scratchpad());
            } else if subcmd == "clear" {
                stm.clear_task();
                println!("🧹 Working memory scratchpad cleared.");
            } else if let Some(goal) = subcmd.strip_prefix("goal ") {
                stm.set_goal(goal.trim());
                println!(
                    "🎯 Working memory active goal set to: {}",
                    goal.trim().bold()
                );
            } else if let Some(sub) = subcmd.strip_prefix("subtask ") {
                stm.set_subtask(sub.trim());
                println!(
                    "⚡ Working memory active subtask set to: {}",
                    sub.trim().bold()
                );
            } else {
                println!("Usage: /stm | /stm clear | /stm goal <goal> | /stm subtask <subtask>");
            }
            Ok(CommandAction::Handled)
        }
        "/consolidate" => {
            let mut stm = state.working_memory.lock_safe();
            let mut ltm = state.long_term_memory.lock_safe();
            let outcomes = memory::MemoryConsolidator::consolidate(&stm, &mut ltm);
            if outcomes.is_empty() {
                let _ = stm.drain_events();
                println!("No salient events in working memory required consolidation to long-term memory.");
            } else {
                match ltm.save(&state.workspace_dir) {
                    Ok(_) => {
                        let _ = stm.drain_events();
                        println!("{}", "=== 🧠 CONSOLIDATION COMPLETED ===".bold().green());
                        for o in outcomes {
                            println!(
                                "  • Consolidated #{} [{}]: {} ({})",
                                o.insight_id,
                                o.category.as_str().cyan(),
                                o.insight_text.bold(),
                                o.reason.dimmed()
                            );
                        }
                    }
                    Err(e) => {
                        eprintln!(
                            "Failed to save long-term memory to disk: {}. Retaining events in working memory.",
                            e
                        );
                    }
                }
            }
            Ok(CommandAction::Handled)
        }
        "/plan" => {
            let goal = arg.trim();
            if goal.is_empty() {
                println!("Usage: /plan <task description>");
            } else {
                match plan::PlanGenerator::generate_plan(&state.workspace_dir, goal) {
                    Ok((path, content)) => {
                        println!(
                            "{}\n{}",
                            format!("Execution Plan generated at {}", path.display())
                                .bold()
                                .green(),
                            content
                        );
                    }
                    Err(e) => println!("Error generating plan: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/benchmark" => {
            let cmd_str = arg.trim();
            if cmd_str.is_empty() {
                println!("Usage: /benchmark <shell command>");
            } else {
                println!("Benchmarking '{}' (5 runs)...", cmd_str.bold());
                match benchmark::BenchmarkRunner::run(cmd_str, &state.workspace_dir, 5) {
                    Ok(stats) => println!("\n{}", stats.format_report()),
                    Err(e) => println!("Benchmark error: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/edit" => {
            let path_str = arg.trim();
            if path_str.is_empty() {
                println!("Usage: /edit <file_path>");
            } else {
                match editor::ExternalEditor::edit_file(Path::new(path_str), &state.workspace_dir) {
                    Ok(_) => println!("Edited file: {}", path_str),
                    Err(e) => println!("Error spawning editor: {}", e),
                }
            }
            Ok(CommandAction::Handled)
        }
        "/issue" => {
            let limit = arg.trim().parse::<usize>().unwrap_or(15);
            match gh_cli::GhCli::list_issues(&state.workspace_dir, limit) {
                Ok(res) => {
                    println!("\n{}\n{}", "=== GITHUB ISSUES ===".bold().cyan(), res)
                }
                Err(e) => println!("Error: {}", e),
            }
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
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
                "reset" => match bisect::GitBisect::reset(&state.workspace_dir) {
                    Ok(out) => println!("{}", out),
                    Err(e) => println!("Bisect reset error: {}", e),
                },
                _ => println!(
                    "Usage: /bisect start [bad] [good] | /bisect run <cmd> | /bisect reset"
                ),
            }
            Ok(CommandAction::Handled)
        }
        "/ast" => {
            println!("Running structural AST scan across workspace...");
            match ast_grep::AstScanner::scan_workspace(&state.workspace_dir) {
                Ok(findings) => {
                    if findings.is_empty() {
                        println!(
                            "{}",
                            "No structural or security defects found.".bold().green()
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
            Ok(CommandAction::Handled)
        }
        "/fetch" => {
            let url = arg.trim();
            if url.is_empty() {
                println!("Usage: /fetch <url>");
            } else {
                println!("Fetching and cleaning documentation from {}...", url.cyan());
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
            Ok(CommandAction::Handled)
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
                if let Some(policy) = security::ToolPolicy::parse_policy(policy_str) {
                    state.tool_permissions.set_policy(tool, policy);
                    println!(
                        "Updated tool '{}' policy to {}",
                        tool,
                        policy.as_str().bold()
                    );
                } else {
                    println!("Invalid policy '{}'. Options: auto, ask, deny", policy_str);
                }
            } else {
                println!("Usage: /permissions | /permissions set <tool> <auto|ask|deny>");
            }
            Ok(CommandAction::Handled)
        }
        "/sandbox" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            if parts.is_empty() {
                println!("{}", "=== ORBIT FILESYSTEM SANDBOX ===".bold().cyan());
                println!(
                    "  Active Mode : {}",
                    state.config.sandbox_mode.as_str().bold().yellow()
                );
                println!(
                    "  Description : {}",
                    state.config.sandbox_mode.description().dimmed()
                );
                println!();
                println!("  Available Modes:");
                println!("    strict    - Reads & writes locked strictly to workspace directory");
                println!(
                    "    workspace - Reads open globally, writes locked to workspace (default)"
                );
                println!("    user      - Reads & writes allowed anywhere in user's home ($HOME)");
                println!("    off       - Unrestricted filesystem access (no boundary checks)");
                println!();
                println!("  Usage: /sandbox <strict|workspace|user|off>");
            } else {
                let requested = parts[0];
                if let Some(mode) = security::SandboxMode::parse_mode(requested) {
                    state.config.sandbox_mode = mode;
                    let _ = state.save_config();
                    println!(
                        "{} {}",
                        "Updated sandbox mode to:".green(),
                        mode.as_str().bold().cyan()
                    );
                    println!("  {}", mode.description().dimmed());
                } else {
                    println!(
                        "{}",
                        format!(
                            "Invalid sandbox mode '{}'. Options: strict, workspace, user, off",
                            requested
                        )
                        .red()
                    );
                }
            }
            Ok(CommandAction::Handled)
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
                            println!("No saved sessions found in ~/.orbit/sessions/.");
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
            Ok(CommandAction::Handled)
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
            Ok(CommandAction::Handled)
        }
        "/budget" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            let sub = parts.first().copied().unwrap_or("status");
            match sub {
                "status" => {
                    let cost = state.metrics.snapshot().await.total_cost_usd;
                    println!("{}", state.budget_enforcer.lock_safe().format_status(cost));
                }
                "off" | "disable" => {
                    state.budget_enforcer.lock_safe().set_hard_cap(None);
                    println!("{}", "Budget limit removed (session unlimited).".yellow());
                }
                "fallback" => {
                    let toggle = parts.get(1).copied().unwrap_or("on");
                    let enable = toggle != "off" && toggle != "false" && toggle != "disable";
                    state
                        .budget_enforcer
                        .lock_safe()
                        .set_fallback_to_free(enable);
                    let status_str = if enable {
                        "ENABLED".green()
                    } else {
                        "DISABLED".red()
                    };
                    println!("Budget auto-fallback to free models: {}", status_str);
                }
                val => match val.parse::<f64>() {
                    Ok(amt) if amt > 0.0 => {
                        state.budget_enforcer.lock_safe().set_hard_cap(Some(amt));
                        println!(
                            "{}",
                            format!("Session budget hard cap set to ${:.4} USD.", amt)
                                .green()
                                .bold()
                        );
                    }
                    _ => {
                        println!("Usage: /budget [amount|status|off|fallback on/off] (e.g. /budget 0.50)");
                    }
                },
            }
            Ok(CommandAction::Handled)
        }
        "/notify" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            let sub = parts.first().copied().unwrap_or("status");
            match sub {
                "status" => {
                    println!("{}", state.notification_mgr.lock_safe().format_status());
                }
                "on" | "enable" => {
                    state.notification_mgr.lock_safe().set_enabled(true);
                    println!("{}", "Task notifications: ENABLED".green().bold());
                }
                "off" | "disable" => {
                    state.notification_mgr.lock_safe().set_enabled(false);
                    println!("{}", "Task notifications: DISABLED".red().bold());
                }
                "bell" => {
                    let current = state.notification_mgr.lock_safe().config().bell_enabled;
                    state.notification_mgr.lock_safe().set_bell(!current);
                    let status_str = if !current {
                        "ENABLED".green()
                    } else {
                        "DISABLED".yellow()
                    };
                    println!("Terminal bell notification: {}", status_str);
                }
                "desktop" => {
                    let current = state.notification_mgr.lock_safe().config().desktop_enabled;
                    state.notification_mgr.lock_safe().set_desktop(!current);
                    let status_str = if !current {
                        "ENABLED".green()
                    } else {
                        "DISABLED".yellow()
                    };
                    println!("Desktop notification: {}", status_str);
                }
                "threshold" => {
                    if let Some(sec_str) = parts.get(1) {
                        if let Ok(sec) = sec_str.parse::<u64>() {
                            state.notification_mgr.lock_safe().set_threshold(sec);
                            println!(
                                "{}",
                                format!("Notification duration threshold set to {} seconds.", sec)
                                    .green()
                                    .bold()
                            );
                        } else {
                            println!("Usage: /notify threshold <seconds>");
                        }
                    } else {
                        println!("Usage: /notify threshold <seconds>");
                    }
                }
                _ => {
                    println!("Usage: /notify [status|on|off|bell|desktop|threshold <seconds>]");
                }
            }
            Ok(CommandAction::Handled)
        }
        "/release" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            let action = parts.first().copied().unwrap_or("dry-run");
            match action {
                "dry-run" | "plan" | "preview" => {
                    match release_gen::ReleaseOrchestrator::plan_release(&state.workspace_dir) {
                        Ok(plan) => println!(
                            "{}",
                            release_gen::ReleaseOrchestrator::format_release_banner(&plan)
                        ),
                        Err(e) => println!("Error planning release: {}", e),
                    }
                }
                "apply" | "bump" => {
                    match release_gen::ReleaseOrchestrator::plan_release(&state.workspace_dir) {
                        Ok(plan) => {
                            if plan.bump == release_gen::SemverBump::None {
                                println!(
                                    "{}",
                                    "No version bump needed (no new commits found since last tag)."
                                        .yellow()
                                );
                            } else {
                                match release_gen::ReleaseOrchestrator::apply_release(
                                    &state.workspace_dir,
                                    &plan,
                                ) {
                                    Ok(()) => {
                                        println!(
                                                            "{}",
                                                            format!(
                                                                "Successfully bumped version to v{} in Cargo.toml and updated CHANGELOG.md!",
                                                                plan.next_version
                                                            )
                                                            .green()
                                                            .bold()
                                                        );
                                    }
                                    Err(e) => println!("Error applying release: {}", e),
                                }
                            }
                        }
                        Err(e) => println!("Error preparing release: {}", e),
                    }
                }
                _ => {
                    println!("Usage: /release [dry-run|apply]");
                }
            }
            Ok(CommandAction::Handled)
        }
        "/ci" => {
            let parts: Vec<&str> = arg.split_whitespace().collect();
            let action = parts.first().copied().unwrap_or("list");
            match action {
                "list" => match ci_triage::CiTriageManager::list_runs(&state.workspace_dir, 8) {
                    Ok(runs) => {
                        println!("{}", "=== RECENT GITHUB ACTIONS CI RUNS ===".bold().cyan());
                        for r in runs {
                            let status_badge = if r.conclusion == "success" {
                                r.conclusion.green()
                            } else if r.conclusion == "failure" {
                                r.conclusion.red().bold()
                            } else {
                                r.status.yellow()
                            };
                            println!(
                                "  #{} [{}] {} (branch: {}, commit: {})",
                                r.id,
                                status_badge,
                                r.workflow.bold(),
                                r.branch,
                                r.commit
                            );
                        }
                        println!("\nTo triage failures: /ci triage [run_id]");
                    }
                    Err(e) => println!("Error querying CI runs: {}", e),
                },
                "triage" | "log" | "failed" => {
                    let run_id = parts.get(1).copied();
                    match ci_triage::CiTriageManager::triage_failure(&state.workspace_dir, run_id) {
                        Ok(report) => println!(
                            "{}",
                            ci_triage::CiTriageManager::format_triage_banner(&report)
                        ),
                        Err(e) => println!("Error triaging CI failure: {}", e),
                    }
                }
                _ => {
                    println!("Usage: /ci [list|triage [run_id]]");
                }
            }
            Ok(CommandAction::Handled)
        }
        _ => {
            println!(
                "Unknown command {}. Type `/help` for options.",
                cmd.yellow()
            );
            Ok(CommandAction::Handled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn test_handle_slash_command_dispatch() {
        let temp = tempdir().unwrap();
        let mut state = AgentState::new(Some(temp.path().to_path_buf())).unwrap();
        let mut rl = DefaultEditor::new().unwrap();

        // 1. Regular prompt without / or ! returns NotACommand
        let res = handle_slash_command("explain this rust code", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(res, CommandAction::NotACommand);

        // 2. /exit and /quit return CommandAction::Exit
        let res_exit = handle_slash_command("/exit", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(res_exit, CommandAction::Exit);
        let res_quit = handle_slash_command("/quit", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(res_quit, CommandAction::Exit);

        // 3. /pwd returns CommandAction::Handled
        let res_pwd = handle_slash_command("/pwd", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(res_pwd, CommandAction::Handled);

        // 4. Unknown slash command returns CommandAction::Handled
        let res_unknown = handle_slash_command("/unknown_command_xyz", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(res_unknown, CommandAction::Handled);

        // 5. Shell command with strict sandbox enforcement
        state.config.sandbox_mode = security::SandboxMode::Strict;
        let res_sh_blocked = handle_slash_command("/sh cd /etc", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(res_sh_blocked, CommandAction::Handled);

        // 6. Navigation within initial_workspace_dir is permitted; escaping is rejected
        let sub = temp.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        let _ = handle_slash_command(&format!("/cd {}", sub.display()), &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(state.workspace_dir, sub.canonicalize().unwrap());

        // /cd .. navigates back to initial_workspace_dir
        let _ = handle_slash_command("/cd ..", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(state.workspace_dir, temp.path().canonicalize().unwrap());

        // /cd .. from root workspace is blocked in Strict mode
        let _ = handle_slash_command("/cd ..", &mut state, &mut rl)
            .await
            .unwrap();
        assert_eq!(state.workspace_dir, temp.path().canonicalize().unwrap());
    }
}
