use crate::{
    compile_guard, diff_review, export, git_context, grep, highlight, lsp, memory, outline, patch,
    repo_map, review, search_index,
    security::{self, SafeLock},
    subagent, tdd, worktree, AgentState,
};
use colored::*;
use serde_json::json;
use std::fs;
use std::path::Path;
use std::process::Command;

pub async fn execute_tool(tool_name: &str, body: &str, state: &AgentState) -> String {
    let quiet = match state.config.verbosity {
        highlight::VerbosityLevel::Quiet => true,
        highlight::VerbosityLevel::Normal | highlight::VerbosityLevel::Verbose => false,
    };
    execute_tool_internal(tool_name, body, state, quiet).await
}

pub const TOOL_REGEX_PATTERN: &str = r"```tool:(run_command|view_file|write_file|apply_patch|tdd_cycle|invoke_subagent|mcp|code_search|code_outline|git_context|adversarial_review|run_dag|code_grep|code_map|worktree_sandbox|lsp_query|export_session|store_insight)\n([\s\S]*?)```";

fn parse_view_file_target(input: &str) -> (&str, Option<usize>, Option<usize>) {
    let ws_parts: Vec<&str> = input.split_whitespace().collect();
    if ws_parts.len() >= 2 && ws_parts[1].parse::<usize>().is_ok() {
        let path = ws_parts[0];
        let start = ws_parts[1].parse::<usize>().ok();
        let end = ws_parts.get(2).and_then(|s| s.parse::<usize>().ok());
        return (path, start, end);
    }

    if let Some((path, rest)) = input.split_once(':') {
        if !path.is_empty() {
            if let Some((s_str, e_str)) = rest.split_once('-') {
                if let (Ok(s), Ok(e)) = (s_str.parse::<usize>(), e_str.parse::<usize>()) {
                    return (path, Some(s), Some(e));
                }
            } else if let Some((s_str, e_str)) = rest.split_once(':') {
                if let (Ok(s), Ok(e)) = (s_str.parse::<usize>(), e_str.parse::<usize>()) {
                    return (path, Some(s), Some(e));
                }
            } else if let Ok(s) = rest.parse::<usize>() {
                return (path, Some(s), None);
            }
        }
    }

    (input, None, None)
}

