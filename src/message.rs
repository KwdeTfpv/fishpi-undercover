use crate::game::PlayerId;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MessageError {
    #[error("未知的消息类型: {0}")]
    UnknownMessageType(String),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameMessage {
    #[serde(rename = "type")]
    pub type_: String,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameStateType {
    Lobby,
    RoleAssignment,
    DescribePhase,
    VotePhase,
    ResultPhase,
    GameOver,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StateChange {
    PlayerAdded(PlayerSnapshot),
    PlayerRemoved(PlayerId),
    DescriptionAdded(PlayerId, String),
    VoteCast(PlayerId, PlayerId),
    StateTransition(GameStateType),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlayerSnapshot {
    pub id: PlayerId,
    pub name: String,
    pub is_alive: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ErrorCode {
    RoomFull,
    GameStarted,
    InvalidState,
    InvalidAction,
    PlayerNotFound,
    NotYourTurn,
    AlreadyVoted,
    InvalidVote,
    Timeout,
    InternalError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageBatch {
    pub messages: Vec<GameMessage>,
    pub timestamp: i64,
}

impl MessageBatch {
    pub fn new() -> Self {
        Self {
            messages: Vec::new(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    pub fn add_message(&mut self, message: GameMessage) {
        self.messages.push(message);
    }

    pub fn is_full(&self) -> bool {
        self.messages.len() >= 50
    }

    pub fn is_expired(&self) -> bool {
        (chrono::Utc::now().timestamp() - self.timestamp) >= 50
    }
}

#[derive(Debug)]
pub struct MessageQueue {
    batches: VecDeque<MessageBatch>,
    current_batch: Option<MessageBatch>,
}

impl MessageQueue {
    pub fn new() -> Self {
        Self {
            batches: VecDeque::new(),
            current_batch: None,
        }
    }

    pub fn enqueue(&mut self, message: GameMessage) {
        if let Some(batch) = &mut self.current_batch {
            if !batch.is_full() && !batch.is_expired() {
                batch.add_message(message);
                return;
            }
        }

        let mut new_batch = MessageBatch::new();
        new_batch.add_message(message);
        self.current_batch = Some(new_batch);
    }

    pub fn dequeue_batch(&mut self) -> Option<MessageBatch> {
        if let Some(batch) = self.current_batch.take() {
            if batch.is_full() || batch.is_expired() {
                self.batches.push_back(batch);
            } else {
                self.current_batch = Some(batch);
            }
        }

        self.batches.pop_front()
    }
}

impl GameMessage {
    pub fn from_legacy_format(
        message_type: &str,
        data: serde_json::Value,
    ) -> Result<Self, MessageError> {
        match message_type {
            "join" => {
                let player_id = data["player_id"].as_str().unwrap_or_default().to_string();
                let name = data["name"].as_str().unwrap_or_default().to_string();
                Ok(GameMessage {
                    type_: "join".to_string(),
                    data: serde_json::json!({
                        "player_id": player_id,
                        "name": name,
                    }),
                })
            }
            "leave" => {
                let player_id = data["player_id"].as_str().unwrap_or_default().to_string();
                Ok(GameMessage {
                    type_: "leave".to_string(),
                    data: serde_json::json!({
                        "player_id": player_id,
                    }),
                })
            }
            "start" => Ok(GameMessage {
                type_: "start".to_string(),
                data: serde_json::Value::Null,
            }),
            "describe" => {
                let content = data["content"].as_str().unwrap_or_default().to_string();
                Ok(GameMessage {
                    type_: "describe".to_string(),
                    data: serde_json::json!({
                        "content": content,
                    }),
                })
            }
            "vote" => {
                let target_id = data["target_id"].as_str().unwrap_or_default().to_string();
                Ok(GameMessage {
                    type_: "vote".to_string(),
                    data: serde_json::json!({
                        "target_id": target_id,
                    }),
                })
            }
            _ => Err(MessageError::UnknownMessageType(message_type.to_string())),
        }
    }
}
