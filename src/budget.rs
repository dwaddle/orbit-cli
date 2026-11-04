use colored::Colorize;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetConfig {
    pub hard_cap_usd: Option<f64>,
    pub soft_cap_pct: f64, // e.g. 0.80 for 80%
    pub fallback_to_free: bool,
    pub free_fallback_model: String,
}

impl Default for BudgetConfig {
    fn default() -> Self {
        Self {
            hard_cap_usd: None,
            soft_cap_pct: 0.80,
            fallback_to_free: true,
            free_fallback_model: "meta-llama/llama-3.3-70b-instruct:free".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BudgetVerdict {
    Ok,
    SoftCapWarning {
        current: f64,
        limit: f64,
        pct: f64,
    },
    HardCapExceeded {
        current: f64,
        limit: f64,
        switch_to_free: bool,
        fallback_model: String,
    },
}

#[derive(Debug, Clone)]
pub struct BudgetEnforcer {
    config: BudgetConfig,
    soft_cap_warned: bool,
}

impl BudgetEnforcer {
    pub fn new(config: BudgetConfig) -> Self {
        Self {
            config,
            soft_cap_warned: false,
        }
    }

    pub fn config(&self) -> &BudgetConfig {
        &self.config
    }

    pub fn set_hard_cap(&mut self, cap: Option<f64>) {
        self.config.hard_cap_usd = cap;
        self.soft_cap_warned = false;
    }

    pub fn set_fallback_to_free(&mut self, enable: bool) {
        self.config.fallback_to_free = enable;
    }

    pub fn evaluate(&mut self, current_cost: f64) -> BudgetVerdict {
        let hard_cap = match self.config.hard_cap_usd {
            Some(cap) if cap > 0.0 => cap,
            _ => return BudgetVerdict::Ok,
        };

        if current_cost >= hard_cap {
            return BudgetVerdict::HardCapExceeded {
                current: current_cost,
                limit: hard_cap,
                switch_to_free: self.config.fallback_to_free,
                fallback_model: self.config.free_fallback_model.clone(),
            };
        }

        let soft_cap = hard_cap * self.config.soft_cap_pct;
        if current_cost >= soft_cap && !self.soft_cap_warned {
            self.soft_cap_warned = true;
            return BudgetVerdict::SoftCapWarning {
                current: current_cost,
                limit: hard_cap,
                pct: (current_cost / hard_cap) * 100.0,
            };
        }

        BudgetVerdict::Ok
    }

    pub fn format_status(&self, current_cost: f64) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}\n",
            "=== SESSION BUDGET STATUS ===".bold().cyan()
        ));
        out.push_str(&format!("  Current Spent:    ${:.4} USD\n", current_cost));
        match self.config.hard_cap_usd {
            Some(cap) => {
                let pct = (current_cost / cap) * 100.0;
                let soft = cap * self.config.soft_cap_pct;
                let bar = render_progress_bar(current_cost, cap, 30);
                out.push_str(&format!(
                    "  Budget Limit:     ${:.4} USD ({:.1}% consumed)\n",
                    cap, pct
                ));
                out.push_str(&format!(
                    "  Soft Cap Alert:   ${:.4} USD ({:.0}%)\n",
                    soft,
                    self.config.soft_cap_pct * 100.0
                ));
                out.push_str(&format!("  Budget Bar:       [{}]\n", bar));
                out.push_str(&format!(
                    "  Auto Free Failover: {} ({})\n",
                    if self.config.fallback_to_free {
                        "ENABLED".green()
                    } else {
                        "DISABLED".yellow()
                    },
                    self.config.free_fallback_model
                ));
            }
            None => {
                out.push_str("  Budget Limit:     UNLIMITED (No hard cap set)\n");
                out.push_str("  Set limit with:   /budget <amount_usd> (e.g. /budget 0.50)\n");
            }
        }
        out
    }
}

fn render_progress_bar(current: f64, max: f64, width: usize) -> String {
    if max <= 0.0 {
        return "-".repeat(width);
    }
    let ratio = (current / max).clamp(0.0, 1.0);
    let filled = (ratio * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);

    let filled_str = "=".repeat(filled);
    let empty_str = " ".repeat(empty);

    if ratio >= 1.0 {
        format!("{}", filled_str.red().bold())
    } else if ratio >= 0.8 {
        format!("{}{}", filled_str.yellow().bold(), empty_str)
    } else {
        format!("{}{}", filled_str.green().bold(), empty_str)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_budget_unlimited_default() {
        let mut enforcer = BudgetEnforcer::new(BudgetConfig::default());
        assert_eq!(enforcer.evaluate(10.0), BudgetVerdict::Ok);
    }

    #[test]
    fn test_budget_soft_cap_and_hard_cap() {
        let config = BudgetConfig {
            hard_cap_usd: Some(1.00),
            soft_cap_pct: 0.80,
            fallback_to_free: true,
            ..Default::default()
        };

        let mut enforcer = BudgetEnforcer::new(config);

        // Under soft cap
        assert_eq!(enforcer.evaluate(0.50), BudgetVerdict::Ok);

        // Hits soft cap at 0.80
        match enforcer.evaluate(0.85) {
            BudgetVerdict::SoftCapWarning {
                current,
                limit,
                pct,
            } => {
                assert_eq!(current, 0.85);
                assert_eq!(limit, 1.00);
                assert!((pct - 85.0).abs() < 1e-4);
            }
            _ => panic!("Expected SoftCapWarning"),
        }

        // Soft cap warning only fires once
        assert_eq!(enforcer.evaluate(0.90), BudgetVerdict::Ok);

        // Exceeds hard cap at 1.05
        match enforcer.evaluate(1.05) {
            BudgetVerdict::HardCapExceeded {
                current,
                limit,
                switch_to_free,
                fallback_model,
            } => {
                assert_eq!(current, 1.05);
                assert_eq!(limit, 1.00);
                assert!(switch_to_free);
                assert_eq!(fallback_model, "meta-llama/llama-3.3-70b-instruct:free");
            }
            _ => panic!("Expected HardCapExceeded"),
        }
    }

    #[test]
    fn test_render_progress_bar() {
        let bar_half = render_progress_bar(0.5, 1.0, 10);
        assert!(!bar_half.is_empty());
        let bar_full = render_progress_bar(1.2, 1.0, 10);
        assert!(!bar_full.is_empty());
    }
}
