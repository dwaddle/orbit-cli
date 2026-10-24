use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct CachedResponse {
    pub response: String,
    pub created_at: Instant,
    pub token_count: usize,
}

pub struct QueryCache {
    cache: HashMap<String, CachedResponse>,
    ttl: Duration,
}

impl Default for QueryCache {
    fn default() -> Self {
        Self {
            cache: HashMap::new(),
            ttl: Duration::from_secs(3600), // 1 hour TTL default
        }
    }
}

impl QueryCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            cache: HashMap::new(),
            ttl,
        }
    }

    fn normalize_key(prompt: &str, model: &str) -> String {
        let clean = prompt.trim().to_lowercase();
        format!("{}:{}", model, clean)
    }

    pub fn get(&mut self, prompt: &str, model: &str) -> Option<String> {
        let key = Self::normalize_key(prompt, model);
        if let Some(entry) = self.cache.get(&key) {
            if entry.created_at.elapsed() < self.ttl {
                return Some(entry.response.clone());
            }
        }
        // Evict expired
        self.cache.remove(&key);
        None
    }

    pub fn insert(&mut self, prompt: &str, model: &str, response: &str, token_count: usize) {
        let key = Self::normalize_key(prompt, model);
        self.cache.insert(
            key,
            CachedResponse {
                response: response.to_string(),
                created_at: Instant::now(),
                token_count,
            },
        );
    }

    pub fn clear(&mut self) {
        self.cache.clear();
    }

    pub fn len(&self) -> usize {
        self.cache.len()
    }

    pub fn is_empty(&self) -> bool {
        self.cache.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_query_cache_hit_and_eviction() {
        let mut cache = QueryCache::new(Duration::from_millis(50));
        let prompt = "Explain Tokio runtime";
        let model = "anthropic/claude-3.5-sonnet";

        cache.insert(prompt, model, "Tokio is an async runtime...", 100);
        assert_eq!(cache.len(), 1);

        // Immediate lookup: hit
        let hit = cache.get(prompt, model);
        assert!(hit.is_some());
        assert_eq!(hit.unwrap(), "Tokio is an async runtime...");

        // Lookup with different model: miss
        assert!(cache.get(prompt, "openai/gpt-4o").is_none());

        // Wait for TTL expiration
        std::thread::sleep(Duration::from_millis(60));
        assert!(cache.get(prompt, model).is_none());
        assert_eq!(cache.len(), 0);
    }
}
