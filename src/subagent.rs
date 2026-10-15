use anyhow::Result;
use colored::*;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::sync::Mutex;

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
}

#[derive(Clone, Default)]
pub struct SubagentManager {
    tasks: Arc<Mutex<Vec<SubagentTask>>>,
    next_id: Arc<Mutex<usize>>,
}

impl SubagentManager {
    pub fn new() -> Self {
        Self {
            tasks: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(Mutex::new(1)),
        }
    }

    pub async fn spawn_subagent(
        &self,
        role: String,
        model: String,
        prompt: String,
        api_key: String,
    ) -> usize {
        let mut id_guard = self.next_id.lock().await;
        let id = *id_guard;
        *id_guard += 1;

        let task = SubagentTask {
            id,
            role: role.clone(),
            model: model.clone(),
            prompt: prompt.clone(),
            status: SubagentStatus::Running,
            result: None,
        };

        {
            let mut tasks_guard = self.tasks.lock().await;
            tasks_guard.push(task);
        }

        let tasks_clone = Arc::clone(&self.tasks);
        let id_copy = id;

        tokio::spawn(async move {
            let res = run_subagent_task(&model, &role, &prompt, &api_key).await;
            let mut tasks_guard = tasks_clone.lock().await;
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

    pub async fn list_subagents(&self) -> Vec<SubagentTask> {
        let tasks_guard = self.tasks.lock().await;
        tasks_guard.clone()
    }

    /// Executes a directed acyclic graph (DAG) of subagent tasks concurrently with channel messaging.
    pub async fn run_dag(&self, mut dag: SubagentDag, api_key: &str) -> Result<String> {
        dag.validate()?;
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

                let key = api_key.to_string();
                println!(
                    "  🚀 Starting DAG Node #{} [{}] ({})",
                    node_id,
                    name.bold(),
                    role.cyan()
                );

                join_handles.push(tokio::spawn(async move {
                    let res = run_subagent_task(&model, &role, &full_prompt, &key).await;
                    (node_id, res)
                }));
            }

            for handle in join_handles {
                let (node_id, res) = handle.await?;
                if let Some(node) = dag.nodes.iter_mut().find(|n| n.id == node_id) {
                    match res {
                        Ok(out) => {
                            println!(
                                "  ✅ Completed DAG Node #{} [{}]",
                                node.id,
                                node.name.green()
                            );
                            node.status = SubagentStatus::Completed;
                            node.output = Some(out);
                            completed_count += 1;
                        }
                        Err(e) => {
                            println!(
                                "  ❌ Failed DAG Node #{} [{}]: {}",
                                node.id,
                                node.name.red(),
                                e
                            );
                            node.status = SubagentStatus::Failed(e.to_string());
                            node.output = Some(format!("Error: {}", e));
                            return Err(anyhow::anyhow!(
                                "DAG task #{} '{}' failed: {}",
                                node.id,
                                node.name,
                                e
                            ));
                        }
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
            if let Some(ref out) = node.output {
                report.push_str(&format!("{}\n\n---\n\n", out.trim()));
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
        });
        id
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

async fn run_subagent_task(model: &str, role: &str, prompt: &str, api_key: &str) -> Result<String> {
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

    let sys_prompt = format!(
        "You are a subagent with role '{}'. Execute the requested task thoroughly, direct, and concisely.",
        role
    );

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
}
