//! Query caching for AI SQL generation

use crate::ai_sql::client::AiResponse;
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Entry in the query cache
#[derive(Debug, Clone)]
struct CacheEntry {
    response: AiResponse,
    created_at: Instant,
    ttl: Duration,
}

impl CacheEntry {
    fn is_expired(&self) -> bool {
        self.created_at.elapsed() > self.ttl
    }
}

/// Query cache for AI responses
pub struct QueryCache {
    cache: HashMap<String, CacheEntry>,
    default_ttl: Duration,
    hits: usize,
    misses: usize,
}

impl QueryCache {
    /// Create a new query cache
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
            default_ttl: Duration::from_secs(3600), // 1 hour default
            hits: 0,
            misses: 0,
        }
    }

    /// Create cache with custom TTL
    pub fn with_ttl(ttl_seconds: u64) -> Self {
        Self {
            cache: HashMap::new(),
            default_ttl: Duration::from_secs(ttl_seconds),
            hits: 0,
            misses: 0,
        }
    }

    /// Get cached response
    pub fn get(&mut self, key: &str) -> Option<AiResponse> {
        // Clean expired entries periodically
        if self.cache.len() > 100 {
            self.clean_expired();
        }

        if let Some(entry) = self.cache.get(key) {
            if entry.is_expired() {
                self.cache.remove(key);
                self.misses += 1;
                None
            } else {
                self.hits += 1;
                Some(entry.response.clone())
            }
        } else {
            self.misses += 1;
            None
        }
    }

    /// Insert response into cache
    pub fn insert(&mut self, key: String, response: AiResponse) {
        let entry = CacheEntry {
            response,
            created_at: Instant::now(),
            ttl: self.default_ttl,
        };
        self.cache.insert(key, entry);
    }

    /// Clear all cached entries
    pub fn clear(&mut self) {
        self.cache.clear();
        self.hits = 0;
        self.misses = 0;
    }

    /// Remove expired entries
    fn clean_expired(&mut self) {
        self.cache.retain(|_, entry| !entry.is_expired());
    }

    /// Get cache statistics (hits, misses)
    pub fn stats(&self) -> (usize, usize) {
        (self.hits, self.misses)
    }

    /// Get cache size
    pub fn size(&self) -> usize {
        self.cache.len()
    }

    /// Get hit rate
    pub fn hit_rate(&self) -> f64 {
        let total = self.hits + self.misses;
        if total == 0 {
            0.0
        } else {
            self.hits as f64 / total as f64
        }
    }
}

impl Default for QueryCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_test_response() -> AiResponse {
        AiResponse {
            sql: "SELECT * FROM users".to_string(),
            explanation: None,
            confidence: 0.9,
            warnings: vec![],
            suggestions: vec![],
        }
    }

    #[test]
    fn test_cache_insert_and_get() {
        let mut cache = QueryCache::new();
        let response = create_test_response();

        cache.insert("test_key".to_string(), response.clone());

        let cached = cache.get("test_key");
        assert!(cached.is_some());
        assert_eq!(cached.unwrap().sql, response.sql);
    }

    #[test]
    fn test_cache_miss() {
        let mut cache = QueryCache::new();

        let result = cache.get("nonexistent_key");
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_expiration() {
        let mut cache = QueryCache::with_ttl(0); // Immediate expiration
        let response = create_test_response();

        cache.insert("test_key".to_string(), response);

        // Sleep briefly to ensure expiration
        std::thread::sleep(Duration::from_millis(10));

        let result = cache.get("test_key");
        assert!(result.is_none());
    }

    #[test]
    fn test_cache_stats() {
        let mut cache = QueryCache::new();
        let response = create_test_response();

        cache.insert("key1".to_string(), response.clone());

        cache.get("key1"); // Hit
        cache.get("key2"); // Miss

        let (hits, misses) = cache.stats();
        assert_eq!(hits, 1);
        assert_eq!(misses, 1);
        assert_eq!(cache.hit_rate(), 0.5);
    }

    #[test]
    fn test_cache_clear() {
        let mut cache = QueryCache::new();
        let response = create_test_response();

        cache.insert("key1".to_string(), response);
        assert_eq!(cache.size(), 1);

        cache.clear();
        assert_eq!(cache.size(), 0);
    }
}
