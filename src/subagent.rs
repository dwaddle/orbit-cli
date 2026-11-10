use anyhow::Result;
use colored::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum SubagentStatus {
    Running,
    Completed,
    Failed(String),
}

#[derive(Debug, Clone)]
pub struct SubagentTask {
    pub id: usize,
    pub role: String,
    pub model: String,
    pub prompt: String,
    pub status: SubagentStatus,
    pub result: Option<String>,
    pub memory_primed: bool,
}

#[derive(Clone, Default)]
pub struct SubagentManager {
    tasks: Arc<std::sync::Mutex<Vec<SubagentTask>>>,
    next_id: Arc<std::sync::Mutex<usize>>,
}

impl SubagentManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(std::sync::Mutex::new(Vec::new())),
            next_id: Arc::new(std::sync::Mutex::new(1)),
        }
    }

    pub async fn spawn_subagent(
        &self,
        role: String,
        model: String,
        prompt: String,
        api_key: String,
    ) -> usize {
        self.spawn_subagent_with_memory(role, model, prompt, api_key, None)
            .await
    }

    pub async fn spawn_subagent_with_memory(
        &self,
        role: String,
        model: String,
        prompt: String,
        api_key: String,
        memory_context: Option<String>,
    ) -> usize {
        let id = {
            let mut id_guard = self.next_id.lock().unwrap();
            let id = *id_guard;
            *id_guard += 1;
            id
        };

        let is_primed = memory_context.is_some();
        let task = SubagentTask {
            id,
            role: role.clone(),
            model: model.clone(),
            prompt: prompt.clone(),
            status: SubagentStatus::Running,
            result: None,
            memory_primed: is_primed,
        };

        {
            let mut tasks_guard = self.tasks.lock().unwrap();
            tasks_guard.push(task);
        }

        let tasks_clone = Arc::clone(&self.tasks);
        let id_copy = id;

        tokio::spawn(async move {
            let res = run_subagent_task_with_memory(
                &model,
                &role,
                &prompt,
                &api_key,
                memory_context.as_deref(),
            )
            .await;
            let mut tasks_guard = tasks_clone.lock().unwrap();
            if let Some(t) = tasks_guard.iter_mut().find(|t| t.id == id_copy) {
                match res {
                    Ok(output) => {
                        t.status = SubagentStatus::Completed;
                        t.result = Some(output);
                    }
                    Err(e) => {
                        t.status = SubagentStatus::Failed(e.to_string());
                        t.result = Some(format!("Error executing subagent: {}", e));
                    }
                }
            }
        });

        id
    }

    pub fn list_subagents(&self) -> Vec<SubagentTask> {
        let tasks_guard = self.tasks.lock().unwrap();
        tasks_guard.clone()
    }

    /// Executes a directed acyclic graph (DAG) of subagent tasks concurrently with channel messaging.
    pub async fn run_dag(&self, dag: SubagentDag, api_key: &str) -> Result<String> {
        self.run_dag_with_memory(dag, api_key, None, None).await
    }

    /// Executes a DAG with full neurocognitive memory integration:
    /// - Dynamic prompt priming from Long-Term Memory (LTM) and Working Memory (STM).
    /// - Working memory episodic event recording and failure tracking.
    /// - Autonomous post-DAG knowledge consolidation.
    pub async fn run_dag_with_memory(
        &self,
        mut dag: SubagentDag,
        api_key: &str,
        ltm: Option<&std::sync::Mutex<crate::memory::LongTermMemory>>,
        stm: Option<&std::sync::Mutex<crate::memory::WorkingMemory>>,
    ) -> Result<String> {
        dag.validate()?;

        // Dynamic cognitive memory priming across all nodes prior to execution
        if ltm.is_some() || stm.is_some() {
            let ltm_guard = ltm.map(|m| m.lock().unwrap());
            let stm_guard = stm.map(|m| m.lock().unwrap());
            dag.prime_nodes(ltm_guard.as_deref(), stm_guard.as_deref());
        }

        println!(
            "\n{}",
            format!(
                "=== 🕸️ EXECUTING SUBAGENT DAG: {} ({} nodes) ===",
                dag.name,
                dag.nodes.len()
            )
            .bold()
            .cyan()
        );

        let total_nodes = dag.nodes.len();
        let mut completed_count = 0;

        while completed_count < total_nodes {
            // Find nodes ready to run (dependencies completed and no output yet)
            let ready_indices: Vec<usize> = dag
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    n.output.is_none()
                        && n.status == SubagentStatus::Running
                        && n.depends_on.iter().all(|dep_id| {
                            dag.nodes
                                .iter()
                                .any(|d| d.id == *dep_id && d.status == SubagentStatus::Completed)
                        })
                })
                .map(|(idx, _)| idx)
                .collect();

            if ready_indices.is_empty() {
                let uncompleted = dag.nodes.iter().any(|n| n.output.is_none());
                if uncompleted {
                    return Err(anyhow::anyhow!(
                        "DAG execution stalled: upstream dependency failure."
                    ));
                }
                break;
            }

            let mut join_handles = Vec::new();
            for &idx in &ready_indices {
                let node = &dag.nodes[idx];
                let node_id = node.id;
                let name = node.name.clone();
                let role = node.role.clone();
                let model = node.model.clone();

                let mut full_prompt = String::new();
                for &dep_id in &node.depends_on {
                    if let Some(dep_node) = dag.nodes.iter().find(|d| d.id == dep_id) {
                        if let Some(ref dep_out) = dep_node.output {
                            full_prompt.push_str(&format!(
                                "[Context from Dependency Task #{} ({} - {})]:\n{}\n\n",
                                dep_node.id,
                                dep_node.name,
                                dep_node.role,
                                dep_out.trim()
                            ));
                        }
                    }
                }
                full_prompt.push_str(&node.prompt);

                let memory_context = node.memory_injected.clone();
                let key = api_key.to_string();
                let primed_badge = if memory_context.is_some() {
                    " [Cognitive Priming Active]".magenta()
                } else {
                    "".normal()
                };
                println!(
                    "  🚀 Starting DAG Node #{} [{}] ({}){}",
                    node_id,
                    name.bold(),
                    role.cyan(),
                    primed_badge
                );

                join_handles.push(tokio::spawn(async move {
                    let res = run_subagent_task_with_memory(
                        &model,
                        &role,
                        &full_prompt,
                        &key,
                        memory_context.as_deref(),
                    )
                    .await;
                    (node_id, res)
                }));
            }

            let mut failed_err = None;

            for i in 0..join_handles.len() {
                let await_res = (&mut join_handles[i]).await;
                match await_res {
                    Ok((node_id, Ok(out))) => {
                        if let Some(node) = dag.nodes.iter_mut().find(|n| n.id == node_id) {
                            println!(
                                "  ✅ Completed DAG Node #{} [{}]",
                                node.id,
                                node.name.green()
                            );
                            node.status = SubagentStatus::Completed;
                            node.output = Some(out.clone());
                            completed_count += 1;

                            if let Some(stm_mutex) = stm {
                                if let Ok(mut stm_guard) = stm_mutex.lock() {
                                    let summary = out
                                        .lines()
                                        .find(|l| !l.trim().is_empty())
                                        .unwrap_or(&out)
                                        .trim();
                                    stm_guard.record_subagent(
                                        &dag.name, &node.name, &node.role, true, summary,
                                    );
                                }
                            }
                        }
                    }
                    Ok((node_id, Err(e))) => {
                        if let Some(node) = dag.nodes.iter_mut().find(|n| n.id == node_id) {
                            println!(
                                "  ❌ Failed DAG Node #{} [{}]: {}",
                                node.id,
                                node.name.red(),
                                e
                            );
                            node.status = SubagentStatus::Failed(e.to_string());
                            node.output = Some(format!("Error: {}", e));

                            if let Some(stm_mutex) = stm {
                                if let Ok(mut stm_guard) = stm_mutex.lock() {
                                    stm_guard.record_subagent(
                                        &dag.name,
                                        &node.name,
                                        &node.role,
                                        false,
                                        &e.to_string(),
                                    );
                                }
                            }
                            failed_err = Some(anyhow::anyhow!(
                                "DAG task #{} '{}' failed: {}",
                                node.id,
                                node.name,
                                e
                            ));
                        } else {
                            failed_err =
                                Some(anyhow::anyhow!("DAG task #{} failed: {}", node_id, e));
                        }

                        // Abort all remaining handles to prevent zombie leaks
                        for h in join_handles.iter().skip(i + 1) {
                            h.abort();
                        }
                        break;
                    }
                    Err(join_err) => {
                        failed_err = Some(anyhow::anyhow!("DAG task join error: {}", join_err));
                        for h in join_handles.iter().skip(i + 1) {
                            h.abort();
                        }
                        break;
                    }
                }
            }

            if let Some(err) = failed_err {
                return Err(err);
            }
        }

        // Autonomous post-DAG memory consolidation
        let mut consolidations = Vec::new();
        if let Some(ltm_mutex) = ltm {
            if let Ok(mut ltm_guard) = ltm_mutex.lock() {
                let direct_outcomes = consolidate_dag_nodes(&dag, &mut ltm_guard);
                consolidations.extend(direct_outcomes);

                if let Some(stm_mutex) = stm {
                    if let Ok(stm_guard) = stm_mutex.lock() {
                        let stm_outcomes = crate::memory::MemoryConsolidator::consolidate(
                            &stm_guard,
                            &mut ltm_guard,
                        );
                        consolidations.extend(stm_outcomes);
                    }
                }
            }
        }

        let mut report = format!("### 🕸️ DAG Execution Report: {}\n\n", dag.name);
        for node in &dag.nodes {
            report.push_str(&format!(
                "#### Node #{} [{}] ({})\n",
                node.id, node.name, node.role
            ));
            report.push_str(&format!("**Dependencies**: {:?}\n\n", node.depends_on));
            if let Some(ref mem) = node.memory_injected {
                report.push_str(&format!(
                    "*Cognitive Memory Priming Active ({} bytes)*\n\n",
                    mem.len()
                ));
            }
            if let Some(ref out) = node.output {
                report.push_str(&format!("{}\n\n---\n\n", out.trim()));
            }
        }

        if !consolidations.is_empty() {
            report.push_str("### 🧠 Neurocognitive Long-Term Memory Consolidations\n\n");
            for outcome in &consolidations {
                report.push_str(&format!(
                    "- **[{}]** {} *(Reason: {})*\n",
                    outcome.category.as_str(),
                    outcome.insight_text,
                    outcome.reason
                ));
            }
        }

        Ok(report)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DagNode {
    pub id: usize,
    pub name: String,
    pub role: String,
    pub model: String,
    pub prompt: String,
    pub depends_on: Vec<usize>,
    pub status: SubagentStatus,
    pub output: Option<String>,
    #[serde(default)]
    pub memory_injected: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentDag {
    pub name: String,
    pub nodes: Vec<DagNode>,
}

impl SubagentDag {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            nodes: Vec::new(),
        }
    }

    pub fn add_node(
        &mut self,
        name: &str,
        role: &str,
        model: &str,
        prompt: &str,
        depends_on: Vec<usize>,
    ) -> usize {
        let id = self.nodes.len() + 1;
        self.nodes.push(DagNode {
            id,
            name: name.to_string(),
            role: role.to_string(),
            model: model.to_string(),
            prompt: prompt.to_string(),
            depends_on,
            status: SubagentStatus::Running,
            output: None,
            memory_injected: None,
        });
        id
    }

    /// Primes all DAG nodes with Long-Term Memory (LTM) insights and Short-Term Working Memory (STM) context.
    pub fn prime_nodes(
        &mut self,
        ltm: Option<&crate::memory::LongTermMemory>,
        stm: Option<&crate::memory::WorkingMemory>,
    ) {
        for node in &mut self.nodes {
            let mut parts = Vec::new();
            if let Some(ltm_ref) = ltm {
                if let Some(injected) = ltm_ref.format_prompt_injection(&node.prompt) {
                    parts.push(injected);
                }
            }
            if let Some(stm_ref) = stm {
                let scratchpad = stm_ref.format_scratchpad();
                if !scratchpad.trim().is_empty() {
                    parts.push(format!(
                        "<working_memory_scratchpad>\n{}\n</working_memory_scratchpad>",
                        scratchpad.trim()
                    ));
                }
            }
            if !parts.is_empty() {
                node.memory_injected = Some(parts.join("\n\n"));
            }
        }
    }

    /// Validates DAG for acyclicity and valid dependency IDs.
    pub fn validate(&self) -> Result<()> {
        let ids: std::collections::HashSet<usize> = self.nodes.iter().map(|n| n.id).collect();
        for node in &self.nodes {
            for dep in &node.depends_on {
                if !ids.contains(dep) {
                    return Err(anyhow::anyhow!(
                        "DAG Node '{}' depends on non-existent node #{}",
                        node.name,
                        dep
                    ));
                }
                if *dep >= node.id {
                    return Err(anyhow::anyhow!(
                        "DAG dependency cycle detected: node #{} cannot depend on #{}",
                        node.id,
                        dep
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Autonomous consolidation of findings from completed DAG nodes directly into LongTermMemory.
pub fn consolidate_dag_nodes(
    dag: &SubagentDag,
    ltm: &mut crate::memory::LongTermMemory,
) -> Vec<crate::memory::ConsolidationOutcome> {
    let mut outcomes = Vec::new();
    for node in &dag.nodes {
        if let Some(ref output) = node.output {
            for line in output.lines() {
                let trimmed = line.trim();
                let is_vuln = trimmed.starts_with("- [CRITICAL]")
                    || trimmed.starts_with("- [VULNERABILITY]")
                    || trimmed.starts_with("- [SECURITY]")
                    || trimmed.starts_with("CRITICAL:")
                    || trimmed.starts_with("VULNERABILITY:")
                    || (trimmed.starts_with("- ")
                        && (trimmed.contains("vulnerability")
                            || trimmed.contains("security risk")
                            || trimmed.contains("SQL injection")));

                let is_arch = trimmed.starts_with("- [ARCHITECTURE]")
                    || trimmed.starts_with("- [INVARIANT]")
                    || trimmed.starts_with("ARCHITECTURE:")
                    || (trimmed.starts_with("- ")
                        && (trimmed.contains("architectural invariant")
                            || trimmed.contains("design requirement")));

                if is_vuln {
                    let clean = trimmed.trim_start_matches(['-', '*', ' ']).trim();
                    let mut keywords =
                        crate::memory::MemoryConsolidator::extract_salient_keywords(clean);
                    keywords.push(node.role.to_lowercase());
                    if !keywords.is_empty() {
                        let id = ltm.add_or_reinforce(
                            crate::memory::InsightCategory::BugPattern,
                            keywords,
                            clean,
                            0.90,
                        );
                        outcomes.push(crate::memory::ConsolidationOutcome {
                            insight_id: id,
                            category: crate::memory::InsightCategory::BugPattern,
                            insight_text: clean.to_string(),
                            reason: format!(
                                "Consolidated security finding from DAG '{}' ({})",
                                dag.name, node.name
                            ),
                        });
                    }
                } else if is_arch {
                    let clean = trimmed.trim_start_matches(['-', '*', ' ']).trim();
                    let mut keywords =
                        crate::memory::MemoryConsolidator::extract_salient_keywords(clean);
                    keywords.push(node.role.to_lowercase());
                    if !keywords.is_empty() {
                        let id = ltm.add_or_reinforce(
                            crate::memory::InsightCategory::Architecture,
                            keywords,
                            clean,
                            0.85,
                        );
                        outcomes.push(crate::memory::ConsolidationOutcome {
                            insight_id: id,
                            category: crate::memory::InsightCategory::Architecture,
                            insight_text: clean.to_string(),
                            reason: format!(
                                "Consolidated architectural invariant from DAG '{}' ({})",
                                dag.name, node.name
                            ),
                        });
                    }
                }
            }
        }
    }
    outcomes
}

pub fn create_audit_dag(target: &str) -> SubagentDag {
    let mut dag = SubagentDag::new(&format!("Security & Quality Audit: {}", target));
    let t1 = dag.add_node(
        "Security Auditor",
        "Security Engineer",
        "deepseek/deepseek-r1",
        &format!(
            "Audit the codebase or file '{}' for vulnerabilities, injection bugs, and memory safety.",
            target
        ),
        vec![],
    );
    let t2 = dag.add_node(
        "QA Engineer",
        "QA & Test Architect",
        "google/gemini-2.5-flash",
        &format!(
            "Inspect test coverage and edge cases for '{}'. Identify missing regression assertions.",
            target
        ),
        vec![],
    );
    dag.add_node(
        "Lead Architect",
        "Principal System Architect",
        "anthropic/claude-sonnet-4.5",
        "Synthesize the Security and QA findings into an actionable, prioritized patch plan.",
        vec![t1, t2],
    );
    dag
}

pub fn create_feature_dag(feature_spec: &str) -> SubagentDag {
    let mut dag = SubagentDag::new(&format!("Feature Implementation: {}", feature_spec));
    let t1 = dag.add_node(
        "Architecture Explorer",
        "Codebase Explorer",
        "google/gemini-2.5-flash",
        &format!(
            "Explore relevant files, signatures, and patterns for: '{}'.",
            feature_spec
        ),
        vec![],
    );
    let t2 = dag.add_node(
        "Test Specifier",
        "Test Engineer",
        "anthropic/claude-sonnet-4.5",
        "Based on the architecture discovery, write detailed test specifications.",
        vec![t1],
    );
    let t3 = dag.add_node(
        "Implementation Coder",
        "Senior Software Engineer",
        "anthropic/claude-sonnet-4.5",
        "Implement the complete code satisfying the feature requirements.",
        vec![t1],
    );
    dag.add_node(
        "Integration Lead",
        "Release Architect",
        "anthropic/claude-sonnet-4.5",
        "Verify implementation against tests and produce the final integration summary.",
        vec![t2, t3],
    );
    dag
}

pub async fn run_subagent_task(
    model: &str,
    role: &str,
    prompt: &str,
    api_key: &str,
) -> Result<String> {
    run_subagent_task_with_memory(model, role, prompt, api_key, None).await
}

pub async fn run_subagent_task_with_memory(
    model: &str,
    role: &str,
    prompt: &str,
    api_key: &str,
    memory_context: Option<&str>,
) -> Result<String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(45))
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()?;

    let target_model = if model.is_empty() || model == "fast" {
        "google/gemini-2.5-flash"
    } else {
        model
    };

    let default_fallbacks = [
        "openai/gpt-4o".to_string(),
        "deepseek/deepseek-r1".to_string(),
        "google/gemini-2.5-flash".to_string(),
    ];
    let candidates = crate::routing::get_failover_candidates(target_model, &default_fallbacks);

    let sys_prompt = if let Some(mem) = memory_context {
        if !mem.trim().is_empty() {
            format!(
                "You are a subagent with role '{}'. Execute the requested task thoroughly, direct, and concisely.\n\n{}",
                role,
                mem.trim()
            )
        } else {
            format!(
                "You are a subagent with role '{}'. Execute the requested task thoroughly, direct, and concisely.",
                role
            )
        }
    } else {
        format!(
            "You are a subagent with role '{}'. Execute the requested task thoroughly, direct, and concisely.",
            role
        )
    };

    for (idx, candidate) in candidates.iter().enumerate() {
        let payload = serde_json::json!({
            "model": candidate,
            "messages": [
                {
                    "role": "system",
                    "content": [
                        {
                            "type": "text",
                            "text": &sys_prompt,
                            "cache_control": {
                                "type": "ephemeral"
                            }
                        }
                    ]
                },
                {"role": "user", "content": prompt}
            ],
            "temperature": 0.2
        });

        let resp_res = client
            .post("https://openrouter.ai/api/v1/chat/completions")
            .header("Authorization", format!("Bearer {}", api_key))
            .header("Content-Type", "application/json")
            .header("HTTP-Referer", "https://github.com/dwaddle/orbit-cli")
            .header("X-Title", "Orbit-CLI")
            .json(&payload)
            .send()
            .await;

        let resp = match resp_res {
            Ok(r) => r,
            Err(e) => {
                if idx + 1 < candidates.len() {
                    continue;
                } else {
                    return Err(anyhow::anyhow!("Subagent network error: {}", e));
                }
            }
        };

        if !resp.status().is_success() {
            let status = resp.status();
            if crate::routing::is_retryable_status(status.as_u16()) && idx + 1 < candidates.len() {
                continue;
            }
            let err = resp
                .text()
                .await
                .unwrap_or_else(|_| "Unknown error".to_string());
            return Err(anyhow::anyhow!(
                "OpenRouter API error (HTTP {}): {}",
                status.as_u16(),
                err
            ));
        }

        let body: serde_json::Value = resp.json().await?;
        let content = body["choices"][0]["message"]["content"]
            .as_str()
            .unwrap_or("")
            .to_string();

        return Ok(content);
    }

    Err(anyhow::anyhow!("Subagent failover chain exhausted."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dag_validation_valid() {
        let mut dag = SubagentDag::new("Test DAG");
        let t1 = dag.add_node("Task 1", "Tester", "model-a", "prompt 1", vec![]);
        let _t2 = dag.add_node("Task 2", "Reviewer", "model-b", "prompt 2", vec![t1]);
        assert!(dag.validate().is_ok());
        assert_eq!(dag.nodes.len(), 2);
    }

    #[test]
    fn test_dag_validation_missing_dependency() {
        let mut dag = SubagentDag::new("Broken DAG");
        dag.add_node("Task 1", "Tester", "model-a", "prompt 1", vec![99]);
        assert!(dag.validate().is_err());
    }

    #[test]
    fn test_dag_validation_cycle_prevention() {
        let mut dag = SubagentDag::new("Cycle DAG");
        // Cannot depend on self or future node
        dag.add_node("Task 1", "Tester", "model-a", "prompt 1", vec![1]);
        assert!(dag.validate().is_err());
    }

    #[test]
    fn test_create_audit_dag_structure() {
        let dag = create_audit_dag("src/main.rs");
        assert_eq!(dag.nodes.len(), 3);
        assert!(dag.validate().is_ok());
        // Lead Architect depends on first two nodes
        assert_eq!(dag.nodes[2].depends_on, vec![1, 2]);
    }

    #[test]
    fn test_create_feature_dag_structure() {
        let dag = create_feature_dag("Async file cache");
        assert_eq!(dag.nodes.len(), 4);
        assert!(dag.validate().is_ok());
        // Integration Lead depends on test specifier and implementation coder
        assert_eq!(dag.nodes[3].depends_on, vec![2, 3]);
    }

    #[test]
    fn test_subagent_dag_node_memory_priming() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = crate::memory::LongTermMemory::load_or_init(ws).unwrap();
        ltm.add_or_reinforce(
            crate::memory::InsightCategory::BuildQuirk,
            vec![
                "async".to_string(),
                "tokio".to_string(),
                "cache".to_string(),
            ],
            "Always use tokio::sync::RwLock for async multi-reader cache",
            0.9,
        );

        let mut stm = crate::memory::WorkingMemory::new();
        stm.set_goal("Implement async cache");
        stm.record_attempt(
            "cargo test cache",
            Some("error: deadlock detected in std::sync::RwLock"),
            false,
        );

        let mut dag = create_feature_dag("async cache subsystem");
        dag.prime_nodes(Some(&ltm), Some(&stm));

        let first_node = &dag.nodes[0];
        assert!(first_node.memory_injected.is_some());
        let injection = first_node.memory_injected.as_ref().unwrap();
        assert!(injection.contains("<learned_project_insights>"));
        assert!(injection.contains("tokio::sync::RwLock"));
        assert!(injection.contains("<working_memory_scratchpad>"));
        assert!(injection.contains("Active Goal: Implement async cache"));
        assert!(injection.contains("deadlock detected"));
    }

    #[test]
    fn test_consolidate_dag_nodes_into_ltm() {
        use tempfile::tempdir;
        let temp = tempdir().unwrap();
        let ws = temp.path();

        let mut ltm = crate::memory::LongTermMemory::load_or_init(ws).unwrap();

        let mut dag = SubagentDag::new("Security & Invariant Audit");
        let t1 = dag.add_node("Auditor", "Security Engineer", "model-a", "prompt", vec![]);
        let _t2 = dag.add_node(
            "Architect",
            "System Architect",
            "model-b",
            "prompt",
            vec![t1],
        );

        dag.nodes[0].output = Some(
            "Audit complete.\n- [CRITICAL] Hardcoded credentials in staging config bypass auth\n- Regular note"
                .to_string(),
        );
        dag.nodes[1].output = Some(
            "Review done.\n- [ARCHITECTURE] Maintain strict tenant isolation across all query boundaries\n- All good"
                .to_string(),
        );

        let outcomes = consolidate_dag_nodes(&dag, &mut ltm);
        assert_eq!(outcomes.len(), 2);

        let vuln_insight = ltm
            .insights
            .iter()
            .find(|i| i.category == crate::memory::InsightCategory::BugPattern)
            .expect("Expected BugPattern insight from critical vulnerability");
        assert!(vuln_insight.insight.contains("Hardcoded credentials"));

        let arch_insight = ltm
            .insights
            .iter()
            .find(|i| i.category == crate::memory::InsightCategory::Architecture)
            .expect("Expected Architecture insight from invariant");
        assert!(arch_insight.insight.contains("tenant isolation"));
    }
}
