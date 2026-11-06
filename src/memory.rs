use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Categories for Long-Term Memory (LTM) insights.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InsightCategory {
    Architecture,
    BugPattern,
    UserPreference,
    BuildQuirk,
}

impl InsightCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Architecture => "architecture",
            Self::BugPattern => "bug_pattern",
            Self::UserPreference => "user_preference",
            Self::BuildQuirk => "build_quirk",
        }
    }
}

impl std::str::FromStr for InsightCategory {
    type Err = ();
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().trim() {
            "architecture" | "arch" => Ok(Self::Architecture),
            "bug_pattern" | "bug" | "fix" => Ok(Self::BugPattern),
            "user_preference" | "user" | "pref" => Ok(Self::UserPreference),
            "build_quirk" | "build" | "quirk" => Ok(Self::BuildQuirk),
            _ => Err(()),
        }
    }
}

/// A single persistent insight in Long-Term Memory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct InsightRecord {
    pub id: usize,
    pub category: InsightCategory,
    pub trigger_keywords: Vec<String>,
    pub insight: String,
    pub confidence: f32,
    pub occurrence_count: u32,
    pub created_at: u64,
}

/// Persistent Long-Term Memory (LTM) store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongTermMemory {
    pub insights: Vec<InsightRecord>,
    next_id: usize,
}

impl Default for LongTermMemory {
    fn default() -> Self {
        Self {
            insights: Vec::new(),
            next_id: 1,
        }
    }
}

