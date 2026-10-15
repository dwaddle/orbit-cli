use anyhow::Result;
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
