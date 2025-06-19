pub mod config;
pub mod game;
pub mod message;
pub mod network;
pub mod room;
pub mod security;
pub mod storage;
pub mod user;
pub mod word_bank;

pub use config::Config;
pub use game::GameState;
pub use message::GameMessage;
pub use network::WebSocketServer;
pub use room::Room;
pub use storage::*;
pub use user::{User, UserManager, UserSession};
pub use word_bank::WordBank;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("网络错误: {0}")]
    Network(#[from] anyhow::Error),
    #[error("游戏错误: {0}")]
    Game(String),
    #[error("房间错误: {0}")]
    Room(String),
    #[error("存储错误: {0}")]
    Storage(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("认证错误: {0}")]
    Auth(String),
}

pub type Result<T> = std::result::Result<T, Error>;
