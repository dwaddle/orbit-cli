use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
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
            "user_preference" | "user" | "pref" | "preference" => Ok(Self::UserPreference),
            "build_quirk" | "build" | "quirk" | "workaround" => Ok(Self::BuildQuirk),
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
    #[serde(default = "default_last_accessed")]
    pub last_accessed: u64,
    #[serde(default = "default_stability")]
    pub stability_days: f32,
    #[serde(default = "default_verification_score")]
    pub verification_score: f32,
    #[serde(default)]
    pub refutation_count: u32,
    #[serde(default)]
    pub refuted_at: Option<u64>,
    #[serde(default)]
    pub refutation_reason: Option<String>,
    #[serde(default)]
    pub supersedes: Option<usize>,
    #[serde(default)]
    pub superseded_by: Option<usize>,
}

fn default_last_accessed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn default_stability() -> f32 {
    14.0 // Baseline half-life: 14 days
}

fn default_verification_score() -> f32 {
    0.5 // Baseline unverified initial state
}

impl InsightRecord {
    /// Calculates the Ebbinghaus memory retrievability R in [0.0, 1.0]
    /// Formula: R = exp( - delta_t / stability )
    pub fn retrievability(&self, now_secs: u64) -> f32 {
        let delta_secs = now_secs.saturating_sub(self.last_accessed);
        let delta_days = delta_secs as f32 / 86400.0;
        let stability = self.stability_days.max(1.0);
        (-delta_days / stability).exp().clamp(0.0, 1.0)
    }

    /// Determines whether this insight has been refuted or replaced by newer evidence.
    pub fn is_refuted(&self) -> bool {
        self.refuted_at.is_some() || self.verification_score <= -0.5 || self.superseded_by.is_some()
    }

    /// Returns a human-readable cognitive status label.
    pub fn status_label(&self) -> &'static str {
        if self.is_refuted() {
            "REFUTED"
        } else if self.verification_score >= 0.8 {
            "VERIFIED"
        } else if self.verification_score < 0.2 {
            "CHALLENGED"
        } else {
            "UNVERIFIED"
        }
    }
}

/// Persistent Long-Term Memory (LTM) store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LongTermMemory {
    pub insights: Vec<InsightRecord>,
    next_id: usize,
    /// Inverted index mapping lowercase keyword tokens to list of insight IDs for O(1) candidate lookup
    #[serde(skip)]
    keyword_index: HashMap<String, Vec<usize>>,
}

impl Default for LongTermMemory {
    fn default() -> Self {
        Self {
            insights: Vec::new(),
            next_id: 1,
            keyword_index: HashMap::new(),
        }
    }
}

