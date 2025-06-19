use crate::Result;
use anyhow::Context;
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordPair {
    pub civilian_word: String,
    pub undercover_word: String,
    pub similarity: f32,
    pub difficulty: Difficulty,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum Difficulty {
    #[serde(rename = "easy")]
    Easy,
    #[serde(rename = "medium")]
    Medium,
    #[serde(rename = "hard")]
    Hard,
}

impl Difficulty {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "easy" => Difficulty::Easy,
            "medium" => Difficulty::Medium,
            "hard" => Difficulty::Hard,
            _ => Difficulty::Medium,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordCategory {
    pub name: String,
    pub words: Vec<WordPair>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WordBankData {
    pub categories: HashMap<String, Vec<WordPair>>,
}

#[derive(Debug, Clone)]
pub struct WordBank {
    categories: HashMap<String, Vec<WordPair>>,
    all_words: Vec<WordPair>,
    config: crate::config::WordBankConfig,
}

impl WordBank {
    pub fn new() -> Self {
        let config = crate::config::Config::get().word_bank.clone();
        let file_path = config.file_path.clone();
        let mut word_bank = WordBank {
            categories: HashMap::new(),
            all_words: Vec::new(),
            config,
        };

        // 尝试从文件加载，如果失败则使用默认词库
        if let Err(e) = word_bank.load_from_file(&file_path) {
            eprintln!("无法加载词库文件: {}, 使用默认词库", e);
            word_bank.load_default_words();
        }

        word_bank
    }

    /// 从文件加载词库
    pub fn load_from_file(&mut self, path: &str) -> Result<()> {
        let content =
            std::fs::read_to_string(path).with_context(|| format!("无法读取词库文件: {}", path))?;

        let data: WordBankData =
            serde_json::from_str(&content).with_context(|| "无法解析词库文件格式")?;

        self.categories = data.categories;
        self.update_all_words();

        Ok(())
    }

    /// 保存词库到文件
    pub fn save_to_file(&self, path: &str) -> Result<()> {
        let data = WordBankData {
            categories: self.categories.clone(),
        };

        let content = serde_json::to_string_pretty(&data).with_context(|| "无法序列化词库")?;

        std::fs::write(path, content).with_context(|| format!("无法写入词库文件: {}", path))?;

        Ok(())
    }

    /// 加载默认词库
    fn load_default_words(&mut self) {
        self.categories.clear();

        // 食物类
        self.categories.insert(
            "食物".to_string(),
            vec![
                WordPair {
                    civilian_word: "苹果".to_string(),
                    undercover_word: "梨".to_string(),
                    similarity: 0.8,
                    difficulty: Difficulty::Easy,
                },
                WordPair {
                    civilian_word: "香蕉".to_string(),
                    undercover_word: "橙子".to_string(),
                    similarity: 0.7,
                    difficulty: Difficulty::Easy,
                },
            ],
        );

        // 电子产品类
        self.categories.insert(
            "电子产品".to_string(),
            vec![
                WordPair {
                    civilian_word: "手机".to_string(),
                    undercover_word: "平板".to_string(),
                    similarity: 0.7,
                    difficulty: Difficulty::Easy,
                },
                WordPair {
                    civilian_word: "电脑".to_string(),
                    undercover_word: "笔记本".to_string(),
                    similarity: 0.8,
                    difficulty: Difficulty::Easy,
                },
            ],
        );

        self.update_all_words();
    }

    /// 更新所有词语列表
    fn update_all_words(&mut self) {
        self.all_words.clear();
        for words in self.categories.values() {
            self.all_words.extend(words.clone());
        }
    }

    /// 获取随机词对
    pub fn get_random_word_pair(&self) -> Option<&WordPair> {
        let mut rng = rand::rng();
        self.all_words.choose(&mut rng)
    }

    /// 根据相似度获取词对
    pub fn get_word_pair_by_similarity(&self, min_similarity: f32) -> Option<&WordPair> {
        let mut rng = rand::rng();
        self.all_words
            .iter()
            .filter(|pair| pair.similarity >= min_similarity)
            .collect::<Vec<_>>()
            .choose(&mut rng)
            .copied()
    }

    /// 根据难度获取词对
    pub fn get_word_pair_by_difficulty(&self, difficulty: Difficulty) -> Option<&WordPair> {
        let mut rng = rand::rng();
        self.all_words
            .iter()
            .filter(|pair| pair.difficulty == difficulty)
            .collect::<Vec<_>>()
            .choose(&mut rng)
            .copied()
    }

    /// 从指定分类获取词对
    pub fn get_word_pair_from_category(&self, category: &str) -> Option<&WordPair> {
        if let Some(words) = self.categories.get(category) {
            let mut rng = rand::rng();
            words.choose(&mut rng)
        } else {
            None
        }
    }

    /// 获取所有分类
    pub fn get_categories(&self) -> Vec<&String> {
        self.categories.keys().collect()
    }

    /// 获取分类中的词对数量
    pub fn get_category_word_count(&self, category: &str) -> usize {
        self.categories
            .get(category)
            .map(|words| words.len())
            .unwrap_or(0)
    }

    /// 添加词对到指定分类
    pub fn add_word_pair(&mut self, category: &str, word_pair: WordPair) {
        self.categories
            .entry(category.to_string())
            .or_insert_with(Vec::new)
            .push(word_pair);
        self.update_all_words();
    }

    /// 添加新分类
    pub fn add_category(&mut self, category: &str) {
        if !self.categories.contains_key(category) {
            self.categories.insert(category.to_string(), Vec::new());
        }
    }

    /// 删除分类
    pub fn remove_category(&mut self, category: &str) {
        self.categories.remove(category);
        self.update_all_words();
    }

    /// 获取词库统计信息
    pub fn get_stats(&self) -> WordBankStats {
        let total_words = self.all_words.len();
        let total_categories = self.categories.len();

        let mut difficulty_stats = HashMap::new();
        for word in &self.all_words {
            *difficulty_stats.entry(word.difficulty.clone()).or_insert(0) += 1;
        }

        let mut category_stats = HashMap::new();
        for (category, words) in &self.categories {
            category_stats.insert(category.clone(), words.len());
        }

        WordBankStats {
            total_words,
            total_categories,
            difficulty_stats,
            category_stats,
        }
    }

    /// 验证词库完整性
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        for (category, words) in &self.categories {
            if words.is_empty() {
                errors.push(format!("分类 '{}' 没有词语", category));
            }

            for (i, word) in words.iter().enumerate() {
                if word.civilian_word.is_empty() || word.undercover_word.is_empty() {
                    errors.push(format!("分类 '{}' 第{}个词对包含空词语", category, i + 1));
                }

                if word.similarity < 0.0 || word.similarity > 1.0 {
                    errors.push(format!(
                        "分类 '{}' 第{}个词对相似度超出范围",
                        category,
                        i + 1
                    ));
                }
            }
        }

        errors
    }

    /// 获取分类中的词对
    pub fn get_category_words(&self, category: &str) -> Option<&Vec<WordPair>> {
        self.categories.get(category)
    }

    /// 获取配置
    pub fn get_config(&self) -> &crate::config::WordBankConfig {
        &self.config
    }
}

#[derive(Debug, Clone)]
pub struct WordBankStats {
    pub total_words: usize,
    pub total_categories: usize,
    pub difficulty_stats: HashMap<Difficulty, usize>,
    pub category_stats: HashMap<String, usize>,
}

impl Default for WordBank {
    fn default() -> Self {
        Self::new()
    }
}
