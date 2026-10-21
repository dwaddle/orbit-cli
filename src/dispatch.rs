use std::future::Future;

#[derive(Debug, Clone)]
pub struct ParsedTool {
    pub name: String,
    pub body: String,
}

pub fn is_read_only(tool_name: &str) -> bool {
    matches!(
        tool_name,
        "view_file" | "code_search" | "code_outline" | "code_map" | "git_context" | "code_grep"
    )
}

pub struct ToolDispatch;

impl ToolDispatch {
    /// Executes a list of tool calls, running contiguous batches of read-only tools
    /// concurrently while preserving strict sequential ordering for mutating tools.
    /// The final results are returned in the exact original index order.
    pub async fn execute_batch<F, Fut>(tools: Vec<ParsedTool>, executor: F) -> Vec<(String, String)>
    where
        F: Fn(String, String) -> Fut,
        Fut: Future<Output = String>,
    {
        if tools.is_empty() {
            return Vec::new();
        }

        let n = tools.len();
        let mut results: Vec<Option<(String, String)>> = (0..n).map(|_| None).collect();

        let mut read_only_batch: Vec<(usize, ParsedTool)> = Vec::new();

        for (idx, tool) in tools.into_iter().enumerate() {
            if is_read_only(&tool.name) {
                read_only_batch.push((idx, tool));
            } else {
                // If we have accumulated read-only tools, drain and execute them concurrently
                if !read_only_batch.is_empty() {
                    let batch = std::mem::take(&mut read_only_batch);
                    let futures: Vec<_> = batch
                        .into_iter()
                        .map(|(orig_idx, t)| {
                            let fut = executor(t.name.clone(), t.body.clone());
                            async move {
                                let res = fut.await;
                                (orig_idx, t.name, res)
                            }
                        })
                        .collect();

                    let batch_results = futures_util::future::join_all(futures).await;
                    for (orig_idx, name, res) in batch_results {
                        results[orig_idx] = Some((name, res));
                    }
                }

                // Execute the mutating tool sequentially
                let name = tool.name.clone();
                let res = executor(tool.name, tool.body).await;
                results[idx] = Some((name, res));
            }
        }

        // Drain any remaining trailing read-only tools
        if !read_only_batch.is_empty() {
            let futures: Vec<_> = read_only_batch
                .into_iter()
                .map(|(orig_idx, t)| {
                    let fut = executor(t.name.clone(), t.body.clone());
                    async move {
                        let res = fut.await;
                        (orig_idx, t.name, res)
                    }
                })
                .collect();

            let batch_results = futures_util::future::join_all(futures).await;
            for (orig_idx, name, res) in batch_results {
                results[orig_idx] = Some((name, res));
            }
        }

        results
            .into_iter()
            .enumerate()
            .map(|(i, r)| {
                r.unwrap_or_else(|| ("unknown".to_string(), format!("Tool {} failed", i)))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tokio::time::{sleep, Duration};

    #[test]
    fn test_is_read_only_classification() {
        assert!(is_read_only("view_file"));
        assert!(is_read_only("code_search"));
        assert!(is_read_only("code_outline"));
        assert!(is_read_only("code_map"));
        assert!(is_read_only("git_context"));
        assert!(is_read_only("code_grep"));

        assert!(!is_read_only("write_file"));
        assert!(!is_read_only("apply_patch"));
        assert!(!is_read_only("run_command"));
        assert!(!is_read_only("tdd_cycle"));
        assert!(!is_read_only("mcp"));
    }

    #[tokio::test]
    async fn test_concurrent_read_only_execution_ordering() {
        let tools = vec![
            ParsedTool {
                name: "view_file".to_string(),
                body: "file_a.rs".to_string(),
            },
            ParsedTool {
                name: "view_file".to_string(),
                body: "file_b.rs".to_string(),
            },
            ParsedTool {
                name: "write_file".to_string(),
                body: "file_c.rs\ncontent".to_string(),
            },
            ParsedTool {
                name: "code_outline".to_string(),
                body: "src/".to_string(),
            },
        ];

        let concurrency_peak = Arc::new(AtomicUsize::new(0));
        let active_tasks = Arc::new(AtomicUsize::new(0));

        let peak_clone = Arc::clone(&concurrency_peak);
        let active_clone = Arc::clone(&active_tasks);

        let results = ToolDispatch::execute_batch(tools, move |name, body| {
            let peak = Arc::clone(&peak_clone);
            let active = Arc::clone(&active_clone);
            async move {
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);

                // Simulate slight I/O duration
                sleep(Duration::from_millis(15)).await;

                active.fetch_sub(1, Ordering::SeqCst);
                format!("Executed {}: {}", name, body)
            }
        })
        .await;

        assert_eq!(results.len(), 4);
        assert_eq!(results[0].0, "view_file");
        assert!(results[0].1.contains("file_a.rs"));
        assert_eq!(results[1].0, "view_file");
        assert!(results[1].1.contains("file_b.rs"));
        assert_eq!(results[2].0, "write_file");
        assert!(results[2].1.contains("file_c.rs"));
        assert_eq!(results[3].0, "code_outline");
        assert!(results[3].1.contains("src/"));

        // Both read-only tools ran concurrently (concurrency >= 2)
        assert!(concurrency_peak.load(Ordering::SeqCst) >= 2);
    }
}