impl LongTermMemory {
    pub fn memory_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".orbit").join("insights.json")
    }

    pub fn archive_path(workspace_root: &Path) -> PathBuf {
        workspace_root.join(".orbit").join("insights_archive.json")
    }

    pub fn load_or_init(workspace_root: &Path) -> Result<Self> {
        let file_path = Self::memory_path(workspace_root);
        if file_path.exists() {
            let data = fs::read_to_string(&file_path)?;
            let mut ltm: Self = match serde_json::from_str(&data) {
                Ok(loaded) => loaded,
                Err(err) => {
                    let ts = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let corrupt_path =
                        file_path.with_file_name(format!("insights.json.corrupt.{}", ts));
                    let _ = fs::copy(&file_path, &corrupt_path);
                    eprintln!(
                        "⚠️ Warning: Corrupt insights.json detected ({}). Backed up to {}. Reinitializing clean store.",
                        err,
                        corrupt_path.display()
                    );
                    Self::default()
                }
            };
            if ltm.next_id == 0 {
                ltm.next_id = ltm.insights.iter().map(|i| i.id).max().unwrap_or(0) + 1;
            }
            ltm.rebuild_index();
            Ok(ltm)
        } else {
            let default_ltm = Self {
                insights: Vec::new(),
                next_id: 1,
                keyword_index: HashMap::new(),
            };
            default_ltm.save(workspace_root)?;
            Ok(default_ltm)
        }
    }

    /// Rebuilds the inverted index mapping lowercase tokens to non-refuted insight IDs.
    pub fn rebuild_index(&mut self) {
        self.keyword_index.clear();
        for rec in &self.insights {
            if rec.is_refuted() {
                continue;
            }
            for kw in &rec.trigger_keywords {
                let kw_clean = kw.to_lowercase();
                self.keyword_index.entry(kw_clean).or_default().push(rec.id);
            }
            // Also index content tokens for full fast coverage
            for token in rec
                .insight
                .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            {
                if token.len() >= 3 {
                    let tok_lower = token.to_lowercase();
                    let entry = self.keyword_index.entry(tok_lower).or_default();
                    if !entry.contains(&rec.id) {
                        entry.push(rec.id);
                    }
                }
            }
        }
    }

    pub fn save(&self, workspace_root: &Path) -> Result<()> {
        let file_path = Self::memory_path(workspace_root);
        if let Some(parent) = file_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        let temp_path = file_path.with_extension("json.tmp");
        fs::write(&temp_path, data)?;
        fs::rename(&temp_path, &file_path)?;
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
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(existing) = self.insights.iter_mut().find(|i| {
            i.category == category
                && (i.insight.eq_ignore_ascii_case(text_clean)
                    || i.insight.contains(text_clean)
                    || text_clean.contains(&i.insight))
        }) {
            existing.occurrence_count += 1;
            existing.confidence = (existing.confidence + confidence).min(1.0);
            existing.last_accessed = now_ts;
            // Spaced repetition: memory stability increases by 50% with each reinforcement
            existing.stability_days = (existing.stability_days * 1.5).min(365.0);
            if existing.is_refuted() && existing.superseded_by.is_none() {
                existing.refuted_at = None;
                existing.refutation_reason = None;
                existing.verification_score = 0.5;
            } else {
                existing.verification_score = (existing.verification_score + 0.1).min(1.0);
            }
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

        self.insights.push(InsightRecord {
            id,
            category,
            trigger_keywords: clean_keywords,
            insight: text_clean.to_string(),
            confidence: confidence.clamp(0.0, 1.0),
            occurrence_count: 1,
            created_at: now_ts,
            last_accessed: now_ts,
            stability_days: 14.0,
            verification_score: 0.5,
            refutation_count: 0,
            refuted_at: None,
            refutation_reason: None,
            supersedes: None,
            superseded_by: None,
        });

        self.rebuild_index();
        id
    }

    /// Marks an insight as refuted and obsolete. It will be excluded from prompt injections and retrieval.
    pub fn refute_insight(
        &mut self,
        id: usize,
        reason: &str,
        superseded_by: Option<usize>,
    ) -> bool {
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        if let Some(rec) = self.insights.iter_mut().find(|r| r.id == id) {
            rec.refuted_at = Some(now_ts);
            rec.refutation_reason = Some(reason.to_string());
            rec.refutation_count += 1;
            rec.verification_score = -1.0;
            rec.confidence = 0.0;
            if superseded_by.is_some() {
                rec.superseded_by = superseded_by;
            }
            self.rebuild_index();
            true
        } else {
            false
        }
    }

    /// Challenges an insight with a penalty. If verification_score drops <= -0.5, it is automatically marked refuted.
    pub fn challenge_insight(&mut self, id: usize, penalty: f32, reason: Option<&str>) -> bool {
        if let Some(rec) = self.insights.iter_mut().find(|r| r.id == id) {
            rec.refutation_count += 1;
            rec.verification_score = (rec.verification_score - penalty).clamp(-1.0, 1.0);
            rec.confidence = (rec.confidence - (penalty * 0.5)).clamp(0.0, 1.0);
            if let Some(r) = reason {
                rec.refutation_reason = Some(r.to_string());
            }
            if rec.verification_score <= -0.5 && rec.refuted_at.is_none() {
                let now_ts = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_secs();
                rec.refuted_at = Some(now_ts);
            }
            self.rebuild_index();
            true
        } else {
            false
        }
    }

    /// Verifies and reinforces an insight with positive evidence.
    pub fn verify_insight(&mut self, id: usize, boost: f32) -> bool {
        let mut resurrected = false;
        let found = if let Some(rec) = self.insights.iter_mut().find(|r| r.id == id) {
            let was_refuted = rec.is_refuted();
            rec.verification_score = (rec.verification_score + boost).clamp(-1.0, 1.0);
            rec.confidence = (rec.confidence + (boost * 0.2)).clamp(0.0, 1.0);
            rec.stability_days = (rec.stability_days * 1.2).min(365.0);
            if rec.verification_score > -0.5 && rec.superseded_by.is_none() {
                rec.refuted_at = None;
                rec.refutation_reason = None;
            }
            if was_refuted && !rec.is_refuted() {
                resurrected = true;
            }
            true
        } else {
            false
        };

        if resurrected {
            self.rebuild_index();
        }
        found
    }

    /// Identifies active insights that conflict with a new corrective instruction.
    pub fn find_conflicting_insights(&self, new_text: &str) -> Vec<usize> {
        let lower = new_text.to_lowercase();
        let negation_markers = [
            "niet meer",
            "geen ",
            "stop met",
            "vervang ",
            "don't ",
            "do not ",
            "never ",
            "instead of",
            "in plaats van",
            "deprecated",
            "obsolete",
            "verkeerd",
            "incorrect",
            "is wrong",
            "stop using",
            "vermijd ",
            "not use",
        ];

        let has_negation = negation_markers.iter().any(|m| lower.contains(m));
        if !has_negation {
            return Vec::new();
        }

        let new_tokens: HashSet<&str> = lower
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|t| t.len() >= 3)
            .collect();

        let mut conflicts = Vec::new();
        for rec in &self.insights {
            if rec.is_refuted() {
                continue;
            }
            let mut match_count = 0;
            for kw in &rec.trigger_keywords {
                if new_tokens.contains(kw.as_str()) || lower.contains(kw) {
                    match_count += 1;
                }
            }
            for token in rec
                .insight
                .to_lowercase()
                .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
                .filter(|t| t.len() >= 4)
            {
                if new_tokens.contains(token) {
                    match_count += 1;
                }
            }

            if match_count >= 2
                || (match_count >= 1
                    && rec
                        .trigger_keywords
                        .iter()
                        .any(|k| lower.contains(k) && k.len() >= 4))
            {
                conflicts.push(rec.id);
            }
        }
        conflicts
    }

    pub fn find_relevant_insights(&self, query: &str, max_results: usize) -> Vec<&InsightRecord> {
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let lower_query = query.to_lowercase();
        let query_tokens: HashSet<&str> = lower_query
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .filter(|t| t.len() >= 3)
            .collect();

        // 1. O(1) Candidate set extraction using Inverted Index
        let mut candidate_ids: HashSet<usize> = HashSet::new();
        for token in &query_tokens {
            if let Some(ids) = self.keyword_index.get(*token) {
                for id in ids {
                    candidate_ids.insert(*id);
                }
            }
        }

        // Also check if any trigger keywords are multi-word substrings in lower_query
        for rec in &self.insights {
            if rec.is_refuted() {
                continue;
            }
            for kw in &rec.trigger_keywords {
                if kw.contains(' ') && lower_query.contains(kw) {
                    candidate_ids.insert(rec.id);
                }
            }
        }

        if candidate_ids.is_empty() {
            return Vec::new();
        }

        let mut scored: Vec<(&InsightRecord, f32)> = candidate_ids
            .into_iter()
            .filter_map(|id| self.insights.iter().find(|r| r.id == id))
            .filter(|rec| !rec.is_refuted())
            .filter_map(|rec| {
                let mut match_score: f32 = 0.0;
                for kw in &rec.trigger_keywords {
                    if lower_query.contains(kw) || query_tokens.contains(kw.as_str()) {
                        match_score += 3.0;
                    }
                }
                for token in &query_tokens {
                    if rec.insight.to_lowercase().contains(token) {
                        match_score += 1.0;
                    }
                }
                if match_score > 0.0 {
                    // Biologically-grounded retention weight:
                    // Base activation from match + frequency reinforcement, scaled by Ebbinghaus retention R
                    let r = rec.retrievability(now_ts);
                    let freq_factor = (rec.occurrence_count as f32).min(10.0);
                    let final_score = (match_score + freq_factor) * r;
                    Some((rec, final_score))
                } else {
                    None
                }
            })
            .collect();

        scored.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| b.0.id.cmp(&a.0.id))
        });
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
                let sanitized_insight = rec
                    .insight
                    .replace("</learned_project_insights>", "[/learned_project_insights]");
                out.push_str(&format!(
                    "- [{}] [{}] {}: {} (reinforcements: {})\n",
                    rec.category.as_str(),
                    rec.status_label(),
                    rec.trigger_keywords.join("/"),
                    sanitized_insight,
                    rec.occurrence_count
                ));
            }
            out.push_str("</learned_project_insights>");
            Some(out)
        }
    }

    /// Synaptic Pruning: removes inactive or refuted insights whose Ebbinghaus retrievability
    /// has dropped below `min_retrievability` (e.g. 0.05). Returns count of pruned records.
    pub fn prune_decayed_insights(&mut self, min_retrievability: f32) -> usize {
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        let initial_len = self.insights.len();
        self.insights
            .retain(|rec| !rec.is_refuted() && rec.retrievability(now_ts) >= min_retrievability);
        let pruned = initial_len.saturating_sub(self.insights.len());
        if pruned > 0 {
            self.rebuild_index();
        }
        pruned
    }

    /// Prunes decayed or refuted insights and moves them to `.orbit/insights_archive.json` for permanent cold storage.
    pub fn prune_and_archive(
        &mut self,
        workspace_root: &Path,
        min_retrievability: f32,
    ) -> Result<usize> {
        let now_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();

        // 1. Identify records to prune without mutating self.insights yet
        let prune_ids: HashSet<usize> = self
            .insights
            .iter()
            .filter(|rec| rec.is_refuted() || rec.retrievability(now_ts) < min_retrievability)
            .map(|rec| rec.id)
            .collect();

        if prune_ids.is_empty() {
            return Ok(0);
        }

        let to_archive: Vec<InsightRecord> = self
            .insights
            .iter()
            .filter(|rec| prune_ids.contains(&rec.id))
            .cloned()
            .collect();

        // 2. Load and safely merge into archive FIRST
        let arch_path = Self::archive_path(workspace_root);
        let mut archive: Vec<InsightRecord> = if arch_path.exists() {
            let data = fs::read_to_string(&arch_path)?;
            match serde_json::from_str(&data) {
                Ok(records) => records,
                Err(e) => {
                    let corrupt_path = arch_path
                        .with_file_name(format!("insights_archive.json.corrupt.{}", now_ts));
                    let _ = fs::copy(&arch_path, &corrupt_path);
                    eprintln!(
                        "⚠️ Corrupt insights_archive.json detected ({}). Backed up to {}.",
                        e,
                        corrupt_path.display()
                    );
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        };

        archive.extend(to_archive);

        if let Some(parent) = arch_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(&archive)?;
        let temp_arch = arch_path.with_extension("json.tmp");
        fs::write(&temp_arch, data)?;
        fs::rename(&temp_arch, &arch_path)?;

        // 3. ONLY after disk persistence succeeds, mutate self.insights
        let pruned_count = prune_ids.len();
        self.insights.retain(|rec| !prune_ids.contains(&rec.id));
        self.rebuild_index();

        Ok(pruned_count)
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
    SubagentExecution {
        dag_name: String,
        node_name: String,
        role: String,
        succeeded: bool,
        summary: String,
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
                // Miller's Law: keep working memory focused on recent 7 +/- 2 chunks (cap at 7)
                if self.failed_attempts.len() > 7 {
                    self.failed_attempts.remove(0);
                }
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

    pub fn record_subagent(
        &mut self,
        dag_name: &str,
        node_name: &str,
        role: &str,
        succeeded: bool,
        summary: &str,
    ) {
        self.observed_events.push(StmEvent::SubagentExecution {
            dag_name: dag_name.to_string(),
            node_name: node_name.to_string(),
            role: role.to_string(),
            succeeded,
            summary: summary.to_string(),
        });

        if !succeeded {
            self.record_attempt(
                &format!("{}: {}", dag_name, node_name),
                Some(summary),
                false,
            );
        }
    }

    /// Drains all observed events that have been processed during consolidation,
    /// preventing redundant re-evaluation on subsequent agent turns.
    pub fn drain_events(&mut self) -> Vec<StmEvent> {
        std::mem::take(&mut self.observed_events)
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
                    if clean.len() >= 10 && !Self::is_trivial_remark(clean) {
                        // 1. Detect and refute any existing insights contradicted by this correction
                        let conflicts = ltm.find_conflicting_insights(clean);
                        let mut primary_superseded = None;
                        for conflict_id in conflicts {
                            let (conflict_cat, conflict_text) = if let Some(r) =
                                ltm.insights.iter().find(|i| i.id == conflict_id)
                            {
                                (r.category, r.insight.clone())
                            } else {
                                (InsightCategory::UserPreference, String::new())
                            };
                            let refutation_reason =
                                format!("Contradicted by user correction: '{}'", clean);
                            ltm.refute_insight(conflict_id, &refutation_reason, None);
                            primary_superseded = Some(conflict_id);
                            outcomes.push(ConsolidationOutcome {
                                insight_id: conflict_id,
                                category: conflict_cat,
                                insight_text: conflict_text,
                                reason: refutation_reason,
                            });
                        }

                        let keywords = Self::extract_salient_keywords(clean);
                        // Require at least one meaningful non-stopword keyword
                        if !keywords.is_empty() {
                            let new_id = ltm.add_or_reinforce(
                                InsightCategory::UserPreference,
                                keywords,
                                clean,
                                1.0,
                            );
                            if let Some(superseded_id) = primary_superseded {
                                if let Some(new_rec) =
                                    ltm.insights.iter_mut().find(|i| i.id == new_id)
                                {
                                    new_rec.supersedes = Some(superseded_id);
                                }
                                if let Some(old_rec) =
                                    ltm.insights.iter_mut().find(|i| i.id == superseded_id)
                                {
                                    old_rec.superseded_by = Some(new_id);
                                }
                            }
                            outcomes.push(ConsolidationOutcome {
                                insight_id: new_id,
                                category: InsightCategory::UserPreference,
                                insight_text: clean.to_string(),
                                reason: if primary_superseded.is_some() {
                                    "Learned correction superseding refuted insight".to_string()
                                } else {
                                    "Learned directly from user correction".to_string()
                                },
                            });
                        }
                    }
                }
                StmEvent::SelfHealingSuccess {
                    tool,
                    target,
                    initial_error,
                    resolution_strategy,
                } => {
                    // Positive verification for any active insight matching the error keywords
                    let err_lower = initial_error.to_lowercase();
                    let mut matched_ids = Vec::new();
                    for rec in &ltm.insights {
                        if !rec.is_refuted()
                            && rec
                                .trigger_keywords
                                .iter()
                                .any(|k| err_lower.contains(k.as_str()))
                        {
                            matched_ids.push(rec.id);
                        }
                    }
                    for mid in matched_ids {
                        ltm.verify_insight(mid, 0.2);
                    }

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
                    tool,
                    target,
                    succeeded,
                    error,
                } => {
                    if *succeeded {
                        // Check if any active insight was exercised and reinforce verification
                        let lower_target = target.to_lowercase();
                        let mut verified_ids = Vec::new();
                        for rec in &ltm.insights {
                            if !rec.is_refuted()
                                && (rec.trigger_keywords.iter().any(|k| {
                                    lower_target.contains(k) || tool.eq_ignore_ascii_case(k)
                                }))
                            {
                                verified_ids.push(rec.id);
                            }
                        }
                        for vid in verified_ids {
                            ltm.verify_insight(vid, 0.1);
                        }
                    } else if let Some(err) = error {
                        // Challenge any insight whose advice was followed and resulted in failure
                        let lower_err = err.to_lowercase();
                        let mut challenged_ids = Vec::new();
                        for rec in &ltm.insights {
                            if !rec.is_refuted()
                                && rec
                                    .trigger_keywords
                                    .iter()
                                    .any(|k| lower_err.contains(k.as_str()))
                            {
                                challenged_ids.push(rec.id);
                            }
                        }
                        for cid in challenged_ids {
                            ltm.challenge_insight(
                                cid,
                                0.25,
                                Some("Tool execution failed while matching insight keywords"),
                            );
                        }

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
                StmEvent::SubagentExecution {
                    dag_name,
                    node_name,
                    role,
                    succeeded,
                    summary,
                } => {
                    if !succeeded {
                        let clean_summary = Self::summarize_error(summary);
                        let insight_text = format!(
                            "Subagent '{}' [{}] failed during DAG '{}': {}",
                            node_name, role, dag_name, clean_summary
                        );
                        let mut keywords = Self::extract_salient_keywords(summary);
                        keywords.push(node_name.to_lowercase());
                        keywords.push(role.to_lowercase());
                        let id = ltm.add_or_reinforce(
                            InsightCategory::BugPattern,
                            keywords,
                            &insight_text,
                            0.85,
                        );
                        outcomes.push(ConsolidationOutcome {
                            insight_id: id,
                            category: InsightCategory::BugPattern,
                            insight_text,
                            reason: format!(
                                "Consolidated subagent execution failure from DAG '{}'",
                                dag_name
                            ),
                        });
                    } else {
                        let upper = summary.to_uppercase();
                        if upper.contains("CRITICAL")
                            || upper.contains("VULNERABILITY")
                            || upper.contains("FINDING")
                            || upper.contains("ARCHITECTURE")
                            || upper.contains("SECURITY")
                        {
                            let first_line = summary
                                .lines()
                                .find(|l| !l.trim().is_empty())
                                .unwrap_or(summary)
                                .trim();
                            let truncated = if first_line.len() > 140 {
                                format!("{}...", &first_line[..140])
                            } else {
                                first_line.to_string()
                            };
                            let insight_text = format!(
                                "In DAG '{}', {} [{}] identified: {}",
                                dag_name, node_name, role, truncated
                            );
                            let mut keywords = Self::extract_salient_keywords(&truncated);
                            keywords.push(role.to_lowercase());
                            if !keywords.is_empty() {
                                let id = ltm.add_or_reinforce(
                                    InsightCategory::Architecture,
                                    keywords,
                                    &insight_text,
                                    0.80,
                                );
                                outcomes.push(ConsolidationOutcome {
                                    insight_id: id,
                                    category: InsightCategory::Architecture,
                                    insight_text,
                                    reason: format!(
                                        "Consolidated salient subagent finding from DAG '{}'",
                                        dag_name
                                    ),
                                });
                            }
                        }
                    }
                }
            }
        }

        outcomes
    }

    pub fn extract_salient_keywords(text: &str) -> Vec<String> {
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

    /// Rejects conversational affirmations and pleasantries that lack actionable instruction.
    pub fn is_trivial_remark(text: &str) -> bool {
        let lower = text.trim().to_lowercase();
        let cleaned: String = lower
            .chars()
            .filter(|c| c.is_alphanumeric() || c.is_whitespace())
            .collect();

        let trivial_phrases = [
            "klinkt goed",
            "sounds good",
            "looks good",
            "prima bedankt",
            "dank je wel",
            "dankjewel",
            "thank you very much",
            "perfect bedankt",
            "heel erg bedankt",
            "geen probleem",
            "hartelijk dank",
            "ziet er goed uit",
            "goed gedaan",
            "well done",
            "nice job",
            "great job",
            "super bedankt",
            "veel dank",
        ];

        for phrase in &trivial_phrases {
            if cleaned == *phrase || cleaned.starts_with(&format!("{} ", phrase)) {
                return true;
            }
        }

        false
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

    pub fn summarize_error(err: &str) -> String {
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
        let mut new_content = self.content.clone();
        new_content.push_str(&formatted);

        let temp_path = self.file_path.with_extension("md.tmp");
        fs::write(&temp_path, &new_content)?;
        fs::rename(&temp_path, &self.file_path)?;

        self.content = new_content;
        Ok(())
    }

    pub fn format_prompt_injection(&self) -> Option<String> {
        let trimmed = self.content.trim();
        if trimmed.is_empty() {
            None
        } else {
            let lines: Vec<&str> = trimmed.lines().collect();
            if lines.len() <= 120 {
                Some(format!("<project_memory>\n{}\n</project_memory>", trimmed))
            } else {
                let header = lines[..40.min(lines.len())].join("\n");
                let recent = lines[lines.len().saturating_sub(60)..].join("\n");
                Some(format!(
                    "<project_memory>\n{}\n\n[... earlier project notes condensed for token efficiency ...]\n\n{}\n</project_memory>",
                    header, recent
                ))
            }
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
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let days = (secs / 86400) as i64;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;

    // Euclidean affine algorithm for civil date (zero external crates or subprocesses)
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1024 + doe / 1460 - doe / 36524) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, m, d, hours, minutes)
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
    fn test_subagent_memory_recording_and_consolidation() {
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        let mut stm = WorkingMemory::new();

        // 1. Record successful subagent finding with salient security/architecture discovery
        stm.record_subagent(
            "Security Audit",
            "Vulnerability Scanner",
            "Security Researcher",
            true,
            "CRITICAL VULNERABILITY: Raw SQL query in auth endpoint allows SQL injection bypass",
        );

        // 2. Record failed subagent node execution
        stm.record_subagent(
            "Feature Pipeline",
            "Code Compiler",
            "Build Engineer",
            false,
            "error[E0308]: mismatched types: expected usize, found u64 in file parser",
        );

        // Verify STM state
        assert_eq!(stm.observed_events.len(), 2);
        assert_eq!(stm.failed_attempts.len(), 1);
        assert!(stm.failed_attempts[0]
            .action
            .contains("Feature Pipeline: Code Compiler"));

        // Verify scratchpad formatting includes the failure
        let scratchpad = stm.format_scratchpad();
        assert!(scratchpad.contains("Previous Failed Attempts"));
        assert!(scratchpad.contains("mismatched types"));

        // Run consolidation
        let outcomes = MemoryConsolidator::consolidate(&stm, &mut ltm);
        assert_eq!(outcomes.len(), 2);

        // Verify LTM consolidation outcomes
        let arch = ltm
            .insights
            .iter()
            .find(|i| i.category == InsightCategory::Architecture)
            .expect("Expected Architecture insight from critical finding");
        assert!(arch.insight.contains("SQL injection"));

        let bug = ltm
            .insights
            .iter()
            .find(|i| i.category == InsightCategory::BugPattern)
            .expect("Expected BugPattern insight from subagent failure");
        assert!(bug.insight.contains("Code Compiler"));
        assert!(bug.insight.contains("mismatched types"));

        // Verify relevance retrieval primes queries mentioning sql or compiler
        let relevant = ltm.find_relevant_insights("prevent sql injection in auth", 3);
        assert!(!relevant.is_empty());
        assert_eq!(relevant[0].category, InsightCategory::Architecture);
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

    #[test]
    fn test_stm_high_throughput_and_rotation_limit() {
        let mut stm = WorkingMemory::new();
        stm.set_goal("Massive batch execution stress test");
        stm.set_subtask("Execute 1000 actions and 50 touchpoints");

        // Record 1,000 attempts (under Miller's Law capped at 7 items via FIFO eviction)
        for i in 0..1000 {
            stm.record_attempt(
                &format!("action_{}", i),
                Some(&format!(
                    "Error detail line 1 for action {}\nStacktrace ignored",
                    i
                )),
                false,
            );
        }
        assert_eq!(
            stm.failed_attempts.len(),
            7,
            "Miller's Law cap must strictly enforce max 7 chunks"
        );
        assert!(stm
            .failed_attempts
            .last()
            .unwrap()
            .error_summary
            .contains("action 999"));

        // Record 50 touchpoints (should be strictly capped at 10 items via FIFO eviction)
        for i in 0..50 {
            stm.record_touchpoint(&PathBuf::from(format!("src/module_{}.rs", i)));
        }
        assert_eq!(stm.recent_touchpoints.len(), 10);
        // The last added (49) must be present, the oldest (0..39) must have been evicted
        assert_eq!(
            stm.recent_touchpoints.last().unwrap(),
            &PathBuf::from("src/module_49.rs")
        );
        assert_eq!(
            stm.recent_touchpoints.first().unwrap(),
            &PathBuf::from("src/module_40.rs")
        );

        // Verify scratchpad formatting under load
        let scratch = stm.format_scratchpad();
        assert!(scratch.contains("🎯 Active Goal: Massive batch execution stress test"));
        assert!(scratch.contains("📁 Touchpoints:"));

        // Verify clean task wiping
        stm.clear_task();
        assert!(stm.current_goal.is_none());
        assert!(stm.current_subtask.is_none());
        assert!(stm.failed_attempts.is_empty());
        assert!(stm.observed_events.is_empty());
        // Touchpoints remain as session workspace context
        assert_eq!(stm.recent_touchpoints.len(), 10);
    }

    #[test]
    fn test_ltm_mass_insertion_and_ranking_stability() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        // Insert 200 distinct synthetic insights across all 4 categories
        for i in 0..200 {
            let cat = match i % 4 {
                0 => InsightCategory::Architecture,
                1 => InsightCategory::BugPattern,
                2 => InsightCategory::UserPreference,
                _ => InsightCategory::BuildQuirk,
            };
            ltm.add_or_reinforce(
                cat,
                vec![
                    format!("tag_{}", i),
                    format!("cluster_{}", i % 10),
                    "common_kw".to_string(),
                ],
                &format!(
                    "Synthetic architecture and coding rule number {} for cluster {}",
                    i,
                    i % 10
                ),
                0.75,
            );
        }
        assert_eq!(ltm.insights.len(), 200);

        // Reinforce a specific insight 5 times
        for _ in 0..5 {
            ltm.add_or_reinforce(
                InsightCategory::Architecture,
                vec!["high_priority".to_string(), "common_kw".to_string()],
                "Synthetic architecture and coding rule number 0 for cluster 0",
                0.9,
            );
        }

        // Verify occurrence count and top-K ranking
        let matched = ltm.find_relevant_insights("high_priority common_kw architecture", 5);
        assert!(!matched.is_empty());
        assert_eq!(matched[0].id, 1);
        assert_eq!(matched[0].occurrence_count, 6); // 1 initial + 5 reinforcements
        assert_eq!(matched[0].confidence, 1.0);
    }

    #[test]
    fn test_ltm_persistence_roundtrip_integrity() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        let test_records = [
            (
                InsightCategory::Architecture,
                vec!["tokio", "async", "lock"],
                "Always guard async locks across await boundaries.",
            ),
            (
                InsightCategory::UserPreference,
                vec!["emacs", "evil", "editor"],
                "User uses Emacs with Evil mode; format buffer commands cleanly.",
            ),
            (
                InsightCategory::BugPattern,
                vec!["sql", "injection", "sanitize"],
                "Escape all SQL user inputs using parameterized statements.",
            ),
            (
                InsightCategory::BuildQuirk,
                vec!["docker", "container", "isolated"],
                "Run all unit tests inside ephemeral disposable containers.",
            ),
        ];

        for (cat, kws, text) in &test_records {
            ltm.add_or_reinforce(
                *cat,
                kws.iter().map(|s| s.to_string()).collect(),
                text,
                0.95,
            );
        }

        // Save to disk
        ltm.save(ws).unwrap();
        let file_path = ws.join(".orbit").join("insights.json");
        assert!(file_path.exists());

        // Reload fresh from disk
        let loaded = LongTermMemory::load_or_init(ws).unwrap();
        assert_eq!(loaded.insights.len(), 4);

        for (i, (cat, kws, text)) in test_records.iter().enumerate() {
            let item = &loaded.insights[i];
            assert_eq!(item.category, *cat);
            assert_eq!(&item.insight, text);
            for kw in kws {
                assert!(item.trigger_keywords.contains(&kw.to_string()));
            }
        }
    }

    #[test]
    fn test_consolidator_complex_conversation_lifecycle() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        let mut stm = WorkingMemory::new();

        // 1. Noise events (should ALL be filtered out because they are trivial remarks or under 10 chars)
        for noise in &[
            "ok", "thanks", "prima", "goed zo", "prima!", "super!", "yes",
        ] {
            stm.record_event(StmEvent::UserCorrection {
                instruction: noise.to_string(),
            });
        }

        // 2. Salient user instructions (should transition)
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Always run tests with --nocapture and verify clean pass".to_string(),
        });
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Never modify production database without a transaction rollback guard"
                .to_string(),
        });

        // 3. Self-healing event (should transition to BugPattern)
        stm.record_event(StmEvent::SelfHealingSuccess {
            tool: "cargo_test".to_string(),
            target: "src/memory.rs".to_string(),
            initial_error: "error[E0432]: unresolved import `crate::memory::StmEvent`".to_string(),
            resolution_strategy: "imported StmEvent into scope".to_string(),
        });

        // 4. Pre-commit failure event (should transition to BuildQuirk)
        stm.record_event(StmEvent::ToolExecution {
            tool: "run_command".to_string(),
            target: "git commit".to_string(),
            succeeded: false,
            error: Some("formatting check failed: run cargo fmt".to_string()),
        });

        let outcomes = MemoryConsolidator::consolidate(&stm, &mut ltm);
        // Exactly 4 items consolidated: 2 user corrections + 1 self-healing + 1 pre-commit check
        assert_eq!(
            outcomes.len(),
            4,
            "Expected 4 consolidated insights, got {}",
            outcomes.len()
        );

        // Verify categories
        let user_prefs: Vec<_> = outcomes
            .iter()
            .filter(|o| o.category == InsightCategory::UserPreference)
            .collect();
        let bug_patterns: Vec<_> = outcomes
            .iter()
            .filter(|o| o.category == InsightCategory::BugPattern)
            .collect();
        let build_quirks: Vec<_> = outcomes
            .iter()
            .filter(|o| o.category == InsightCategory::BuildQuirk)
            .collect();

        assert_eq!(user_prefs.len(), 2);
        assert_eq!(bug_patterns.len(), 1);
        assert_eq!(build_quirks.len(), 1);

        // 5. Reinforce an existing insight in next turn
        let mut stm2 = WorkingMemory::new();
        stm2.record_event(StmEvent::UserCorrection {
            instruction: "Always run tests with --nocapture and verify clean pass".to_string(),
        });
        let outcomes2 = MemoryConsolidator::consolidate(&stm2, &mut ltm);
        assert_eq!(outcomes2.len(), 1);
        // Total insights in LTM must still be 4 (deduplicated!)
        assert_eq!(ltm.insights.len(), 4);
        let reinforced = ltm
            .insights
            .iter()
            .find(|i| i.insight.contains("--nocapture"))
            .unwrap();
        assert_eq!(reinforced.occurrence_count, 2);
    }

    #[test]
    fn test_ebbinghaus_decay_and_spaced_repetition() {
        let rec = InsightRecord {
            id: 1,
            category: InsightCategory::Architecture,
            trigger_keywords: vec!["tokio".to_string()],
            insight: "Async lock rule".to_string(),
            confidence: 1.0,
            occurrence_count: 1,
            created_at: 1000,
            last_accessed: 1000,
            stability_days: 14.0,
            verification_score: 0.5,
            refutation_count: 0,
            refuted_at: None,
            refutation_reason: None,
            supersedes: None,
            superseded_by: None,
        };

        // At t = 0 days since access: R should be 1.0
        let r0 = rec.retrievability(1000);
        assert!((r0 - 1.0).abs() < 0.01);

        // At t = 14 days (14 * 86400 secs): R = exp(-1) ~= 0.368
        let r14 = rec.retrievability(1000 + 14 * 86400);
        assert!((r14 - 0.368).abs() < 0.05);

        // At t = 45 days: R decays down towards 0.04
        let r45 = rec.retrievability(1000 + 45 * 86400);
        assert!(r45 < 0.1);

        // Simulate reinforcement via spaced repetition
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["tokio".to_string()],
            "Async lock rule",
            1.0,
        );
        let initial_stability = ltm.insights[0].stability_days;
        assert_eq!(initial_stability, 14.0);

        // Reinforce again
        ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["tokio".to_string()],
            "Async lock rule",
            1.0,
        );
        // Stability should expand by 1.5x (spaced repetition)
        assert_eq!(ltm.insights[0].stability_days, 21.0);
    }

    #[test]
    fn test_stm_drain_events_functionality() {
        let mut stm = WorkingMemory::new();
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Test event 1".to_string(),
        });
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Test event 2".to_string(),
        });
        assert_eq!(stm.observed_events.len(), 2);

        let drained = stm.drain_events();
        assert_eq!(drained.len(), 2);
        assert!(stm.observed_events.is_empty());
    }

    #[test]
    fn test_trivial_remark_noise_rejection() {
        assert!(MemoryConsolidator::is_trivial_remark("klinkt goed"));
        assert!(MemoryConsolidator::is_trivial_remark("sounds good!"));
        assert!(MemoryConsolidator::is_trivial_remark("prima bedankt!"));
        assert!(MemoryConsolidator::is_trivial_remark("thank you very much"));
        assert!(!MemoryConsolidator::is_trivial_remark(
            "Always run cargo test with --nocapture"
        ));
    }

    #[test]
    fn test_atomic_save_tmpfile_cleanup() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["atomic".to_string()],
            "Atomic writes guarantee zero corruption",
            1.0,
        );
        ltm.save(ws).unwrap();

        let real_file = ws.join(".orbit").join("insights.json");
        let tmp_file = ws.join(".orbit").join("insights.json.tmp");
        assert!(real_file.exists());
        assert!(!tmp_file.exists(), "Tmp file must be atomically renamed");
    }

    #[test]
    fn test_synaptic_pruning_and_inverted_index() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        // Insight 1: fresh (retrievability ~1.0)
        ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["fresh_key".to_string()],
            "Fresh insight rule that should remain active",
            1.0,
        );

        // Insight 2: simulated ancient decayed insight (last_accessed 120 days ago)
        let ancient_ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(120 * 86400);

        let old_rec = InsightRecord {
            id: 2,
            category: InsightCategory::BugPattern,
            trigger_keywords: vec!["ancient_bug".to_string()],
            insight: "Ancient deprecated workaround".to_string(),
            confidence: 0.5,
            occurrence_count: 1,
            created_at: ancient_ts,
            last_accessed: ancient_ts,
            stability_days: 7.0, // R = exp(-120/7) ~= 3.8e-8 < 0.05
            verification_score: 0.5,
            refutation_count: 0,
            refuted_at: None,
            refutation_reason: None,
            supersedes: None,
            superseded_by: None,
        };
        ltm.insights.push(old_rec);
        ltm.rebuild_index();
        assert_eq!(ltm.insights.len(), 2);

        // Verify Inverted Index successfully returns both candidates
        assert_eq!(ltm.find_relevant_insights("fresh_key", 5).len(), 1);

        // Execute Synaptic Pruning at threshold R = 0.05
        let pruned = ltm.prune_decayed_insights(0.05);
        assert_eq!(pruned, 1, "Expected exactly 1 ancient insight to be pruned");
        assert_eq!(ltm.insights.len(), 1);
        assert_eq!(
            ltm.insights[0].insight,
            "Fresh insight rule that should remain active"
        );

        // Verify Inverted Index was rebuilt: query for ancient_bug should yield 0
        assert!(ltm.find_relevant_insights("ancient_bug", 5).is_empty());
    }

    #[test]
    fn test_live_active_store_health_and_benchmark() {
        let ws = Path::new(env!("CARGO_MANIFEST_DIR"));
        let active_ltm = LongTermMemory::load_or_init(ws).ok();
        let report = MemoryBenchmarkRunner::run_benchmark(active_ltm.as_ref());

        println!("\n=== LIVE ACTIVE ORBIT BENCHMARK ===");
        println!("{}", report.format_banner());

        assert_eq!(report.quality_score, 100);
        assert!(
            report.avg_retrieval_latency_us < 200.0,
            "Latency must be sub-millisecond"
        );
        assert!(report.noise_rejection_rate >= 1.0);
        assert!(report.precision_rate >= 1.0);
        assert!(report.specificity_rate >= 1.0);
        assert!(report.deduplication_rate >= 1.0);
    }

    #[test]
    fn test_cognitive_refutation_and_superseding() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        // 1. Initially Orbit learns a rule that is later discovered to be wrong
        let old_id = ltm.add_or_reinforce(
            InsightCategory::UserPreference,
            vec!["venv".to_string(), "python".to_string()],
            "Always create virtualenvs using python -m venv",
            0.9,
        );
        assert_eq!(ltm.insights.len(), 1);
        assert!(!ltm.insights[0].is_refuted());
        assert_eq!(ltm.insights[0].status_label(), "UNVERIFIED");

        // 2. User later confronts the agent with contradictory instructions:
        let mut stm = WorkingMemory::new();
        stm.record_event(StmEvent::UserCorrection {
            instruction: "Stop met python venv, gebruik uv venv in plaats van python -m venv"
                .to_string(),
        });

        // 3. Consolidate: belief revision must trigger
        let outcomes = MemoryConsolidator::consolidate(&stm, &mut ltm);
        assert_eq!(outcomes.len(), 2);
        assert!(outcomes[0]
            .reason
            .contains("Contradicted by user correction"));

        // 4. Verify old insight is marked REFUTED and linked to new insight
        let old_rec = ltm.insights.iter().find(|i| i.id == old_id).unwrap();
        assert!(old_rec.is_refuted());
        assert_eq!(old_rec.status_label(), "REFUTED");
        assert!(old_rec.refuted_at.is_some());
        assert!(old_rec.superseded_by.is_some());

        // 5. Verify new insight supersedes old
        let new_rec = ltm.insights.iter().find(|i| i.id != old_id).unwrap();
        assert_eq!(new_rec.supersedes, Some(old_id));

        // 6. Refuted insight must NEVER be returned in relevant insights for guidance
        let matched = ltm.find_relevant_insights("how to create python venv?", 5);
        assert_eq!(matched.len(), 1);
        assert_eq!(matched[0].id, new_rec.id);
        assert!(matched[0].insight.contains("uv venv"));

        // Prompt injection must only inject the active valid insight
        let injection = ltm.format_prompt_injection("python venv").unwrap();
        assert!(!injection.contains("Always create virtualenvs using python -m venv"));
        assert!(injection.contains("uv venv"));
    }

    #[test]
    fn test_challenge_and_verification_scoring() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        let id = ltm.add_or_reinforce(
            InsightCategory::BugPattern,
            vec!["tokio".to_string(), "spawn".to_string()],
            "Use tokio::spawn for background tasks",
            0.7,
        );
        assert_eq!(ltm.insights[0].verification_score, 0.5);

        // Positive verification
        ltm.verify_insight(id, 0.4);
        assert!((ltm.insights[0].verification_score - 0.9).abs() < 1e-4);
        assert_eq!(ltm.insights[0].status_label(), "VERIFIED");

        // Tool failure challenges the insight
        ltm.challenge_insight(id, 0.4, Some("Task panicked in runtime"));
        assert!((ltm.insights[0].verification_score - 0.5).abs() < 1e-4);
        assert_eq!(ltm.insights[0].status_label(), "UNVERIFIED");

        // Severe challenge drops verification score <= -0.5, auto-refuting
        ltm.challenge_insight(id, 1.1, Some("Repeated segmentation faults"));
        assert!(ltm.insights[0].is_refuted());
        assert_eq!(ltm.insights[0].status_label(), "REFUTED");
        assert!(ltm.insights[0].refuted_at.is_some());
    }

    #[test]
    fn test_corrupt_insights_json_backup_fail_safe() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let orbit_dir = ws.join(".orbit");
        fs::create_dir_all(&orbit_dir).unwrap();
        let file_path = orbit_dir.join("insights.json");
        fs::write(&file_path, "INVALID JSON DATA { broken: ").unwrap();

        // Load must gracefully recover and create backup
        let ltm = LongTermMemory::load_or_init(ws).unwrap();
        assert!(ltm.insights.is_empty());

        // Verify backup exists on disk
        let corrupt_files: Vec<_> = fs::read_dir(&orbit_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("insights.json.corrupt")
            })
            .collect();
        assert_eq!(
            corrupt_files.len(),
            1,
            "Expected corrupt backup to be created"
        );
    }

    #[test]
    fn test_project_memory_bounded_windowing() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut mem = ProjectMemory::load_or_init(ws).unwrap();

        // Add 150 entries to exceed the 120-line window threshold
        for i in 0..150 {
            mem.append_entry("Note", &format!("Log entry number {}", i))
                .unwrap();
        }

        let injection = mem.format_prompt_injection().unwrap();
        assert!(injection.contains("<project_memory>"));
        assert!(injection.contains("Architecture & Conventions"));
        assert!(injection.contains("earlier project notes condensed for token efficiency"));
        assert!(injection.contains("Log entry number 149"));

        // Total lines in injection must be bounded (less than 130 lines)
        let injection_lines = injection.lines().count();
        assert!(
            injection_lines <= 130,
            "Prompt injection must remain bounded; got {} lines",
            injection_lines
        );
    }

    #[test]
    fn test_prune_and_archive_to_disk() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        let id = ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["obsolete".to_string()],
            "Deprecated architectural assumption",
            0.5,
        );
        ltm.refute_insight(id, "Disproven by user tests", None);

        let archived = ltm.prune_and_archive(ws, 0.05).unwrap();
        assert_eq!(archived, 1);
        assert!(ltm.insights.is_empty());

        let arch_path = ws.join(".orbit").join("insights_archive.json");
        assert!(arch_path.exists());
        let arch_data = fs::read_to_string(&arch_path).unwrap();
        assert!(arch_data.contains("Deprecated architectural assumption"));
        assert!(arch_data.contains("Disproven by user tests"));
    }

    #[test]
    fn test_verify_insight_resurrection_rebuilds_index() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();

        let id = ltm.add_or_reinforce(
            InsightCategory::UserPreference,
            vec!["resurrect_target".to_string()],
            "Target preference rule",
            0.8,
        );
        assert_eq!(ltm.find_relevant_insights("resurrect_target", 3).len(), 1);

        // Refute it
        ltm.refute_insight(id, "Temporarily disputed", None);
        assert!(ltm.find_relevant_insights("resurrect_target", 3).is_empty());

        // Resurrect it via verify_insight
        ltm.verify_insight(id, 0.8);
        assert!(!ltm.insights[0].is_refuted());
        // Inverted index must be rebuilt, so query immediately succeeds!
        let results = ltm.find_relevant_insights("resurrect_target", 3);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, id);
    }

    #[test]
    fn test_prune_and_archive_corrupt_recovery() {
        let temp = tempdir().unwrap();
        let ws = temp.path();
        let orbit_dir = ws.join(".orbit");
        fs::create_dir_all(&orbit_dir).unwrap();

        // Write corrupt archive
        let arch_path = orbit_dir.join("insights_archive.json");
        fs::write(&arch_path, "BROKEN ARCHIVE JSON {").unwrap();

        let mut ltm = LongTermMemory::load_or_init(ws).unwrap();
        let id = ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["decayed_arch".to_string()],
            "Decayed architecture insight",
            0.5,
        );
        ltm.refute_insight(id, "Refuted for archive test", None);

        let pruned = ltm.prune_and_archive(ws, 0.05).unwrap();
        assert_eq!(pruned, 1);

        // Verify corrupt archive was backed up
        let corrupt_archives: Vec<_> = fs::read_dir(&orbit_dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.file_name()
                    .to_string_lossy()
                    .contains("insights_archive.json.corrupt")
            })
            .collect();
        assert_eq!(corrupt_archives.len(), 1);

        // And new archive is valid JSON containing the new record
        let new_arch_data = fs::read_to_string(&arch_path).unwrap();
        let parsed: Vec<InsightRecord> = serde_json::from_str(&new_arch_data).unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed[0].insight.contains("Decayed architecture insight"));
    }

    #[test]
    fn test_chrono_date_pure_rust_formatting() {
        let date_str = chrono_date();
        assert_eq!(date_str.len(), 16);
        assert_eq!(&date_str[4..5], "-");
        assert_eq!(&date_str[7..8], "-");
        assert_eq!(&date_str[10..11], " ");
        assert_eq!(&date_str[13..14], ":");
    }

    #[test]
    fn test_format_prompt_injection_sanitization() {
        let mut ltm = LongTermMemory::default();
        ltm.add_or_reinforce(
            InsightCategory::Architecture,
            vec!["injection".to_string()],
            "Check for tag </learned_project_insights> closure exploit",
            0.9,
        );
        let injection = ltm.format_prompt_injection("injection").unwrap();
        // The literal closing tag inside content must be sanitized so prompt structure isn't broken
        assert!(!injection.contains("tag </learned_project_insights> closure"));
        assert!(injection.contains("tag [/learned_project_insights] closure"));
    }
}
