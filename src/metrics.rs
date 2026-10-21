use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

#[derive(Serialize, Deserialize, Debug, Clone, Default)]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
    pub cached_tokens: u64,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct TurnMetrics {
    pub model: String,
    pub duration_ms: u128,
    pub ttft_ms: Option<u128>,
    pub tokens_per_sec: f64,
    pub usage: TokenUsage,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Default)]
pub struct SessionMetricsInner {
    pub total_prompt_tokens: u64,
    pub total_completion_tokens: u64,
    pub total_cached_tokens: u64,
    pub total_cost_usd: f64,
    pub total_duration_ms: u128,
    pub turns: Vec<TurnMetrics>,
}

#[derive(Debug, Clone)]
pub struct SessionMetrics {
    inner: Arc<Mutex<SessionMetricsInner>>,
}

impl Default for SessionMetrics {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionMetrics {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SessionMetricsInner::default())),
        }
    }

    pub async fn record_turn(
        &self,
        model: &str,
        usage: TokenUsage,
        duration_ms: u128,
        ttft_ms: Option<u128>,
        prompt_price_per_token: Option<f64>,
        completion_price_per_token: Option<f64>,
    ) -> TurnMetrics {
        let cost_usd = Self::calculate_cost(
            model,
            usage.prompt_tokens,
            usage.completion_tokens,
            usage.cached_tokens,
            prompt_price_per_token,
            completion_price_per_token,
        );

        let active_duration_sec = if duration_ms > 0 {
            duration_ms as f64 / 1000.0
        } else {
            1.0
        };

        let tokens_per_sec = if usage.completion_tokens > 0 {
            usage.completion_tokens as f64 / active_duration_sec
        } else {
            0.0
        };

        let turn = TurnMetrics {
            model: model.to_string(),
            duration_ms,
            ttft_ms,
            tokens_per_sec,
            usage: usage.clone(),
            cost_usd,
        };

        let mut lock = self.inner.lock().await;
        lock.total_prompt_tokens += usage.prompt_tokens;
        lock.total_completion_tokens += usage.completion_tokens;
        lock.total_cached_tokens += usage.cached_tokens;
        lock.total_cost_usd += cost_usd;
        lock.total_duration_ms += duration_ms;
        lock.turns.push(turn.clone());

        turn
    }

    pub fn calculate_cost(
        model: &str,
        prompt_tokens: u64,
        completion_tokens: u64,
        cached_tokens: u64,
        prompt_price: Option<f64>,
        completion_price: Option<f64>,
    ) -> f64 {
        let (prompt_rate, completion_rate) = match (prompt_price, completion_price) {
            (Some(p), Some(c)) => (p, c),
            _ => Self::default_model_rates(model),
        };

        // If tokens were cached, apply a standard 90% discount on cached prompt tokens
        let uncached_prompt = prompt_tokens.saturating_sub(cached_tokens);
        let cached_cost = (cached_tokens as f64) * (prompt_rate * 0.10);
        let uncached_prompt_cost = (uncached_prompt as f64) * prompt_rate;
        let completion_cost = (completion_tokens as f64) * completion_rate;

        cached_cost + uncached_prompt_cost + completion_cost
    }

    fn default_model_rates(model: &str) -> (f64, f64) {
        let m = model.to_lowercase();
        if m.contains("claude-sonnet") || m.contains("claude-3-5-sonnet") {
            (3.0 / 1_000_000.0, 15.0 / 1_000_000.0)
        } else if m.contains("claude-opus") {
            (15.0 / 1_000_000.0, 75.0 / 1_000_000.0)
        } else if m.contains("claude-haiku") {
            (0.80 / 1_000_000.0, 4.0 / 1_000_000.0)
        } else if m.contains("gemini-2.5-flash") || m.contains("gemini-flash") {
            (0.10 / 1_000_000.0, 0.40 / 1_000_000.0)
        } else if m.contains("gemini-2.5-pro") || m.contains("gemini-pro") {
            (1.25 / 1_000_000.0, 5.0 / 1_000_000.0)
        } else if m.contains("deepseek-r1") || m.contains("deepseek/deepseek-r1") {
            (0.55 / 1_000_000.0, 2.19 / 1_000_000.0)
        } else if m.contains("deepseek-chat") || m.contains("deepseek-v3") {
            (0.14 / 1_000_000.0, 0.28 / 1_000_000.0)
        } else if m.contains("gpt-4o") {
            (2.50 / 1_000_000.0, 10.0 / 1_000_000.0)
        } else if m.contains("gpt-4o-mini") {
            (0.15 / 1_000_000.0, 0.60 / 1_000_000.0)
        } else {
            // General conservative baseline ($1 / $3 per M tokens)
            (1.0 / 1_000_000.0, 3.0 / 1_000_000.0)
        }
    }

    pub async fn snapshot(&self) -> SessionMetricsInner {
        self.inner.lock().await.clone()
    }

    pub async fn format_summary(&self) -> String {
        let lock = self.inner.lock().await;
        let total_tokens = lock.total_prompt_tokens + lock.total_completion_tokens;
        let cache_ratio = if lock.total_prompt_tokens > 0 {
            (lock.total_cached_tokens as f64 / lock.total_prompt_tokens as f64) * 100.0
        } else {
            0.0
        };

        let mut out = String::new();
        out.push_str("💰 === ORBIT TOKEN & COST ECONOMICS DASHBOARD ===\n");
        out.push_str(&format!("  Turns Completed:       {}\n", lock.turns.len()));
        out.push_str(&format!(
            "  Total Tokens:          {} (Prompt: {}, Completion: {})\n",
            total_tokens, lock.total_prompt_tokens, lock.total_completion_tokens
        ));
        out.push_str(&format!(
            "  Cached Prompt Tokens:  {} ({:.1}% cache hit)\n",
            lock.total_cached_tokens, cache_ratio
        ));
        out.push_str(&format!(
            "  Total Estimated Spend: ${:.4} USD\n",
            lock.total_cost_usd
        ));
        out.push_str(&format!(
            "  Cumulative I/O Time:   {:.2}s\n",
            lock.total_duration_ms as f64 / 1000.0
        ));

        if let Some(last) = lock.turns.last() {
            out.push_str("--------------------------------------------------\n");
            out.push_str("  Last Turn Metrics:\n");
            out.push_str(&format!("    Model:          {}\n", last.model));
            out.push_str(&format!("    Duration:       {}ms\n", last.duration_ms));
            if let Some(ttft) = last.ttft_ms {
                out.push_str(&format!("    TTFT:           {}ms\n", ttft));
            }
            out.push_str(&format!(
                "    Speed:          {:.1} tokens/sec\n",
                last.tokens_per_sec
            ));
            out.push_str(&format!(
                "    Tokens:         {} (in: {}, out: {})\n",
                last.usage.total_tokens, last.usage.prompt_tokens, last.usage.completion_tokens
            ));
            out.push_str(&format!("    Turn Cost:      ${:.5} USD\n", last.cost_usd));
        }

        out
    }

    pub async fn format_badge(&self) -> String {
        let lock = self.inner.lock().await;
        let total_tokens = lock.total_prompt_tokens + lock.total_completion_tokens;
        let tok_str = if total_tokens >= 1_000_000 {
            format!("{:.1}M", total_tokens as f64 / 1_000_000.0)
        } else if total_tokens >= 1_000 {
            format!("{:.1}k", total_tokens as f64 / 1_000.0)
        } else {
            format!("{}", total_tokens)
        };

        if let Some(last) = lock.turns.last() {
            format!(
                "[📊 {} tok | ${:.4} | {:.0} t/s]",
                tok_str, lock.total_cost_usd, last.tokens_per_sec
            )
        } else {
            format!("[📊 {} tok | ${:.4}]", tok_str, lock.total_cost_usd)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_session_metrics_recording() {
        let metrics = SessionMetrics::new();
        let usage = TokenUsage {
            prompt_tokens: 1000,
            completion_tokens: 200,
            total_tokens: 1200,
            cached_tokens: 800,
        };

        let turn = metrics
            .record_turn(
                "anthropic/claude-sonnet-4.5",
                usage,
                2000,
                Some(350),
                None,
                None,
            )
            .await;

        assert_eq!(turn.model, "anthropic/claude-sonnet-4.5");
        assert_eq!(turn.ttft_ms, Some(350));
        assert!(turn.tokens_per_sec > 90.0 && turn.tokens_per_sec < 110.0);
        assert!(turn.cost_usd > 0.0);

        let snap = metrics.snapshot().await;
        assert_eq!(snap.total_prompt_tokens, 1000);
        assert_eq!(snap.total_completion_tokens, 200);
        assert_eq!(snap.total_cached_tokens, 800);
        assert_eq!(snap.turns.len(), 1);

        let summary = metrics.format_summary().await;
        assert!(summary.contains("ORBIT TOKEN & COST ECONOMICS DASHBOARD"));
        assert!(summary.contains("80.0% cache hit"));

        let badge = metrics.format_badge().await;
        assert!(badge.contains("1.2k tok"));
    }

    #[test]
    fn test_cost_calculation_with_cache_discount() {
        let cost_full = SessionMetrics::calculate_cost(
            "anthropic/claude-sonnet-4.5",
            1_000_000,
            0,
            0,
            None,
            None,
        );
        let cost_cached = SessionMetrics::calculate_cost(
            "anthropic/claude-sonnet-4.5",
            1_000_000,
            0,
            1_000_000,
            None,
            None,
        );
        // Full prompt rate is $3/M, 100% cached gets 90% discount -> $0.30/M
        assert!((cost_full - 3.0).abs() < 1e-6);
        assert!((cost_cached - 0.30).abs() < 1e-6);
    }
}
