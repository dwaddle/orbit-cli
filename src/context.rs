use crate::Message;

pub const DEFAULT_CONTEXT_LIMIT: usize = 128_000;
pub const DEFAULT_PRUNE_THRESHOLD: f32 = 0.80;

/// Fast token estimator (~3.8 chars per token for code & english prose).
pub fn estimate_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    // Blend of character count / 3.8 and whitespace word count
    let chars = text.chars().count();
    let words = text.split_whitespace().count();
    let by_chars = (chars as f32 / 3.8).ceil() as usize;
    let by_words = (words as f32 * 1.3).ceil() as usize;
    (by_chars + by_words) / 2
}

pub fn estimate_message_tokens(msg: &Message) -> usize {
    estimate_tokens(&msg.role) + estimate_tokens(&msg.content) + 4
}

pub fn estimate_total_tokens(history: &[Message], system_prompt: &str) -> usize {
    let mut total = estimate_tokens(system_prompt) + 10;
    for m in history {
        total += estimate_message_tokens(m);
    }
    total
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PruneResult {
    pub pruned: bool,
    pub tool_outputs_compressed: usize,
    pub turns_compacted: usize,
    pub tokens_before: usize,
    pub tokens_after: usize,
}

/// Prunes and compacts conversation context if usage exceeds `threshold_ratio` of `max_tokens`.
pub fn auto_prune_context(
    history: &mut Vec<Message>,
    system_prompt: &str,
    max_tokens: usize,
    threshold_ratio: f32,
) -> PruneResult {
    let tokens_before = estimate_total_tokens(history, system_prompt);
    let threshold = (max_tokens as f32 * threshold_ratio) as usize;

    if tokens_before < threshold || history.is_empty() {
        return PruneResult {
            pruned: false,
            tool_outputs_compressed: 0,
            turns_compacted: 0,
            tokens_before,
            tokens_after: tokens_before,
        };
    }

    let mut tool_outputs_compressed = 0;
    let mut turns_compacted = 0;

    // Step 1: Compress large tool outputs in earlier turns (keep last 4 messages intact)
    let preserve_recent = 4.min(history.len());
    let prune_range_end = history.len().saturating_sub(preserve_recent);

    for msg in history.iter_mut().take(prune_range_end) {
        if msg.role == "user" && msg.content.starts_with("[TOOL OUTPUT (") {
            let lines: Vec<&str> = msg.content.lines().collect();
            if lines.len() > 25 || msg.content.len() > 1000 {
                let header = lines.first().unwrap_or(&"[TOOL OUTPUT]");
                let head = lines[1..lines.len().min(6)].join("\n");
                let tail = lines[lines.len().saturating_sub(5)..].join("\n");
                let pruned_msg = format!(
                    "{}\n{}\n... [Output pruned: {} lines truncated to save context] ...\n{}",
                    header,
                    head,
                    lines.len().saturating_sub(10),
                    tail
                );
                msg.content = pruned_msg;
                tool_outputs_compressed += 1;
            }
        }
    }

    let mut current_tokens = estimate_total_tokens(history, system_prompt);

    // Step 2: Sliding-Window summarization of older turns if still above threshold
    if current_tokens >= threshold && history.len() > preserve_recent + 2 {
        let older_messages: Vec<Message> = history.drain(0..prune_range_end).collect();
        turns_compacted = older_messages.len();

        let mut summary = String::from(
            "[SLIDING-WINDOW CONVERSATION SUMMARY (Compacted earlier turns to avoid context overflow)]:\n",
        );

        for m in older_messages {
            let snippet = m.content.lines().next().unwrap_or("").trim();
            let preview = if snippet.len() > 100 {
                &snippet[..100]
            } else {
                snippet
            };
            summary.push_str(&format!("• {}: {}\n", m.role.to_uppercase(), preview));
        }

        history.insert(
            0,
            Message {
                role: "system".to_string(),
                content: summary,
            },
        );
        current_tokens = estimate_total_tokens(history, system_prompt);
    }

    PruneResult {
        pruned: tool_outputs_compressed > 0 || turns_compacted > 0,
        tool_outputs_compressed,
        turns_compacted,
        tokens_before,
        tokens_after: current_tokens,
    }
}

pub fn format_context_report(
    history: &[Message],
    system_prompt: &str,
    max_tokens: usize,
) -> String {
    let sys_tokens = estimate_tokens(system_prompt);
    let mut history_tokens = 0;
    let mut tool_tokens = 0;

    for m in history {
        let t = estimate_message_tokens(m);
        history_tokens += t;
        if m.content.starts_with("[TOOL OUTPUT") {
            tool_tokens += t;
        }
    }

    let total = sys_tokens + history_tokens;
    let limit = if max_tokens == 0 {
        DEFAULT_CONTEXT_LIMIT
    } else {
        max_tokens
    };
    let percentage = (total as f32 / limit as f32) * 100.0;
    let threshold = (limit as f32 * DEFAULT_PRUNE_THRESHOLD) as usize;

    let status = if percentage >= 80.0 {
        "⚠️ HIGH USAGE (Pruning Active)"
    } else if percentage >= 60.0 {
        "🟡 MODERATE"
    } else {
        "🟢 HEALTHY"
    };

    format!(
        "=== 📊 CONTEXT & TOKEN USAGE REPORT ===\n\
        • Status: {}\n\
        • System Prompt: ~{} tokens\n\
        • History Turns: {} messages (~{} tokens, tool output: ~{} tokens)\n\
        • Total Estimated Usage: ~{} / {} tokens ({:.1}%)\n\
        • Auto-Pruning Threshold (80%): ~{} tokens\n",
        status,
        sys_tokens,
        history.len(),
        history_tokens,
        tool_tokens,
        total,
        limit,
        percentage,
        threshold
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_token_estimator() {
        let text = "Hello world! This is a test sentence for estimating token counts.";
        let tokens = estimate_tokens(text);
        assert!(tokens >= 10 && tokens <= 25);
    }

    #[test]
    fn test_compress_large_tool_output() {
        let mut history = vec![
            Message {
                role: "user".to_string(),
                content: format!(
                    "[TOOL OUTPUT (run_command)]:\n{}",
                    (0..100)
                        .map(|i| format!("line {}", i))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            },
            Message {
                role: "assistant".to_string(),
                content: "Task completed.".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "Recent message 1".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Recent reply 1".to_string(),
            },
            Message {
                role: "user".to_string(),
                content: "Recent message 2".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "Recent reply 2".to_string(),
            },
        ];

        // Trigger with low limit
        let res = auto_prune_context(&mut history, "sys", 100, 0.10);
        assert!(res.pruned);
        assert_eq!(res.tool_outputs_compressed, 1);
        assert!(history[0].content.contains("Output pruned"));
    }

    #[test]
    fn test_sliding_window_summarization() {
        let mut history = Vec::new();
        for i in 0..10 {
            history.push(Message {
                role: "user".to_string(),
                content: format!("Question number {}", i),
            });
            history.push(Message {
                role: "assistant".to_string(),
                content: format!("Answer number {}", i),
            });
        }

        // Limit = 100 tokens, threshold = 0.50 -> will compact
        let res = auto_prune_context(&mut history, "sys", 100, 0.50);
        assert!(res.pruned);
        assert!(res.turns_compacted > 0);
        assert_eq!(history[0].role, "system");
        assert!(history[0]
            .content
            .contains("SLIDING-WINDOW CONVERSATION SUMMARY"));
    }
}
