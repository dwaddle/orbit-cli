use crate::Message;
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelTier {
    Fast,
    Heavy,
}

impl ModelTier {
    pub fn name(&self) -> &'static str {
        match self {
            ModelTier::Fast => "Fast-Tier (Low Latency / Discovery)",
            ModelTier::Heavy => "Heavy-Tier (Reasoning / Implementation)",
        }
    }
}

/// Classifies a user prompt into Fast-Tier or Heavy-Tier based on task intent and complexity.
pub fn classify_prompt_tier(prompt: &str) -> ModelTier {
    let lower = prompt.trim().to_lowercase();

    // Fast-tier trigger patterns: exploration, search, status, documentation queries
    let fast_keywords = [
        "zoek",
        "vind",
        "waar staat",
        "toon",
        "wat doet",
        "leg uit",
        "welke bestanden",
        "status",
        "list",
        "show",
        "search",
        "find",
        "where is",
        "explain",
        "outline",
        "check",
        "git log",
        "git blame",
        "git diff",
        "inspect",
        "summarize",
        "what is",
    ];

    // Heavy-tier trigger patterns: active coding, architectural changes, refactoring, fixing
    let heavy_keywords = [
        "implementeer",
        "schrijf",
        "refactor",
        "fix",
        "los op",
        "bouw",
        "herschrijf",
        "patch",
        "modify",
        "create file",
        "write",
        "code",
        "test toevoegen",
        "add test",
        "compile",
        "debug",
    ];

    let has_heavy = heavy_keywords.iter().any(|k| lower.contains(k));
    let has_fast = fast_keywords.iter().any(|k| lower.contains(k));

    if has_heavy || lower.len() > 600 {
        ModelTier::Heavy
    } else if has_fast {
        ModelTier::Fast
    } else {
        // Default short queries to Fast if cascading
        ModelTier::Fast
    }
}

/// Constructs OpenRouter messages array with Anthropic/DeepSeek prompt caching control headers.
pub fn build_cached_messages(
    system_prompt: &str,
    history: &[Message],
    enable_cache: bool,
) -> Vec<Value> {
    let mut req_messages = Vec::new();

    if enable_cache {
        // Inject ephemeral cache_control for static system prompt, tools schema, and project rules
        req_messages.push(serde_json::json!({
            "role": "system",
            "content": [
                {
                    "type": "text",
                    "text": system_prompt,
                    "cache_control": {
                        "type": "ephemeral"
                    }
                }
            ]
        }));
    } else {
        req_messages.push(serde_json::json!({
            "role": "system",
            "content": system_prompt
        }));
    }

    let history_len = history.len();
    for (idx, m) in history.iter().enumerate() {
        // Cache large reference documents or lengthy user prompts (> 1500 chars) in the latest turns
        if enable_cache && m.role == "user" && m.content.len() > 1500 && idx + 2 >= history_len {
            req_messages.push(serde_json::json!({
                "role": &m.role,
                "content": [
                    {
                        "type": "text",
                        "text": &m.content,
                        "cache_control": {
                            "type": "ephemeral"
                        }
                    }
                ]
            }));
        } else {
            req_messages.push(serde_json::json!({
                "role": &m.role,
                "content": &m.content
            }));
        }
    }

    req_messages
}

/// Determines the candidate failover models in priority order.
pub fn get_failover_candidates(
    primary_model: &str,
    configured_fallbacks: &[String],
) -> Vec<String> {
    let mut candidates = vec![primary_model.trim().to_string()];

    for fb in configured_fallbacks {
        let trimmed = fb.trim().to_string();
        if !trimmed.is_empty() && !candidates.contains(&trimmed) {
            candidates.push(trimmed);
        }
    }

    candidates
}

/// Checks if an HTTP status code represents a transient error eligible for automatic failover.
pub fn is_retryable_status(status_code: u16) -> bool {
    matches!(
        status_code,
        429 | 500 | 502 | 503 | 504 | 520 | 521 | 522 | 524
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_prompt_tier_fast() {
        assert_eq!(
            classify_prompt_tier("zoek de main functie in src"),
            ModelTier::Fast
        );
        assert_eq!(
            classify_prompt_tier("where is AgentState defined?"),
            ModelTier::Fast
        );
        assert_eq!(
            classify_prompt_tier("leg uit wat de status van de repo is"),
            ModelTier::Fast
        );
    }

    #[test]
    fn test_classify_prompt_tier_heavy() {
        assert_eq!(
            classify_prompt_tier("implementeer prompt caching in routing.rs"),
            ModelTier::Heavy
        );
        assert_eq!(
            classify_prompt_tier("refactor de agent state en fix bug"),
            ModelTier::Heavy
        );
        assert_eq!(
            classify_prompt_tier("schrijf een complete test suite voor search index"),
            ModelTier::Heavy
        );
    }

    #[test]
    fn test_build_cached_messages_with_and_without_cache() {
        let history = vec![
            Message {
                role: "user".to_string(),
                content: "hello orbit".to_string(),
            },
            Message {
                role: "assistant".to_string(),
                content: "hi there".to_string(),
            },
        ];

        let uncached = build_cached_messages("system instructions", &history, false);
        assert_eq!(uncached.len(), 3);
        assert_eq!(uncached[0]["content"], "system instructions");

        let cached = build_cached_messages("system instructions", &history, true);
        assert_eq!(cached.len(), 3);
        assert!(cached[0]["content"].is_array());
        assert_eq!(
            cached[0]["content"][0]["cache_control"]["type"],
            "ephemeral"
        );
    }

    #[test]
    fn test_failover_candidates_order_and_dedup() {
        let primary = "anthropic/claude-sonnet-4.5";
        let fallbacks = vec![
            "openai/gpt-4o".to_string(),
            "anthropic/claude-sonnet-4.5".to_string(),
            "deepseek/deepseek-r1".to_string(),
        ];

        let candidates = get_failover_candidates(primary, &fallbacks);
        assert_eq!(candidates.len(), 3);
        assert_eq!(candidates[0], "anthropic/claude-sonnet-4.5");
        assert_eq!(candidates[1], "openai/gpt-4o");
        assert_eq!(candidates[2], "deepseek/deepseek-r1");
    }

    #[test]
    fn test_retryable_status() {
        assert!(is_retryable_status(429));
        assert!(is_retryable_status(503));
        assert!(is_retryable_status(502));
        assert!(is_retryable_status(500));
        assert!(!is_retryable_status(400));
        assert!(!is_retryable_status(401));
        assert!(!is_retryable_status(404));
    }
}
