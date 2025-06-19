use crate::Error;
use crate::Result;
use dashmap::DashMap;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    pub rate_limits: RateLimitConfig,
    pub word_filter: WordFilterConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub describe_window: u64,
    pub describe_max_actions: u32,
    pub vote_window: u64,
    pub vote_max_actions: u32,
    pub default_window: u64,
    pub default_max_actions: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordFilterConfig {
    pub sensitive_words: Vec<String>,
    pub custom_words: Vec<String>,
    pub replacement: String,
}

pub struct Security {
    rate_limits: DashMap<Uuid, RateLimiter>,
    word_filter: WordFilter,
    _rng: ThreadRng,
    config: SecurityConfig,
}

struct RateLimiter {
    last_action: Instant,
    count: u32,
    window: Duration,
    max_actions: u32,
}

impl RateLimiter {
    fn new(window: Duration, max_actions: u32) -> Self {
        RateLimiter {
            last_action: Instant::now(),
            count: 0,
            window,
            max_actions,
        }
    }

    fn check(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_action) > self.window {
            self.count = 0;
            self.last_action = now;
        }

        if self.count >= self.max_actions {
            false
        } else {
            self.count += 1;
            true
        }
    }
}

pub struct WordFilter {
    sensitive_words: Vec<String>,
    custom_words: Vec<String>,
    replacement: String,
}

impl Security {
    pub fn new(config_path: Option<&Path>) -> Result<Self> {
        let config = if let Some(path) = config_path {
            let config_str = fs::read_to_string(path)
                .map_err(|e| Error::Game(format!("Failed to read config file: {}", e)))?;
            serde_json::from_str(&config_str)
                .map_err(|e| Error::Game(format!("Failed to parse config file: {}", e)))?
        } else {
            SecurityConfig::default()
        };

        Ok(Security {
            rate_limits: DashMap::new(),
            word_filter: WordFilter::new(&config.word_filter),
            _rng: rand::rng(),
            config,
        })
    }

    pub fn check_rate_limit(&self, player_id: Uuid, action_type: &str) -> Result<()> {
        let (window, max_actions) = match action_type {
            "describe" => (
                Duration::from_secs(self.config.rate_limits.describe_window),
                self.config.rate_limits.describe_max_actions,
            ),
            "vote" => (
                Duration::from_secs(self.config.rate_limits.vote_window),
                self.config.rate_limits.vote_max_actions,
            ),
            _ => (
                Duration::from_secs(self.config.rate_limits.default_window),
                self.config.rate_limits.default_max_actions,
            ),
        };

        if let Some(mut limiter) = self.rate_limits.get_mut(&player_id) {
            if !limiter.check() {
                return Err(crate::Error::Game("操作过于频繁".to_string()));
            }
        } else {
            self.rate_limits
                .insert(player_id, RateLimiter::new(window, max_actions));
        }
        Ok(())
    }

    pub fn validate_input(&self, text: &str, max_length: usize) -> Result<()> {
        // 检查长度
        if text.len() > max_length {
            return Err(crate::Error::Game(format!(
                "输入长度超过限制: {}",
                max_length
            )));
        }

        // 检查是否为空
        if text.trim().is_empty() {
            return Err(crate::Error::Game("输入不能为空".to_string()));
        }

        // 检查是否包含敏感词
        if self.word_filter.contains_sensitive_words(text) {
            return Err(crate::Error::Game("输入包含敏感词".to_string()));
        }

        Ok(())
    }

    pub fn filter_sensitive_words(&self, text: &str) -> String {
        self.word_filter.filter(text)
    }

    pub fn add_custom_word(&mut self, word: String) {
        self.word_filter.add_custom_word(word);
    }

    pub fn remove_custom_word(&mut self, word: &str) {
        self.word_filter.remove_custom_word(word);
    }

    pub fn get_custom_words(&self) -> Vec<String> {
        self.word_filter.get_custom_words()
    }
}

impl WordFilter {
    fn new(config: &WordFilterConfig) -> Self {
        WordFilter {
            sensitive_words: config.sensitive_words.clone(),
            custom_words: config.custom_words.clone(),
            replacement: config.replacement.clone(),
        }
    }

    fn filter(&self, text: &str) -> String {
        let mut result = text.to_string();

        // 过滤敏感词
        for word in &self.sensitive_words {
            result = result.replace(word, &self.replacement);
        }

        // 过滤自定义词
        for word in &self.custom_words {
            result = result.replace(word, &self.replacement);
        }

        result
    }

    fn contains_sensitive_words(&self, text: &str) -> bool {
        self.sensitive_words.iter().any(|word| text.contains(word))
            || self.custom_words.iter().any(|word| text.contains(word))
    }

    fn add_custom_word(&mut self, word: String) {
        if !self.custom_words.contains(&word) {
            self.custom_words.push(word);
        }
    }

    fn remove_custom_word(&mut self, word: &str) {
        self.custom_words.retain(|w| w != word);
    }

    fn get_custom_words(&self) -> Vec<String> {
        self.custom_words.clone()
    }
}

impl Default for SecurityConfig {
    fn default() -> Self {
        SecurityConfig {
            rate_limits: RateLimitConfig {
                describe_window: 30,
                describe_max_actions: 1,
                vote_window: 10,
                vote_max_actions: 1,
                default_window: 1,
                default_max_actions: 10,
            },
            word_filter: WordFilterConfig {
                sensitive_words: vec!["敏感词1".to_string(), "敏感词2".to_string()],
                custom_words: Vec::new(),
                replacement: "***".to_string(),
            },
        }
    }
}