pub async fn execute_tool_quiet(tool_name: &str, body: &str, state: &AgentState) -> String {
    execute_tool_internal(tool_name, body, state, true).await
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

    // Security Tool Permission Gate
    let policy = state.tool_permissions.check_permission(tool_name);
    match policy {
        security::ToolPolicy::Deny => {
            return format!(
                "⛔ Security Policy: Execution of tool '{}' is DENIED.",
                tool_name
            );
        }
        security::ToolPolicy::Ask => {
            if !quiet {
                match security::prompt_tool_approval(tool_name) {
                    Ok(true) => {}
                    _ => return format!("Tool '{}' execution rejected by user.", tool_name),
                }
            } else {
                return format!(
                    "⛔ Security Policy: Execution of tool '{}' requires interactive confirmation (ToolPolicy::Ask). In non-interactive or quiet mode, this tool is blocked. To allow, configure tool policy to 'auto' via `/policy {} auto`.",
                    tool_name, tool_name
                );
            }
        }
        security::ToolPolicy::Auto => {}
    }

    match tool_name {
        "run_command" => {
            let cmd = body.trim();
            if let Err(e) =
                security::WorkspaceSandbox::validate_command(cmd, state.config.sandbox_mode)
            {
                return format!("Security Sandbox Error: {}", e);
            }
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
            let trimmed = body.trim();
            let (path_str, start_line_opt, end_line_opt) = parse_view_file_target(trimmed);
            let safe_path = match security::WorkspaceSandbox::sanitize_path_with_mode(
                Path::new(path_str),
                &state.workspace_dir,
                state.config.sandbox_mode,
                false,
            ) {
                Ok(p) => p,
                Err(e) => return format!("Security Sandbox Error: {}", e),
            };
            state
                .working_memory
                .lock_safe()
                .record_touchpoint(&safe_path);

            let metadata = match fs::metadata(&safe_path) {
                Ok(m) => m,
                Err(e) => return format!("Failed to read file {}: {}", safe_path.display(), e),
            };

            if !metadata.file_type().is_file() {
                return format!("Cannot view non-regular file: {}", safe_path.display());
            }

            use std::io::{BufRead, BufReader};
            let file = match fs::File::open(&safe_path) {
                Ok(f) => f,
                Err(e) => return format!("Failed to open file {}: {}", safe_path.display(), e),
            };
            let reader = BufReader::new(file);

            let start = start_line_opt.unwrap_or(1).max(1);
            let max_lines_to_return = 250usize;
            let end_limit = end_line_opt.unwrap_or(start + max_lines_to_return - 1);
            let requested_count = if end_limit >= start {
                (end_limit - start + 1).min(max_lines_to_return)
            } else {
                max_lines_to_return
            };

            let mut selected_lines = Vec::new();
            let mut current_line = 0usize;
            let mut total_lines = 0usize;

            for line_res in reader.lines() {
                current_line += 1;
                total_lines = current_line;
                if let Ok(line) = line_res {
                    if current_line >= start && selected_lines.len() < requested_count {
                        selected_lines.push(format!("{:4} | {}", current_line, line));
                    }
                }
            }

            let shown_start = if selected_lines.is_empty() { 0 } else { start };
            let shown_end = if selected_lines.is_empty() {
                0
            } else {
                start + selected_lines.len() - 1
            };

            let mut output = format!(
                "File Content ({} - lines {}-{} of {}):\n{}",
                safe_path.display(),
                shown_start,
                shown_end,
                total_lines,
                selected_lines.join("\n")
            );

            if shown_end < total_lines {
                output.push_str(&format!(
                    "\n... [Truncated: showing lines {}-{}. Use `view_file {}:{}-{}` to view more]",
                    shown_start,
                    shown_end,
                    path_str,
                    shown_end + 1,
                    (shown_end + 250).min(total_lines)
                ));
            }
            output
        }
        "write_file" => {
            let mut parts = body.trim_start().splitn(2, '\n');
            let path_str = parts.next().unwrap_or("").trim();
            let content = match parts.next() {
                Some(c) => c,
                None => {
                    return "Error: Invalid write_file payload. First line must be target path, followed by a newline and content.".to_string();
                }
            };
            if path_str.is_empty() {
                return "Error: Empty path specified for write_file".to_string();
            }

            let safe_path = match security::WorkspaceSandbox::sanitize_path_with_mode(
                Path::new(path_str),
                &state.workspace_dir,
                state.config.sandbox_mode,
                true,
            ) {
                Ok(p) => p,
                Err(e) => return format!("Security Sandbox Error: {}", e),
            };
            state
                .working_memory
                .lock_safe()
                .record_touchpoint(&safe_path);

            let path_display = safe_path.display().to_string();

            // Interactive diff approval check
            if state.config.diff_approval_required && !quiet {
                let existing = fs::read_to_string(&safe_path).unwrap_or_default();
                let diff = diff_review::generate_unified_diff(&existing, content, &path_display);
                if !diff_review::prompt_diff_approval(&diff, &path_display).unwrap_or(true) {
                    return format!("Modification to {} was rejected by user.", path_display);
                }
            }

            // Snapshot before write
            state
                .checkpoint_mgr
                .record_before_modify(&safe_path, "write_file");

            if let Some(parent) = safe_path.parent() {
                let _ = fs::create_dir_all(parent);
            }

            match fs::write(&safe_path, content) {
                Ok(_) => {
                    let mut res = format!(
                        "Successfully wrote {} bytes to {}",
                        content.len(),
                        path_display
                    );

                    // Compile Guard check
                    if state.config.compile_guard_enabled {
                        let comp = state.compile_guard.check_for_path(&safe_path);
                        if comp.command != "none" {
                            if !quiet {
                                println!("{}", compile_guard::format_compile_banner(&comp));
                            }
                            res.push_str(&format!(
                                "\n\n{}",
                                compile_guard::format_compile_banner(&comp)
                            ));
                            if !comp.success {
                                *state.last_compile_error.lock_safe() =
                                    Some(("write_file".to_string(), comp.output.clone()));
                                if state.config.self_healing_enabled {
                                    if !quiet {
                                        println!(
                                            "{}",
                                            "🩹 Compile Guard detected error. Self-healing triggered..."
                                                .yellow()
                                        );
                                    }
                                    res.push_str("\n\n[SELF-HEALING REQUIRED]: Compilation failed. Please emit a corrected ```tool:write_file or ```tool:apply_patch immediately to resolve these compiler errors.");
                                }
                            } else {
                                // Successful compilation: check if this was a self-healing resolution of a prior error
                                let prev_err = state.last_compile_error.lock_safe().take();
                                if let Some((prev_tool, err_text)) = prev_err {
                                    state.working_memory.lock_safe().record_event(
                                        memory::StmEvent::SelfHealingSuccess {
                                            tool: prev_tool,
                                            target: path_display.clone(),
                                            initial_error: err_text,
                                            resolution_strategy: "adjusted code implementation to satisfy compiler checks".to_string(),
                                        },
                                    );
                                }
                            }
                        }
                    }

                    if state.config.adversarial_review_enabled && !state.config.api_key.is_empty() {
                        if !quiet {
                            println!("{}", "🛡️ Running automatic adversarial review...".dimmed());
                        }
                        match review::AdversarialReviewer::review_code(
                            &path_display,
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
                Err(e) => format!("Failed to write file {}: {}", path_display, e),
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

            let safe_target = if !target_path.is_empty() {
                match security::WorkspaceSandbox::sanitize_path_with_mode(
                    Path::new(&target_path),
                    &state.workspace_dir,
                    state.config.sandbox_mode,
                    true,
                ) {
                    Ok(p) => p.display().to_string(),
                    Err(e) => return format!("Security Sandbox Error: {}", e),
                }
            } else {
                target_path
            };

            // Snapshot target file before patch
            if !safe_target.is_empty() {
                state
                    .checkpoint_mgr
                    .record_before_modify(Path::new(&safe_target), "apply_patch");
                state
                    .working_memory
                    .lock_safe()
                    .record_touchpoint(Path::new(&safe_target));
            }

            match patch::apply_patch_to_file_with_mode(
                &safe_target,
                &patch_payload,
                Some(&state.workspace_dir),
                Some(state.config.sandbox_mode),
            ) {
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
                            if !comp.success {
                                *state.last_compile_error.lock_safe() =
                                    Some(("apply_patch".to_string(), comp.output.clone()));
                                if state.config.self_healing_enabled {
                                    if !quiet {
                                        println!(
                                            "{}",
                                            "🩹 Compile Guard detected error. Self-healing triggered..."
                                                .yellow()
                                        );
                                    }
                                    res.push_str("\n\n[SELF-HEALING REQUIRED]: Compilation failed. Please emit a corrected ```tool:apply_patch immediately to fix these compiler errors.");
                                }
                            } else {
                                // Successful compilation: check if this was a self-healing resolution of a prior error
                                let prev_err = state.last_compile_error.lock_safe().take();
                                if let Some((prev_tool, err_text)) = prev_err {
                                    state
                                        .working_memory
                                        .lock_safe()
                                        .record_event(memory::StmEvent::SelfHealingSuccess {
                                        tool: prev_tool,
                                        target: patch_res.path.clone(),
                                        initial_error: err_text,
                                        resolution_strategy:
                                            "applied corrective patch satisfying compiler checks"
                                                .to_string(),
                                    });
                                }
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
            let maybe_dag = if trimmed.starts_with("audit") {
                let target = trimmed.strip_prefix("audit").unwrap_or("").trim();
                let target = if target.is_empty() { "." } else { target };
                Some(subagent::create_audit_dag(target))
            } else if trimmed.starts_with("feature") {
                let spec = trimmed.strip_prefix("feature").unwrap_or("").trim();
                Some(subagent::create_feature_dag(spec))
            } else {
                None
            };

            match maybe_dag {
                Some(dag) => {
                    let initial_insight_count =
                        { state.long_term_memory.lock_safe().insights.len() };
                    let rep_res = state
                        .subagent_mgr
                        .run_dag_with_memory(
                            dag,
                            &state.config.api_key,
                            Some(&state.long_term_memory),
                            Some(&state.working_memory),
                        )
                        .await;

                    let should_save = {
                        state.long_term_memory.lock_safe().insights.len() > initial_insight_count
                    };
                    if should_save {
                        let ltm_guard = state.long_term_memory.lock_safe();
                        let _ = ltm_guard.save(&state.workspace_dir);
                    }

                    match rep_res {
                        Ok(rep) => rep,
                        Err(e) => format!("DAG execution error: {}", e),
                    }
                }
                None => "Usage: run_dag audit <target> OR run_dag feature <spec>".to_string(),
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

            let mem_injection = {
                let ltm_guard = state.long_term_memory.lock_safe();
                let stm_guard = state.working_memory.lock_safe();
                let mut parts = Vec::new();
                if let Some(ltm_text) = ltm_guard.format_prompt_injection(&prompt) {
                    parts.push(ltm_text);
                }
                let scratchpad = stm_guard.format_scratchpad();
                if !scratchpad.trim().is_empty() {
                    parts.push(format!(
                        "<working_memory_scratchpad>\n{}\n</working_memory_scratchpad>",
                        scratchpad.trim()
                    ));
                }
                if parts.is_empty() {
                    None
                } else {
                    Some(parts.join("\n\n"))
                }
            };

            let is_primed = mem_injection.is_some();
            let task_id = state
                .subagent_mgr
                .spawn_subagent_with_memory(
                    role.clone(),
                    model.clone(),
                    prompt.clone(),
                    state.config.api_key.clone(),
                    mem_injection,
                )
                .await;

            let prime_badge = if is_primed {
                " (Cognitive Memory Primed)"
            } else {
                ""
            };
            format!("Subagent Task #{} spawned in background (Role: {}, Model: {}){}. Use /subagents to check progress.", task_id, role, model, prime_badge)
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
                        let mut lock = state.active_sandbox.lock_safe();
                        *lock = Some(sandbox);
                        banner
                    }
                    Err(e) => format!("Failed to create worktree sandbox: {}", e),
                },
                "diff" => {
                    let lock = state.active_sandbox.lock_safe();
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
                    let lock = state.active_sandbox.lock_safe();
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
                    let mut lock = state.active_sandbox.lock_safe();
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
        "store_insight" => {
            let mut category = memory::InsightCategory::Architecture;
            let mut keywords = Vec::new();
            let mut insight = String::new();

            for line in body.lines() {
                let trimmed = line.trim();
                if let Some(cat) = trimmed.strip_prefix("category:") {
                    match cat.trim().parse() {
                        Ok(c) => category = c,
                        Err(_) => {
                            return format!(
                                "Error: Invalid category '{}'. Valid categories: Architecture, Preference, BugPattern, Workaround, Performance.",
                                cat.trim()
                            );
                        }
                    }
                } else if let Some(kw) = trimmed.strip_prefix("keywords:") {
                    keywords = kw.split(',').map(|s| s.trim().to_string()).collect();
                } else if let Some(ins) = trimmed.strip_prefix("insight:") {
                    insight = ins.trim().to_string();
                } else if !trimmed.is_empty() && insight.is_empty() {
                    insight = trimmed.to_string();
                }
            }

            if insight.is_empty() {
                return "Error: Empty insight provided to store_insight".to_string();
            }

            if keywords.is_empty() {
                keywords.push(category.as_str().to_string());
            }

            let mut ltm = state.long_term_memory.lock_safe();
            let id = ltm.add_or_reinforce(category, keywords.clone(), &insight, 1.0);
            let _ = ltm.save(&state.workspace_dir);

            format!(
                "✅ Consolidated insight #{} [{}] ('{}') to Long-Term Memory.",
                id,
                category.as_str(),
                insight
            )
        }
        _ => format!("Unknown tool: {}", tool_name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{SandboxMode, WorkspaceSandbox};
    use regex::Regex;

    #[test]
    fn test_tool_regex_pattern() {
        let re = Regex::new(TOOL_REGEX_PATTERN).unwrap();
        let sample = "```tool:run_command\nls -la\n```";
        let caps = re.captures(sample).unwrap();
        assert_eq!(&caps[1], "run_command");
        assert_eq!(&caps[2], "ls -la\n");
    }

    #[test]
    fn test_strict_sandbox_command_validation() {
        assert!(WorkspaceSandbox::validate_command("cargo check", SandboxMode::Strict).is_ok());
        assert!(WorkspaceSandbox::validate_command("cd /etc", SandboxMode::Strict).is_err());
        assert!(WorkspaceSandbox::validate_command("cd   /", SandboxMode::Strict).is_err());
        assert!(WorkspaceSandbox::validate_command("cd   ..", SandboxMode::Strict).is_err());
        assert!(WorkspaceSandbox::validate_command("pushd /var", SandboxMode::Strict).is_err());
        assert!(WorkspaceSandbox::validate_command("ls /etc", SandboxMode::Strict).is_err());
        assert!(
            WorkspaceSandbox::validate_command("cat ~/.ssh/id_rsa", SandboxMode::Strict).is_err()
        );
        assert!(
            WorkspaceSandbox::validate_command("cat $HOME/.ssh/id_rsa", SandboxMode::Strict)
                .is_err()
        );
        assert!(
            WorkspaceSandbox::validate_command("cat /etc/passwd", SandboxMode::Strict).is_err()
        );
        assert!(
            WorkspaceSandbox::validate_command("cat ../secret.txt", SandboxMode::Strict).is_err()
        );
        // Off mode permits anything
        assert!(WorkspaceSandbox::validate_command("cat /etc/passwd", SandboxMode::Off).is_ok());
    }

    #[test]
    fn test_safelock_recovers_from_poison() {
        use std::sync::{Arc, Mutex};
        let m = Arc::new(Mutex::new(42));
        let m_clone = Arc::clone(&m);
        let _ = std::panic::catch_unwind(move || {
            let _g = m_clone.lock().unwrap();
            panic!("Intentional panic to poison mutex");
        });
        assert!(m.is_poisoned());
        let val = *m.lock_safe();
        assert_eq!(val, 42);
    }

    #[tokio::test]
    async fn test_execute_tool_strict_sandbox_guards() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let mut state = AgentState::new(Some(temp.path().to_path_buf())).unwrap();
        state.config.sandbox_mode = SandboxMode::Strict;
        state
            .tool_permissions
            .set_policy("run_command", security::ToolPolicy::Auto);

        // 1. Escaping via cd /etc blocked
        let out_cd = execute_tool("run_command", "cd /etc", &state).await;
        assert!(
            out_cd.contains("Security Sandbox Error"),
            "Expected sandbox error, got: {}",
            out_cd
        );

        // 2. Escaping via quoted paths blocked
        let out_quoted = execute_tool("run_command", "ls \"/etc\"", &state).await;
        assert!(
            out_quoted.contains("Security Sandbox Error"),
            "Expected sandbox error for quoted arg, got: {}",
            out_quoted
        );

        // 3. view_file on a directory rejected as non-regular file
        let out_dir = execute_tool("view_file", &temp.path().to_string_lossy(), &state).await;
        assert!(
            out_dir.contains("Cannot view non-regular file"),
            "Expected non-regular file error, got: {}",
            out_dir
        );

        // 4. view_file on regular file succeeds
        let test_file = temp.path().join("hello.txt");
        std::fs::write(&test_file, "hello sandbox world").unwrap();
        let out_file = execute_tool("view_file", "hello.txt", &state).await;
        assert!(
            out_file.contains("hello sandbox world"),
            "Expected file content, got: {}",
            out_file
        );

        // 5. view_file pagination and range inspection for large files
        let big_file = temp.path().join("big.txt");
        let content_300: String = (1..=300)
            .map(|i| format!("Line number {}", i))
            .collect::<Vec<_>>()
            .join("\n");
        std::fs::write(&big_file, &content_300).unwrap();

        let out_range = execute_tool("view_file", "big.txt:201-250", &state).await;
        assert!(
            out_range.contains("lines 201-250 of 300"),
            "Expected range 201-250 of 300, got: {}",
            out_range
        );
        assert!(out_range.contains("Line number 201"));
        assert!(out_range.contains("Line number 250"));
        assert!(!out_range.contains("Line number 200"));
        assert!(!out_range.contains("Line number 251"));
    }

    #[test]
    fn test_parse_view_file_target_formats() {
        assert_eq!(
            parse_view_file_target("src/main.rs"),
            ("src/main.rs", None, None)
        );
        assert_eq!(
            parse_view_file_target("src/main.rs:100:200"),
            ("src/main.rs", Some(100), Some(200))
        );
        assert_eq!(
            parse_view_file_target("src/main.rs:100-200"),
            ("src/main.rs", Some(100), Some(200))
        );
        assert_eq!(
            parse_view_file_target("src/main.rs 100 200"),
            ("src/main.rs", Some(100), Some(200))
        );
        assert_eq!(
            parse_view_file_target("src/main.rs 100"),
            ("src/main.rs", Some(100), None)
        );
    }

    #[test]
    fn test_strict_sandbox_command_flags_and_home_expansion() {
        // Flag bypass tests (cd -P /, cd -- /)
        assert!(WorkspaceSandbox::validate_command("cd -P /", SandboxMode::Strict).is_err());
        assert!(WorkspaceSandbox::validate_command("cd -- /", SandboxMode::Strict).is_err());
        assert!(WorkspaceSandbox::validate_command("cd -L /etc", SandboxMode::Strict).is_err());

        // Expanded home path test
        if let Some(home) = dirs::home_dir() {
            let sensitive_ssh = format!("cat {}/.ssh/id_rsa", home.display());
            assert!(
                WorkspaceSandbox::validate_command(&sensitive_ssh, SandboxMode::Strict).is_err()
            );
        }
    }
}