impl LongTermMemory {
    pub fn memory_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".orbit").join("insights.json")
    }

    pub fn load_or_init(workspace_root: &Path) -> Result<Self> {
        let file_path = Self::memory_path(workspace_root);
        if file_path.exists() {
            let data = fs::read_to_string(&file_path)?;
            let mut ltm: Self = serde_json::from_str(&data).unwrap_or_default();
            if ltm.next_id == 0 {
                ltm.next_id = ltm.insights.iter().map(|i| i.id).max().unwrap_or(0) + 1;
            }
            Ok(ltm)
        } else {
            let default_ltm = Self {
                insights: Vec::new(),
                next_id: 1,
            };
            default_ltm.save(workspace_root)?;
            Ok(default_ltm)
        }
    }

    pub fn save(&self, workspace_root: &Path) -> Result<()> {
        let file_path = Self::memory_path(workspace_root);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(&file_path, data)?;
        Ok(())
    }

    pub fn add_or_reinforce(
        &mut self,
        category: InsightCategory,
        trigger_keywords: Vec<String>,
        insight_text: &str,
        confidence: f32,
    ) -> usize {
        let text_clean = insight_text.trim();
        if let Some(existing) = self.insights.iter_mut().find(|i| {
            i.category == category
                && (i.insight.eq_ignore_ascii_case(text_clean)
                    || i.insight.contains(text_clean)
                    || text_clean.contains(&i.insight))
        }) {
            existing.occurrence_count += 1;
            existing.confidence = (existing.confidence + confidence).min(1.0);
            for kw in trigger_keywords {
                let kw_clean = kw.to_lowercase().trim().to_string();
                if !kw_clean.is_empty() && !existing.trigger_keywords.iter().any(|k| k == &kw_clean)
                {
                    existing.trigger_keywords.push(kw_clean);
                }
            }
            return existing.id;
        }

        let id = self.next_id;
        self.next_id += 1;

        let clean_keywords: Vec<String> = trigger_keywords
            .into_iter()
            .map(|k| k.to_lowercase().trim().to_string())
            .filter(|k| !k.is_empty())
            .collect();

        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        self.insights.push(InsightRecord {
            id,
            category,
            trigger_keywords: clean_keywords,
            insight: text_clean.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            occurrence_count: 1,
            created_at: ts,
        });

        id
    }

    pub fn find_relevant_insights(&self, query: &str, max_results: usize) -> Vec<&InsightRecord> {
        let lower_query = query.to_lowercase();
        let query_tokens: HashSet<&str> = lower_query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|t| t.len() >= 3)
            .collect();

        let mut scored: Vec<(&InsightRecord, usize)> = self
            .insights
            .iter()
            .filter_map(|rec| {
                let mut score = 0;
                for kw in &rec.trigger_keywords {
                    if lower_query.contains(kw) || query_tokens.contains(kw.as_str()) {
                        score += 3;
                    }
                }
                for token in &query_tokens {
                    if rec.insight.to_lowercase().contains(token) {
                        score += 1;
                    }
                }
                if score > 0 {
                    Some((rec, score + (rec.occurrence_count as usize)))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by_key(|a| std::cmp::Reverse(a.1));
        scored
            .into_iter()
            .take(max_results)
            .map(|(r, _)| r)
            .collect()
    }

    pub fn format_prompt_injection(&self, query: &str) -> Option<String> {
        let relevant = self.find_relevant_insights(query, 5);
        if relevant.is_empty() {
            None
        } else {
            let mut out = String::from("<learned_project_insights>\n");
            for rec in relevant {
                out.push_str(&format!(
                    "- [{}] {}: {} (reinforcements: {})\n",
                    rec.category.as_str(),
                    rec.trigger_keywords.join("/"),
                    rec.insight,
                    rec.occurrence_count
                ));
            }
            out.push_str("</learned_project_insights>");
            Some(out)
        }
    }
}

/// Short-Term Working Memory (STM) tracking current task state, failed attempts, and observations.
#[derive(Debug, Clone, Default)]
pub struct WorkingMemory {
    pub current_goal: Option<String>,
    pub current_subtask: Option<String>,
    pub failed_attempts: Vec<FailedAttempt>,
    pub recent_touchpoints: Vec<PathBuf>,
    pub observed_events: Vec<StmEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FailedAttempt {
    pub action: String,
    pub error_summary: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StmEvent {
    ToolExecution {
        tool: String,
        target: String,
        succeeded: bool,
        error: Option<String>,
    },
    UserCorrection {
        instruction: String,
    },
    SelfHealingSuccess {
        tool: String,
        target: String,
        initial_error: String,
        resolution_strategy: String,
    },
}

impl WorkingMemory {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_goal(&mut self, goal: &str) {
        self.current_goal = Some(goal.trim().to_string());
    }

    pub fn set_subtask(&mut self, subtask: &str) {
        self.current_subtask = Some(subtask.trim().to_string());
    }

    pub fn record_attempt(&mut self, action: &str, error: Option<&str>, succeeded: bool) {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if !succeeded {
            if let Some(err) = error {
                let first_line = err.lines().next().unwrap_or(err).trim();
                let summary = if first_line.len() > 120 {
                    format!("{}...", &first_line[..120])
                } else {
                    first_line.to_string()
                };
                self.failed_attempts.push(FailedAttempt {
                    action: action.to_string(),
                    error_summary: summary,
                    timestamp: ts,
                });
            }
        }
    }

    pub fn record_touchpoint(&mut self, path: &Path) {
        let pb = path.to_path_buf();
        if !self.recent_touchpoints.contains(&pb) {
            self.recent_touchpoints.push(pb);
            if self.recent_touchpoints.len() > 10 {
                self.recent_touchpoints.remove(0);
            }
        }
    }

    pub fn record_event(&mut self, event: StmEvent) {
        self.observed_events.push(event);
    }

    pub fn clear_task(&mut self) {
        self.current_goal = None;
        self.current_subtask = None;
        self.failed_attempts.clear();
        self.observed_events.clear();
    }

    pub fn format_scratchpad(&self) -> String {
        let mut out = String::new();
        if let Some(ref goal) = self.current_goal {
            out.push_str(&format!("🎯 Active Goal: {}\n", goal));
        }
        if let Some(ref subtask) = self.current_subtask {
            out.push_str(&format!("  ⚡ Current Subtask: {}\n", subtask));
        }
        if !self.failed_attempts.is_empty() {
            out.push_str("  ⚠️ Previous Failed Attempts in this Session:\n");
            for att in &self.failed_attempts {
                out.push_str(&format!(
                    "    • {} failed: {}\n",
                    att.action, att.error_summary
                ));
            }
        }
        if !self.recent_touchpoints.is_empty() {
            let files: Vec<String> = self
                .recent_touchpoints
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            out.push_str(&format!("  📁 Touchpoints: {}\n", files.join(", ")));
        }
        out
    }
}

/// The bridge that consolidates Salient STM events into LTM insights.
pub struct MemoryConsolidator;

#[derive(Debug, Clone, PartialEq)]
pub struct ConsolidationOutcome {
    pub insight_id: usize,
    pub category: InsightCategory,
    pub insight_text: String,
    pub reason: String,
}

impl MemoryConsolidator {
    /// Consolidates salient learning events from Working Memory into Long-Term Memory.
    pub fn consolidate(stm: &WorkingMemory, ltm: &mut LongTermMemory) -> Vec<ConsolidationOutcome> {
        let mut outcomes = Vec::new();

        for event in &stm.observed_events {
            match event {
                StmEvent::UserCorrection { instruction } => {
                    let clean = instruction.trim();
                    if clean.len() >= 10 {
                        let keywords = Self::extract_salient_keywords(clean);
                        let id = ltm.add_or_reinforce(
                            InsightCategory::UserPreference,
                            keywords,
                            clean,
                            1.0,
                        );
                        outcomes.push(ConsolidationOutcome {
                            insight_id: id,
                            category: InsightCategory::UserPreference,
                            insight_text: clean.to_string(),
                            reason: "Learned directly from user correction".to_string(),
                        });
                    }
                }
                StmEvent::SelfHealingSuccess {
                    tool,
                    target,
                    initial_error,
                    resolution_strategy,
                } => {
                    if Self::is_systemic_error(initial_error) {
                        let insight_text = format!(
                            "When encountering '{}' during {}, resolve by {}",
                            Self::summarize_error(initial_error),
                            tool,
                            resolution_strategy.trim()
                        );
                        let mut keywords = Self::extract_salient_keywords(initial_error);
                        keywords.push(tool.clone());
                        if let Some(ext) = Path::new(target).extension().and_then(|e| e.to_str()) {
                            keywords.push(ext.to_string());
                        }

                        let id = ltm.add_or_reinforce(
                            InsightCategory::BugPattern,
                            keywords,
                            &insight_text,
                            0.8,
                        );
                        outcomes.push(ConsolidationOutcome {
                            insight_id: id,
                            category: InsightCategory::BugPattern,
                            insight_text,
                            reason: "Consolidated from self-healing diagnostic recovery"
                                .to_string(),
                        });
                    }
                }
                StmEvent::ToolExecution {
                    tool: _,
                    target: _,
                    succeeded,
                    error,
                } => {
                    if !succeeded {
                        if let Some(err) = error {
                            if err.contains("pre-commit")
                                || err.contains("cargo fmt")
                                || err.contains("formatting check failed")
                            {
                                let insight_text =
                                    "Run 'cargo fmt' before committing changes to pass repository pre-commit hooks.";
                                let keywords = vec![
                                    "git".to_string(),
                                    "commit".to_string(),
                                    "cargo fmt".to_string(),
                                    "pre-commit".to_string(),
                                ];
                                let id = ltm.add_or_reinforce(
                                    InsightCategory::BuildQuirk,
                                    keywords,
                                    insight_text,
                                    0.9,
                                );
                                outcomes.push(ConsolidationOutcome {
                                    insight_id: id,
                                    category: InsightCategory::BuildQuirk,
                                    insight_text: insight_text.to_string(),
                                    reason: "Consolidated repository pre-commit requirement"
                                        .to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        outcomes
    }

    fn extract_salient_keywords(text: &str) -> Vec<String> {
        let stop_words: HashSet<&str> = [
            "the", "and", "or", "in", "on", "at", "to", "for", "with", "a", "an", "is", "was",
            "are", "were", "of", "by", "from", "van", "een", "het", "de", "en", "naar", "met",
            "voor", "niet", "wel", "maar", "als", "om", "te", "dan", "ook", "dit", "dat",
        ]
        .into_iter()
        .collect();

        text.split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|w| w.len() >= 3 && !stop_words.contains(&w.to_lowercase().as_str()))
            .map(|w| w.to_lowercase())
            .collect::<HashSet<_>>()
            .into_iter()
            .take(6)
            .collect()
    }

    fn is_systemic_error(err: &str) -> bool {
        let lower = err.to_lowercase();
        lower.contains("cannot find")
            || lower.contains("unresolved import")
            || lower.contains("trait bound")
            || lower.contains("mismatched types")
            || lower.contains("borrow")
            || lower.contains("pre-commit")
            || lower.contains("lto")
            || lower.contains("linker")
    }

    fn summarize_error(err: &str) -> String {
        let first_line = err.lines().next().unwrap_or(err).trim();
        if first_line.len() > 80 {
            format!("{}...", &first_line[..80])
        } else {
            first_line.to_string()
        }
    }
}

pub struct ProjectMemory {
    pub file_path: PathBuf,
    pub content: String,
}

impl ProjectMemory {
    pub fn memory_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".orbit").join("memory.md")
    }

    pub fn load_or_init(workspace_root: &Path) -> Result<Self> {
        let file_path = Self::memory_path(workspace_root);
        if file_path.exists() {
            let content = fs::read_to_string(&file_path)?;
            Ok(Self { file_path, content })
        } else {
            let init_content = r#"# Project Memory & Knowledge Graph
Persistent architectural decisions, discovered constraints, and codebase patterns.

## Architecture & Conventions
- Modular Rust design with async Tokio runtime.
- Deterministic test coverage across all tool implementations.

## Discovered Gotchas & Constraints
- Keep host disk clean; avoid unbounded dependency bloat.
"#
            .to_string();

            if let Some(parent) = file_path.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&file_path, &init_content)?;
            Ok(Self {
                file_path,
                content: init_content,
            })
        }
    }

    pub fn append_entry(&mut self, heading: &str, entry: &str) -> Result<()> {
        let date = chrono_date();
        let formatted = format!("\n### [{}] {}\n- {}\n", date, heading, entry);
        self.content.push_str(&formatted);
        fs::write(&self.file_path, &self.content)?;
        Ok(())
    }

    pub fn format_prompt_injection(&self) -> Option<String> {
        let trimmed = self.content.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(format!("<project_memory>\n{}\n</project_memory>", trimmed))
        }
    }
}

/// Comprehensive benchmark and evaluation result for the Orbit Memory subsystem.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryEvalReport {
    pub total_insights_evaluated: usize,
    pub noise_tests_total: usize,
    pub noise_tests_passed: usize,
    pub noise_rejection_rate: f64,
    pub precision_tests_total: usize,
    pub precision_tests_passed: usize,
    pub precision_rate: f64,
    pub specificity_tests_total: usize,
    pub specificity_tests_passed: usize,
    pub specificity_rate: f64,
    pub deduplication_tests_total: usize,
    pub deduplication_tests_passed: usize,
    pub deduplication_rate: f64,
    pub avg_retrieval_latency_us: f64,
    pub estimated_tokens_saved: usize,
    pub quality_score: usize,
    pub verdict: String,
}

impl MemoryEvalReport {
    pub fn format_banner(&self) -> String {
        let mut out = String::new();
        out.push_str("=== 🧠 ORBIT MEMORY BENCHMARK & DIAGNOSTIC REPORT ===\n\n");
        out.push_str(&format!(
            "• Noise Rejection (Vague/Trivial Ignored):    {}/{} ({:.1}%) [{}]\n",
            self.noise_tests_passed,
            self.noise_tests_total,
            self.noise_rejection_rate * 100.0,
            if self.noise_rejection_rate >= 0.99 {
                "PASS"
            } else {
                "WARN"
            }
        ));
        out.push_str(&format!(
            "• Retrieval Precision (Targeted Context Hits): {}/{} ({:.1}%) [{}]\n",
            self.precision_tests_passed,
            self.precision_tests_total,
            self.precision_rate * 100.0,
            if self.precision_rate >= 0.90 {
                "PASS"
            } else {
                "WARN"
            }
        ));
        out.push_str(&format!(
            "• Zero-Overhead Specificity (Irrelevant = 0):   {}/{} ({:.1}%) [{}]\n",
            self.specificity_tests_passed,
            self.specificity_tests_total,
            self.specificity_rate * 100.0,
            if self.specificity_rate >= 0.99 {
                "PASS"
            } else {
                "WARN"
            }
        ));
        out.push_str(&format!(
            "• Deduplication & Reinforcement Integrity:    {}/{} ({:.1}%) [{}]\n",
            self.deduplication_tests_passed,
            self.deduplication_tests_total,
            self.deduplication_rate * 100.0,
            if self.deduplication_rate >= 0.99 {
                "PASS"
            } else {
                "WARN"
            }
        ));
        out.push_str(&format!(
            "• Average Retrieval Latency:                  {:.2} µs ({:.3} ms) [FAST]\n",
            self.avg_retrieval_latency_us,
            self.avg_retrieval_latency_us / 1000.0
        ));
        out.push_str(&format!(
            "• Net Token Overhead Saved (vs Brute-Force):  ~{} tokens saved\n\n",
            self.estimated_tokens_saved
        ));
        out.push_str(&format!(
            "🏆 Memory Quality Score: {}/100 -> {}\n",
            self.quality_score, self.verdict
        ));
        out
    }
}

pub struct MemoryBenchmarkRunner;

impl MemoryBenchmarkRunner {
    /// Executes a deterministic multi-axis benchmark against the active or synthetic memory store.
    pub fn run_benchmark(active_ltm: Option<&LongTermMemory>) -> MemoryEvalReport {
        let mut test_ltm = LongTermMemory::default();

        // 1. Seed realistic benchmark insights across domains
        test_ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["sqlite".to_string(), "wal".to_string(), "pool".to_string()],
            "Always enable WAL mode on SQLite connections and use connection pooling.",
            0.95,
        );
        test_ltm.add_or_reinforce(
            InsightCategory::BuildQuirk,
            vec!["cargo".to_string(), "fmt".to_string(), "commit".to_string()],
            "Always run 'cargo fmt' before git commit to pass repository pre-commit checks.",
            0.90,
        );
        test_ltm.add_or_reinforce(
            InsightCategory::BugPattern,
            vec![
                "tokio".to_string(),
                "channel".to_string(),
                "deadlock".to_string(),
            ],
            "Avoid unbounded channels and recursive lock acquisitions across await boundaries.",
            0.85,
        );
        test_ltm.add_or_reinforce(
            InsightCategory::UserPreference,
            vec![
                "conventional".to_string(),
                "commit".to_string(),
                "git".to_string(),
            ],
            "Use conventional commit messages with standard feat/fix/chore prefixes.",
            1.0,
        );

        // Also merge active LTM insights if provided
        if let Some(act) = active_ltm {
            for ins in &act.insights {
                test_ltm.add_or_reinforce(
                    ins.category,
                    ins.trigger_keywords.clone(),
                    &ins.insight,
                    ins.confidence,
                );
            }
        }

        // --- AXIS 1: Noise Rejection ---
        let noise_samples = ["ok thanks", "prima", "goed zo", "yes", "testje"];
        let mut noise_passed = 0;
        for s in &noise_samples {
            let mut stm = WorkingMemory::new();
            stm.record_event(StmEvent::UserCorrection {
                instruction: s.to_string(),
            });
            let mut ltm_clone = test_ltm.clone();
            let outcomes = MemoryConsolidator::consolidate(&stm, &mut ltm_clone);
            if outcomes.is_empty() {
                noise_passed += 1;
            }
        }

        // --- AXIS 2: Retrieval Precision ---
        let precision_queries = [
            (
                "How should we configure the sqlite database connection pool?",
                InsightCategory::Architecture,
                "sqlite",
            ),
            (
                "Please commit my changes with cargo fmt checks",
                InsightCategory::BuildQuirk,
                "cargo fmt",
            ),
            (
                "Can you inspect tokio channel deadlock and locks?",
                InsightCategory::BugPattern,
                "channel",
            ),
            (
                "What format should my git commit message follow?",
                InsightCategory::UserPreference,
                "conventional commit",
            ),
        ];
        let mut precision_passed = 0;
        for (q, expected_cat, expected_sub) in &precision_queries {
            let matched = test_ltm.find_relevant_insights(q, 3);
            if !matched.is_empty()
                && matched[0].category == *expected_cat
                && matched[0].insight.to_lowercase().contains(expected_sub)
            {
                precision_passed += 1;
            }
        }

        // --- AXIS 3: Zero-Overhead Specificity ---
        let irrelevant_queries = [
            "Can you write a short poem about the sea?",
            "What is the weather like in Amsterdam?",
            "Translate hello world to French",
            "Calculate 42 multiplied by 1337",
        ];
        let mut specificity_passed = 0;
        for q in &irrelevant_queries {
            let matched = test_ltm.find_relevant_insights(q, 5);
            let injection = test_ltm.format_prompt_injection(q);
            if matched.is_empty() && injection.is_none() {
                specificity_passed += 1;
            }
        }

        // --- AXIS 4: Deduplication & Reinforcement Integrity ---
        let mut dedup_ltm = test_ltm.clone();
        let initial_count = dedup_ltm.insights.len();
        // Insert exact same rule again
        let id_re = dedup_ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["sqlite".to_string(), "wal".to_string()],
            "Always enable WAL mode on SQLite connections and use connection pooling.",
            0.98,
        );
        let dedup_passed = if dedup_ltm.insights.len() == initial_count
            && id_re == 1
            && dedup_ltm.insights[0].occurrence_count == 2
        {
            1
        } else {
            0
        };

        // --- Latency Measurement ---
        let start = std::time::Instant::now();
        let iters = 200;
        for _ in 0..iters {
            let _ = test_ltm.find_relevant_insights("sqlite database connection pool", 3);
            let _ = test_ltm.find_relevant_insights("poem about flowers", 3);
        }
        let total_us = start.elapsed().as_micros() as f64;
        let avg_latency_us = total_us / (iters as f64 * 2.0);

        // --- Calculation of Token Savings ---
        // Without relevance filtering, every turn would dump all insights into prompt
        let brute_force_chars: usize = test_ltm.insights.iter().map(|i| i.insight.len() + 30).sum();
        let brute_force_tokens = (brute_force_chars / 4).max(10);
        let estimated_tokens_saved = brute_force_tokens * 45; // estimated 45 turns session

        let noise_rate = noise_passed as f64 / noise_samples.len() as f64;
        let prec_rate = precision_passed as f64 / precision_queries.len() as f64;
        let spec_rate = specificity_passed as f64 / irrelevant_queries.len() as f64;
        let dedup_rate = dedup_passed as f64;

        let quality_score =
            ((noise_rate * 25.0) + (prec_rate * 35.0) + (spec_rate * 25.0) + (dedup_rate * 15.0))
                .round() as usize;

        let verdict = if quality_score >= 95 {
            "OPTIMAL (Production-Ready Cognitive Memory)".to_string()
        } else if quality_score >= 80 {
            "GOOD (Acceptable Signal-to-Noise Ratio)".to_string()
        } else {
            "SUBOPTIMAL (Requires Threshold Tuning)".to_string()
        };

        MemoryEvalReport {
            total_insights_evaluated: test_ltm.insights.len(),
            noise_tests_total: noise_samples.len(),
            noise_tests_passed: noise_passed,
            noise_rejection_rate: noise_rate,
            precision_tests_total: precision_queries.len(),
            precision_tests_passed: precision_passed,
            precision_rate: prec_rate,
            specificity_tests_total: irrelevant_queries.len(),
            specificity_tests_passed: specificity_passed,
            specificity_rate: spec_rate,
            deduplication_tests_total: 1,
            deduplication_tests_passed: dedup_passed,
            deduplication_rate: dedup_rate,
            avg_retrieval_latency_us: avg_latency_us,
            estimated_tokens_saved,
            quality_score,
            verdict,
        }
    }
}

fn chrono_date() -> String {
    let out = std::process::Command::new("date")
        .arg("+%Y-%m-%d %H:%M")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|_| "2026-10-24 08:00".to_string());
    if out.is_empty() {
        "2026-10-24 08:00".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_project_memory_init_and_append() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut mem = ProjectMemory::load_or_init(ws).unwrap();
        assert!(mem.file_path.exists());
        assert!(mem.content.contains("# Project Memory"));

        mem.append_entry("Security Rule", "Always sanitize path input")
            .unwrap();
        assert!(mem.content.contains("Security Rule"));
        assert!(mem.content.contains("Always sanitize path input"));

        let injection = mem.format_prompt_injection().unwrap();
        assert!(injection.starts_with("<project_memory>"));
        assert!(injection.ends_with("</project_memory>"));
    }

    #[test]
    fn test_working_memory_tracking_and_scratchpad() {
        let mut stm = WorkingMemory::new();
        stm.set_goal("Fix flaky database connection in test harness");
        stm.set_subtask("Retry with exponential backoff");
        stm.record_attempt(
            "connect_db",
            Some("Connection refused: 127.0.0.1:5432"),
            false,
        );
        stm.record_touchpoint(Path::new("tests/db_test.rs"));

        let scratchpad = stm.format_scratchpad();
        assert!(scratchpad.contains("🎯 Active Goal: Fix flaky database connection"));
        assert!(scratchpad.contains("⚡ Current Subtask: Retry with exponential backoff"));
        assert!(scratchpad.contains("Connection refused"));
        assert!(scratchpad.contains("tests/db_test.rs"));

        stm.clear_task();
        assert!(stm.current_goal.is_none());
        assert!(stm.failed_attempts.is_empty());
    }

    #[test]
    fn test_long_term_memory_save_and_relevance_retrieval() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        ltm.add_or_reinforce(
            InsightCategory::BuildQuirk,
            vec![
                "git".to_string(),
                "commit".to_string(),
                "cargo fmt".to_string(),
            ],
            "Always run cargo fmt before git commit",
            0.9,
        );
        ltm.add_or_reinforce(
            InsightCategory::UserPreference,
            vec!["tone".to_string(), "metaphor".to_string()],
            "Do not use maritime metaphors",
            1.0,
        );
        ltm.save(ws).unwrap();

        // Reload
        let loaded = LongTermMemory::load_or_init(ws).unwrap();
        assert_eq!(loaded.insights.len(), 2);

        // Find relevant insights
        let results = loaded.find_relevant_insights("Can you commit the changes to git?", 5);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].category, InsightCategory::BuildQuirk);
        assert!(results[0].insight.contains("cargo fmt"));

        let injection = loaded.format_prompt_injection("git commit please").unwrap();
        assert!(injection.contains("<learned_project_insights>"));
        assert!(injection.contains("cargo fmt"));
    }

    #[test]
    fn test_memory_consolidator_transition() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        let mut stm = WorkingMemory::new();

        // 1. User correction event in STM
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Keep font packages intact during system cleanups".to_string(),
        });

        // 2. Pre-commit failure event in STM
        stm.record_event(StmEvent::ToolExecution {
            tool: "run_command".to_string(),
            target: "git commit".to_string(),
            succeeded: false,
            error: Some(
                "❌ [Pre-Commit] Formatting check failed! Run 'cargo fmt' before committing."
                    .to_string(),
            ),
        });

        // 3. Self-healing success event
        stm.record_event(StmEvent::SelfHealingSuccess {
            tool: "write_file".to_string(),
            target: "src/db.rs".to_string(),
            initial_error: "error[E0432]: unresolved import `tokio::sync::Mutex`".to_string(),
            resolution_strategy: "adding `use tokio::sync::Mutex;` at top of file".to_string(),
        });

        // Run consolidation
        let outcomes = MemoryConsolidator::consolidate(&stm, &mut ltm);
        assert_eq!(outcomes.len(), 3);

        // Verify LTM stored the items
        assert_eq!(ltm.insights.len(), 3);
        let user_pref = ltm
            .insights
            .iter()
            .find(|i| i.category == InsightCategory::UserPreference)
            .unwrap();
        assert!(user_pref.insight.contains("font packages intact"));

        let build_quirk = ltm
            .insights
            .iter()
            .find(|i| i.category == InsightCategory::BuildQuirk)
            .unwrap();
        assert!(build_quirk.insight.contains("cargo fmt"));

        let bug_pattern = ltm
            .insights
            .iter()
            .find(|i| i.category == InsightCategory::BugPattern)
            .unwrap();
        assert!(bug_pattern.insight.contains("unresolved import"));
    }

    #[test]
    fn test_eval_overgang_agressiviteit_en_ruis_filtering() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        let mut stm = WorkingMemory::new();

        // 1. Trivial feedback / non-salient remarks should NOT transition to LTM
        stm.record_event(StmEvent::UserCorrection {
            instruction: "ok thanks".to_string(), // < 10 chars, noise
        });
        stm.record_event(StmEvent::UserCorrection {
            instruction: "goedzo".to_string(), // short, noise
        });

        // 2. Ephemeral or non-systemic compile glitches should NOT become permanent rules
        stm.record_event(StmEvent::SelfHealingSuccess {
            tool: "run_command".to_string(),
            target: "test_script.sh".to_string(),
            initial_error: "mkdir: cannot create directory 'tmp': File exists".to_string(), // not a systemic language/linker error
            resolution_strategy: "mkdir -p tmp".to_string(),
        });

        // 3. True salient instruction SHOULD transition
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Always run tests with --nocapture flag".to_string(),
        });

        let outcomes = MemoryConsolidator::consolidate(&stm, &mut ltm);

        // Only the salient instruction (#3) should have transitioned
        assert_eq!(
            outcomes.len(),
            1,
            "Expected exactly 1 consolidated outcome, but got {}",
            outcomes.len()
        );
        assert_eq!(outcomes[0].category, InsightCategory::UserPreference);
        assert!(outcomes[0].insight_text.contains("--nocapture"));
        assert_eq!(ltm.insights.len(), 1);
    }

    #[test]
    fn test_eval_relevance_ranking_en_token_budget_beheersing() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        // Populate multiple insights across diverse domains
        ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec![
                "database".to_string(),
                "sqlite".to_string(),
                "wal".to_string(),
            ],
            "Always enable WAL mode on SQLite connections for concurrency.",
            0.9,
        );
        ltm.add_or_reinforce(
            InsightCategory::BuildQuirk,
            vec![
                "docker".to_string(),
                "cache".to_string(),
                "buildkit".to_string(),
            ],
            "Set DOCKER_BUILDKIT=1 before running docker build.",
            0.8,
        );
        ltm.add_or_reinforce(
            InsightCategory::BugPattern,
            vec![
                "tokio".to_string(),
                "channel".to_string(),
                "deadlock".to_string(),
            ],
            "Never await on unbounded channels without select! branch timeouts.",
            0.85,
        );
        ltm.add_or_reinforce(
            InsightCategory::UserPreference,
            vec![
                "commit".to_string(),
                "git".to_string(),
                "message".to_string(),
            ],
            "Use conventional commits format: feat, fix, chore.",
            1.0,
        );

        // Test 1: Query specifically about SQLite database
        let query_db = "Could you check the sqlite database settings and optimize queries?";
        let matched_db = ltm.find_relevant_insights(query_db, 2);
        assert_eq!(matched_db.len(), 1, "Only DB insight should match");
        assert_eq!(matched_db[0].category, InsightCategory::Architecture);
        assert!(matched_db[0].insight.contains("WAL mode"));

        // Test 2: Query unrelated to any insight -> zero injection (zero token overhead)
        let query_irrelevant = "Can you write a poem about the autumn leaves?";
        let matched_none = ltm.find_relevant_insights(query_irrelevant, 5);
        assert!(
            matched_none.is_empty(),
            "Irrelevant query must produce zero matched insights"
        );
        assert!(ltm.format_prompt_injection(query_irrelevant).is_none());

        // Test 3: Top-K bounding keeps token budget controlled
        let query_broad = "git commit to database";
        let matched_top = ltm.find_relevant_insights(query_broad, 1);
        assert_eq!(matched_top.len(), 1, "Top-K must strictly enforce limit 1");
    }

    #[test]
    fn test_eval_consolidatie_frequentie_versterking_en_deduplicatie() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        // 1. Initial insight insertion
        let id1 = ltm.add_or_reinforce(
            InsightCategory::BuildQuirk,
            vec!["cargo".to_string(), "fmt".to_string()],
            "Always run cargo fmt before git commit",
            0.8,
        );
        assert_eq!(id1, 1);
        assert_eq!(ltm.insights.len(), 1);
        assert_eq!(ltm.insights[0].occurrence_count, 1);
        assert_eq!(ltm.insights[0].confidence, 0.8);

        // 2. Reinforce the identical insight (e.g. encountered again during another session)
        let id2 = ltm.add_or_reinforce(
            InsightCategory::BuildQuirk,
            vec![
                "cargo".to_string(),
                "fmt".to_string(),
                "pre-commit".to_string(),
            ],
            "Always run cargo fmt before git commit",
            0.9,
        );
        assert_eq!(id2, 1, "Must deduplicate and update existing ID 1");
        assert_eq!(ltm.insights.len(), 1, "Must not introduce duplicate record");
        assert_eq!(
            ltm.insights[0].occurrence_count, 2,
            "Occurrence count must increment"
        );
        assert!(
            ltm.insights[0].confidence >= 0.9,
            "Confidence must be elevated"
        );
        assert!(ltm.insights[0]
            .trigger_keywords
            .contains(&"pre-commit".to_string()));

        // 3. Verify rank boost due to frequency reinforcement
        ltm.add_or_reinforce(
            InsightCategory::BuildQuirk,
            vec!["cargo".to_string(), "clippy".to_string()],
            "Run cargo clippy on all targets",
            0.85,
        );
        let ranked = ltm.find_relevant_insights("cargo check and commit", 5);
        // The reinforced insight with occurrence_count = 2 must rank higher
        assert_eq!(ranked[0].id, 1);
    }

    #[test]
    fn test_memory_benchmark_runner_execution() {
        let report = MemoryBenchmarkRunner::run_benchmark(None);
        assert_eq!(report.noise_tests_passed, report.noise_tests_total);
        assert_eq!(report.precision_tests_passed, report.precision_tests_total);
        assert_eq!(
            report.specificity_tests_passed,
            report.specificity_tests_total
        );
        assert_eq!(report.deduplication_tests_passed, 1);
        assert!(
            report.quality_score >= 95,
            "Quality score should be >= 95: {}",
            report.quality_score
        );
        assert!(report.avg_retrieval_latency_us > 0.0);

        let banner = report.format_banner();
        println!("\n{}", banner);
        assert!(banner.contains("ORBIT MEMORY BENCHMARK & DIAGNOSTIC REPORT"));
        assert!(banner.contains("Noise Rejection"));
        assert!(banner.contains("Retrieval Precision"));
        assert!(banner.contains("Zero-Overhead Specificity"));
        assert!(banner.contains("Deduplication & Reinforcement"));
    }
}
