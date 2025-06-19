use anyhow::Result;
use once_cell::sync::OnceCell;
use serde::Deserialize;
use std::net::SocketAddr;
use std::time::Duration;

static CONFIG: OnceCell<Config> = OnceCell::new();

#[derive(Debug, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub websocket: WebSocketConfig,
    pub game: GameConfig,
    pub redis: RedisConfig,
    // pub log: LogConfig,
    pub security: SecurityConfig,
    pub auth: AuthConfig,
    pub word_bank: WordBankConfig,
}

#[derive(Debug, Deserialize)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub workers: usize,
}

#[derive(Debug, Deserialize)]
pub struct WebSocketConfig {
    pub path: String,
    pub ping_interval: u64,
    pub ping_timeout: u64,
}

#[derive(Debug, Deserialize)]
pub struct GameConfig {
    pub min_players: usize,
    pub max_players: usize,
    pub describe_time_limit: u64,
    pub vote_time_limit: u64,
    pub round_delay: u64,
}

#[derive(Debug, Deserialize)]
pub struct RedisConfig {
    pub url: String,
    pub pool_size: u32,
}

// #[derive(Debug, Deserialize)]
// pub struct LogConfig {
//     pub level: String,
//     pub file: String,
// }

#[derive(Debug, Deserialize)]
pub struct SecurityConfig {
    pub rate_limit_requests: u32,
    pub rate_limit_duration: u64,
}

#[derive(Debug, Deserialize)]
pub struct AuthConfig {
    pub domain: String,
}

#[derive(Debug, Deserialize, Clone)]
pub struct WordBankConfig {
    pub file_path: String,
    pub min_similarity: f32,
    pub max_words_per_category: usize,
    pub enable_categories: bool,
}

impl Config {
    pub fn load() -> Result<Self> {
        let config = config::Config::builder()
            .add_source(config::File::with_name("config"))
            // .add_source(config::Environment::with_prefix("GAME"))
            .build()?;

        Ok(config.try_deserialize::<Config>()?)
    }

    /// 初始化全局配置
    pub fn init() -> Result<()> {
        let config = Self::load()?;
        CONFIG
            .set(config)
            .map_err(|_| anyhow::anyhow!("配置已经初始化"))?;
        Ok(())
    }

    /// 获取全局配置实例
    pub fn get() -> &'static Config {
        CONFIG.get().expect("配置未初始化，请先调用 Config::init()")
    }

    pub fn server_addr(&self) -> SocketAddr {
        format!("{}:{}", self.server.host, self.server.port)
            .parse()
            .expect("Invalid server address")
    }

    pub fn ping_interval(&self) -> Duration {
        Duration::from_secs(self.websocket.ping_interval)
    }

    pub fn ping_timeout(&self) -> Duration {
        Duration::from_secs(self.websocket.ping_timeout)
    }

    pub fn describe_time_limit(&self) -> Duration {
        Duration::from_secs(self.game.describe_time_limit)
    }

    pub fn vote_time_limit(&self) -> Duration {
        Duration::from_secs(self.game.vote_time_limit)
    }

    pub fn round_delay(&self) -> Duration {
        Duration::from_secs(self.game.round_delay)
    }
}
