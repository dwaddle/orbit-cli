use crate::{
    budget,
    commands::{self, CommandAction},
    context, dispatch, highlight, memory, modal_prompt,
    security::SafeLock,
    spinner, stream_completion,
    tools::{execute_tool, TOOL_REGEX_PATTERN},
    AgentState, Message,
};
use anyhow::Result;
use colored::*;
use regex::Regex;
use rustyline::error::ReadlineError;
use rustyline::{Config, DefaultEditor, EditMode};

/// Maximum autonomous tool-execution turns allowed per user prompt before pausing for input.
pub const MAX_TOOL_TURNS: usize = 25;

/// Interactive REPL loop managing readline input, prompt modes, continuations,
/// slash command dispatch, and multi-turn agent execution turns.
pub async fn run_repl(state: &mut AgentState) -> Result<()> {
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
                    match commands::handle_slash_command(input, state, &mut rl).await? {
                        CommandAction::Exit => {
                            println!("Bye!");
                            break;
                        }
                        CommandAction::Handled => continue,
                        CommandAction::NotACommand => {}
                    }
                }

                execute_repl_turn(input, state, &tool_re).await?;
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
                println!("Error reading line: {:?}", err);
                break;
            }
        }
    }

    let _ = rl.save_history(&state.config_path.with_file_name("history.txt"));
    Ok(())
}

async fn execute_repl_turn(input: &str, state: &mut AgentState, tool_re: &Regex) -> Result<()> {
    // Check if user input is corrective or instructive feedback for Working Memory
    let input_lower = input.trim().to_lowercase();
    let is_correction = input_lower.starts_with("no,")
        || input_lower.starts_with("no ")
        || input_lower.starts_with("nee,")
        || input_lower.starts_with("nee ")
        || input_lower.starts_with("niet zo")
        || input_lower.starts_with("don't ")
        || input_lower.starts_with("dont ")
        || input_lower.starts_with("gebruik altijd")
        || input_lower.starts_with("always use")
        || input_lower.starts_with("remember to")
        || input_lower.starts_with("onthoud dat");

    if is_correction {
        let mut stm = state.working_memory.lock_safe();
        stm.record_event(memory::StmEvent::UserCorrection {
            instruction: input.trim().to_string(),
        });
    } else {
        let mut stm = state.working_memory.lock_safe();
        if stm.current_goal.is_none() && input.trim().len() > 15 {
            stm.set_goal(input.trim());
        }
    }

    state.history.push(Message {
        role: "user".to_string(),
        content: input.to_string(),
    });

    let total_spent = state.metrics.snapshot().await.total_cost_usd;
    let verdict = state.budget_enforcer.lock_safe().evaluate(total_spent);
    match verdict {
        budget::BudgetVerdict::SoftCapWarning {
            current,
            limit,
            pct,
        } => {
            println!(
                "{}",
                format!(
                    "⚠️  BUDGET WARNING: Session spent ${:.4} USD ({:.1}% of ${:.4} cap).",
                    current, pct, limit
                )
                .yellow()
                .bold()
            );
        }
        budget::BudgetVerdict::HardCapExceeded {
            current,
            limit,
            switch_to_free,
            fallback_model,
        } => {
            println!(
                "{}",
                format!(
                    "🛑 BUDGET CAP EXCEEDED: Session spent ${:.4} USD >= limit ${:.4} USD.",
                    current, limit
                )
                .red()
                .bold()
            );
            if switch_to_free {
                println!(
                    "{}",
                    format!(
                        "🔄 Auto-switching active model to free fallback: {}",
                        fallback_model
                    )
                    .yellow()
                    .bold()
                );
                state.config.active_model = fallback_model;
            } else {
                println!(
                    "{}",
                    "Halting turn. Increase budget with `/budget <amount>` or `/budget off`.".red()
                );
                return Ok(());
            }
        }
        budget::BudgetVerdict::Ok => {}
    }

    let turn_start_time = std::time::Instant::now();
    let mut tool_turn_count = 0;

    loop {
        tool_turn_count += 1;
        if tool_turn_count > MAX_TOOL_TURNS {
            let ceiling_msg = format!(
                "🛑 Reached maximum autonomous tool turn ceiling ({} turns). Pausing turn for user input.",
                MAX_TOOL_TURNS
            );
            println!("{}", ceiling_msg.yellow().bold());
            if state
                .history
                .last()
                .map(|m| m.role == "user")
                .unwrap_or(false)
            {
                state.history.push(Message {
                    role: "assistant".to_string(),
                    content: ceiling_msg,
                });
            }
            break;
        }
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

        match stream_completion(state).await {
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
                            dispatch::ToolDispatch::execute_batch(parsed_tools, |name, body| {
                                let state_ref = &state;
                                async move { execute_tool(&name, &body, state_ref).await }
                            })
                            .await;

                        tool_spinner.stop_and_clear();

                        let mut combined_output = String::new();
                        for (tool_name, tool_result) in results {
                            let redacted_result = state.secret_redactor.redact(&tool_result);
                            match state.config.verbosity {
                                highlight::VerbosityLevel::Quiet => {
                                    let first_line =
                                        redacted_result.lines().next().unwrap_or("Done").trim();
                                    println!("  {} {}", "✓".green().bold(), first_line.dimmed());
                                }
                                highlight::VerbosityLevel::Normal => {
                                    if tool_name == "write_file" || tool_name == "apply_patch" {
                                        let first_line =
                                            redacted_result.lines().next().unwrap_or("Done").trim();
                                        println!("  {} {}", "✓".green().bold(), first_line);
                                    } else {
                                        println!("{}\n{}", "Result:".dimmed(), redacted_result);
                                    }
                                }
                                highlight::VerbosityLevel::Verbose => {
                                    println!("{}\n{}", "Result:".dimmed(), redacted_result);
                                }
                            }
                            combined_output.push_str(&format!(
                                "[TOOL OUTPUT ({})]:\n{}\n\n",
                                tool_name, redacted_result
                            ));
                        }
                        if !combined_output.is_empty() {
                            state.history.push(Message {
                                role: "user".to_string(),
                                content: combined_output.trim_end().to_string(),
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
                let dur_secs = turn_start_time.elapsed().as_secs();
                state.notification_mgr.lock_safe().notify_if_exceeded(
                    "Turn Execution",
                    dur_secs,
                    true,
                    Some("Ready for next command."),
                );

                // Autonomous memory consolidation check at turn completion
                let consolidation_needed =
                    !state.working_memory.lock_safe().observed_events.is_empty();

                if consolidation_needed {
                    let mut stm = state.working_memory.lock_safe();
                    let mut ltm = state.long_term_memory.lock_safe();
                    let outcomes = memory::MemoryConsolidator::consolidate(&stm, &mut ltm);

                    if !outcomes.is_empty() {
                        if ltm.save(&state.workspace_dir).is_ok() {
                            let _ = stm.drain_events();
                            for o in outcomes {
                                println!(
                                    "{}",
                                    format!(
                                        "💡 [Auto-Consolidated LTM #{}] [{}]: {}",
                                        o.insight_id,
                                        o.category.as_str(),
                                        o.insight_text
                                    )
                                    .green()
                                    .bold()
                                );
                            }
                        }
                    } else {
                        let _ = stm.drain_events();
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
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_repl_max_tool_turns_constant() {
        assert_eq!(MAX_TOOL_TURNS, 25);
    }

    #[test]
    fn test_repl_stm_drain_invariants() {
        let temp = tempdir().unwrap();
        let state = AgentState::new(Some(temp.path().to_path_buf())).unwrap();

        // 1. Initially empty STM
        assert!(state.working_memory.lock_safe().observed_events.is_empty());

        // 2. Add an event
        state
            .working_memory
            .lock_safe()
            .record_event(memory::StmEvent::UserCorrection {
                instruction: "gebruik altijd cargo fmt voor commits".to_string(),
            });
        assert_eq!(state.working_memory.lock_safe().observed_events.len(), 1);

        // 3. Consolidate and verify event retention if save succeeds
        let mut stm = state.working_memory.lock_safe();
        let mut ltm = state.long_term_memory.lock_safe();
        let outcomes = memory::MemoryConsolidator::consolidate(&stm, &mut ltm);
        assert!(!outcomes.is_empty());
        if ltm.save(&state.workspace_dir).is_ok() {
            let _ = stm.drain_events();
        }
        assert!(stm.observed_events.is_empty());
    }

    #[test]
    fn test_repl_message_alternation_invariant() {
        // Verify that consecutive user messages are not added when combining tool outputs
        let mut history: Vec<Message> = Vec::new();
        history.push(Message {
            role: "user".to_string(),
            content: "run tools".to_string(),
        });
        history.push(Message {
            role: "assistant".to_string(),
            content: "```tool:run_command\nls\n```\n```tool:run_command\npwd\n```".to_string(),
        });

        // Simulating combined tool output batch
        let results = vec![("run_command", "file1.txt"), ("run_command", "/workspace")];
        let mut combined_output = String::new();
        for (name, out) in results {
            combined_output.push_str(&format!("[TOOL OUTPUT ({})]:\n{}\n\n", name, out));
        }
        history.push(Message {
            role: "user".to_string(),
            content: combined_output.trim_end().to_string(),
        });

        // Invariant: last message is user, previous is assistant, before is user
        assert_eq!(history.len(), 3);
        assert_eq!(history[0].role, "user");
        assert_eq!(history[1].role, "assistant");
        assert_eq!(history[2].role, "user");
    }
}
