use crate::game::{GameState, Player, Role};
use crate::user::{User, UserSession};
use anyhow::Result;
use chrono::{DateTime, Utc};
use hex;
use redis::aio::ConnectionManager;
use redis::{AsyncCommands, Client};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tokio::sync::Mutex;
use uuid::Uuid;

#[derive(Clone)]
pub struct RedisStorage {
    manager: Arc<Mutex<ConnectionManager>>,
}

impl RedisStorage {
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        Ok(Self {
            manager: Arc::new(Mutex::new(manager)),
        })
    }

    // 房间相关操作
    pub async fn save_room_state(&self, room_id: &str, state: &impl Serialize) -> Result<()> {
        let state_json = serde_json::to_string(state)?;
        let mut conn = self.manager.lock().await;
        conn.hset::<_, _, _, ()>(format!("room:{}", room_id), "state", state_json)
            .await?;
        Ok(())
    }

    pub async fn load_room_state<T: for<'de> Deserialize<'de>>(
        &self,
        room_id: &str,
    ) -> Result<Option<T>> {
        let mut conn = self.manager.lock().await;
        let state_json: Option<String> = conn.hget(format!("room:{}", room_id), "state").await?;

        match state_json {
            Some(json) => Ok(Some(serde_json::from_str(&json)?)),
            None => Ok(None),
        }
    }

    // 玩家相关操作
    pub async fn save_player_info(&self, player_id: &str, name: &str, room_id: &str) -> Result<()> {
        let mut conn = self.manager.lock().await;
        let key = format!("player:{}", player_id);
        conn.hset::<_, _, _, ()>(&key, "name", name).await?;
        conn.hset::<_, _, _, ()>(&key, "room_id", room_id).await?;
        conn.hset::<_, _, _, ()>(&key, "last_active", chrono::Utc::now().timestamp())
            .await?;
        Ok(())
    }

    pub async fn get_player_info(&self, player_id: &str) -> Result<Option<(String, String)>> {
        let mut conn = self.manager.lock().await;
        let key = format!("player:{}", player_id);
        let name: Option<String> = conn.hget(&key, "name").await?;
        let room_id: Option<String> = conn.hget(&key, "room_id").await?;

        match (name, room_id) {
            (Some(name), Some(room_id)) => Ok(Some((name, room_id))),
            _ => Ok(None),
        }
    }

    // 投票相关操作
    pub async fn save_vote(&self, room_id: &str, voter_id: &str, target_id: &str) -> Result<()> {
        let mut conn = self.manager.lock().await;
        conn.hset::<_, _, _, ()>(format!("votes:{}", room_id), voter_id, target_id)
            .await?;
        Ok(())
    }

    pub async fn get_votes(&self, room_id: &str) -> Result<Vec<(String, String)>> {
        let mut conn = self.manager.lock().await;
        let votes: Vec<(String, String)> = conn.hgetall(format!("votes:{}", room_id)).await?;
        Ok(votes)
    }

    // 词库相关操作
    pub async fn add_word(&self, word: &str) -> Result<()> {
        let mut conn = self.manager.lock().await;
        conn.sadd::<_, _, ()>("word_bank", word).await?;
        Ok(())
    }

    pub async fn get_random_word(&self) -> Result<Option<String>> {
        let mut conn = self.manager.lock().await;
        let word: Option<String> = conn.srandmember("word_bank").await?;
        Ok(word)
    }

    // 游戏历史记录
    pub async fn save_game_history(
        &self,
        room_id: &str,
        game_state: &impl Serialize,
    ) -> Result<()> {
        let state_json = serde_json::to_string(game_state)?;
        let mut conn = self.manager.lock().await;
        conn.rpush::<_, _, ()>(format!("game_history:{}", room_id), state_json)
            .await?;
        Ok(())
    }

    pub async fn get_game_history<T: for<'de> Deserialize<'de>>(
        &self,
        room_id: &str,
    ) -> Result<Vec<T>> {
        let mut conn = self.manager.lock().await;
        let history: Vec<String> = conn
            .lrange(format!("game_history:{}", room_id), 0, -1)
            .await?;

        let mut states = Vec::new();
        for json in history {
            states.push(serde_json::from_str(&json)?);
        }
        Ok(states)
    }

    // 连接状态管理
    pub async fn update_connection_status(&self, player_id: &str, status: &str) -> Result<()> {
        let mut conn = self.manager.lock().await;
        let key = format!("connection:{}", player_id);
        conn.hset::<_, _, _, ()>(&key, "status", status).await?;
        conn.hset::<_, _, _, ()>(&key, "last_seen", chrono::Utc::now().timestamp())
            .await?;
        Ok(())
    }

    pub async fn get_connection_status(&self, player_id: &str) -> Result<Option<String>> {
        let mut conn = self.manager.lock().await;
        let status: Option<String> = conn
            .hget(format!("connection:{}", player_id), "status")
            .await?;
        Ok(status)
    }
}

pub struct Storage {
    manager: Arc<Mutex<ConnectionManager>>,
}

impl Storage {
    pub async fn new(redis_url: &str) -> Result<Self> {
        let client = Client::open(redis_url)?;
        let manager = ConnectionManager::new(client).await?;
        Ok(Storage {
            manager: Arc::new(Mutex::new(manager)),
        })
    }

    /// 保存房间状态
    pub async fn save_room_state(&self, room_id: String, state: &GameState) -> Result<()> {
        let key = format!("room:{}:state", room_id);
        let value =
            serde_json::to_string(state).map_err(|e| crate::Error::Storage(e.to_string()))?;

        let mut conn = self.manager.lock().await;
        conn.set_ex::<_, _, ()>(&key, &value, 3600)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;

        Ok(())
    }

    /// 加载房间状态
    pub async fn load_room_state(&self, room_id: String) -> Result<Option<GameState>> {
        let key = format!("room:{}:state", room_id);
        let mut conn = self.manager.lock().await;
        let value: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;

        match value {
            Some(json) => {
                let state: GameState = serde_json::from_str(&json)
                    .map_err(|e| crate::Error::Storage(e.to_string()))?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// 保存游戏结果
    pub async fn save_game_result(
        &self,
        room_id: String,
        winner: Role,
        players: &[Player],
    ) -> Result<()> {
        let key = format!("game:{}:result", room_id);
        let result = GameResult {
            room_id,
            winner,
            players: players.to_vec(),
            timestamp: Utc::now(),
        };

        let value =
            serde_json::to_string(&result).map_err(|e| crate::Error::Storage(e.to_string()))?;

        let mut conn = self.manager.lock().await;
        conn.set_ex::<_, _, ()>(&key, &value, 86400)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;

        Ok(())
    }

    pub async fn get_game_history(&self, limit: usize) -> Result<Vec<GameResult>> {
        let mut conn = self.manager.lock().await;
        let history_key = "game_history";
        let keys: Vec<String> = conn.lrange(history_key, 0, limit as isize - 1).await?;

        let mut results = Vec::new();
        for key in keys {
            if let Some(data) = conn.get::<_, Option<String>>(&key).await? {
                if let Ok(result) = serde_json::from_str::<GameResult>(&data) {
                    results.push(result);
                }
            }
        }

        Ok(results)
    }

    /// 获取玩家统计信息
    pub async fn get_player_stats(&self, player_id: Uuid) -> Result<PlayerStats> {
        let mut conn = self.manager.lock().await;
        let key = format!("player_stats:{}", player_id);

        let stats: Option<String> = conn.get(&key).await?;
        match stats {
            Some(data) => {
                let stats = serde_json::from_str(&data)?;
                Ok(stats)
            }
            None => Ok(PlayerStats::default()),
        }
    }

    /// 检查玩家是否已在其他房间，返回当前房间ID
    pub async fn get_player_current_room(&self, player_id: &str) -> Result<Option<String>> {
        let mut conn = self.manager.lock().await;
        let key = format!("player:{}", player_id);
        let stored_room_id: Option<String> = conn.hget(&key, "room_id").await?;
        Ok(stored_room_id)
    }

    /// 检查玩家是否已在其他房间（保持向后兼容）
    pub async fn is_player_in_other_room(&self, player_id: &str, current_room_id: &str) -> Result<bool> {
        if let Some(room_id) = self.get_player_current_room(player_id).await? {
            Ok(room_id != current_room_id)
        } else {
            Ok(false) // 玩家没有房间信息，说明不在任何房间
        }
    }

    /// 清理玩家的房间信息（当玩家离开房间时调用）
    pub async fn clear_player_room_info(&self, player_id: &str) -> Result<()> {
        let mut conn = self.manager.lock().await;
        let key = format!("player:{}", player_id);
        conn.del::<_, ()>(&key).await?;
        Ok(())
    }

    /// 保存玩家房间信息
    pub async fn save_player_room_info(&self, player_id: &str, name: &str, room_id: &str) -> Result<()> {
        let mut conn = self.manager.lock().await;
        let key = format!("player:{}", player_id);
        conn.hset::<_, _, _, ()>(&key, "name", name).await?;
        conn.hset::<_, _, _, ()>(&key, "room_id", room_id).await?;
        conn.hset::<_, _, _, ()>(&key, "last_active", chrono::Utc::now().timestamp()).await?;
        Ok(())
    }

    pub async fn update_player_stats(&self, player_id: Uuid, stats: &PlayerStats) -> Result<()> {
        let mut conn = self.manager.lock().await;
        let key = format!("player_stats:{}", player_id);
        let data = serde_json::to_string(stats)?;
        conn.set::<_, _, ()>(&key, data).await?;
        Ok(())
    }

    pub async fn save_checkpoint(&self, room_id: Uuid, state: &GameState) -> Result<()> {
        let mut conn = self.manager.lock().await;
        let key = format!("checkpoint:{}", room_id);
        let data = serde_json::to_string(state)?;
        conn.set::<_, _, ()>(&key, data).await?;
        Ok(())
    }

    pub async fn load_checkpoint(&self, room_id: Uuid) -> Result<Option<GameState>> {
        let mut conn = self.manager.lock().await;
        let key = format!("checkpoint:{}", room_id);
        let data: Option<String> = conn.get(&key).await?;

        match data {
            Some(data) => {
                let state = serde_json::from_str(&data)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    /// 保存用户会话
    pub async fn save_session(&self, session: &UserSession) -> Result<()> {
        let key = format!("session:{}", session.session_id);
        let session_json = serde_json::to_string(session)
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        let mut conn = self.manager.lock().await;
        // 设置过期时间（秒）
        let ttl = (session.expires_at - Utc::now()).num_seconds().max(0) as u64;
        conn.set_ex::<_, _, ()>(&key, &session_json, ttl)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        Ok(())
    }

    /// 获取用户会话
    pub async fn get_session(&self, session_id: &Uuid) -> Result<Option<UserSession>> {
        let key = format!("session:{}", session_id);
        let mut conn = self.manager.lock().await;
        
        let session_json: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        match session_json {
            Some(json) => {
                let session: UserSession = serde_json::from_str(&json)
                    .map_err(|e| crate::Error::Storage(e.to_string()))?;
                Ok(Some(session))
            }
            None => Ok(None),
        }
    }

    /// 删除用户会话
    pub async fn delete_session(&self, session_id: &Uuid) -> Result<()> {
        let key = format!("session:{}", session_id);
        let mut conn = self.manager.lock().await;
        
        conn.del::<_, ()>(&key)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        Ok(())
    }

    /// 更新会话过期时间
    pub async fn extend_session(&self, session_id: &Uuid, new_expires_at: DateTime<Utc>) -> Result<()> {
        let key = format!("session:{}", session_id);
        let mut conn = self.manager.lock().await;
        
        // 先获取现有会话
        let session_json: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        if let Some(json) = session_json {
            let mut session: UserSession = serde_json::from_str(&json)
                .map_err(|e| crate::Error::Storage(e.to_string()))?;
            
            // 更新过期时间
            session.expires_at = new_expires_at;
            let updated_json = serde_json::to_string(&session)
                .map_err(|e| crate::Error::Storage(e.to_string()))?;
            
            // 重新保存，设置新的过期时间
            let ttl = (new_expires_at - Utc::now()).num_seconds().max(0) as u64;
            conn.set_ex::<_, _, ()>(&key, &updated_json, ttl)
                .await
                .map_err(|e| crate::Error::Storage(e.to_string()))?;
        }
        
        Ok(())
    }

    /// 保存用户信息
    pub async fn save_user(&self, user: &User) -> Result<()> {
        let key = format!("user:{}", user.id);
        let user_json = serde_json::to_string(user)
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        let mut conn = self.manager.lock().await;
        // 用户信息不过期，永久保存
        conn.set::<_, _, ()>(&key, &user_json)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        Ok(())
    }

    /// 获取用户信息
    pub async fn get_user(&self, user_id: &str) -> Result<Option<User>> {
        let key = format!("user:{}", user_id);
        let mut conn = self.manager.lock().await;
        
        let user_json: Option<String> = conn
            .get(&key)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        match user_json {
            Some(json) => {
                let user: User = serde_json::from_str(&json)
                    .map_err(|e| crate::Error::Storage(e.to_string()))?;
                Ok(Some(user))
            }
            None => Ok(None),
        }
    }

    /// 删除用户信息
    pub async fn delete_user(&self, user_id: &str) -> Result<()> {
        let key = format!("user:{}", user_id);
        let mut conn = self.manager.lock().await;
        
        conn.del::<_, ()>(&key)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        
        Ok(())
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GameResult {
    pub room_id: String,
    pub winner: Role,
    pub players: Vec<Player>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct PlayerStats {
    pub games_played: u32,
    pub games_won: u32,
    pub games_as_undercover: u32,
    pub games_won_as_undercover: u32,
    pub games_as_civilian: u32,
    pub games_won_as_civilian: u32,
    pub total_votes_received: u32,
    pub total_votes_cast: u32,
    pub correct_votes: u32,
    pub last_played: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub state: GameState,
    pub version: u64,
    pub timestamp: DateTime<Utc>,
    pub is_consistent: bool,
}

impl Storage {
    pub async fn create_checkpoint(
        &self,
        room_id: Uuid,
        state: &GameState,
        version: u64,
    ) -> Result<()> {
        let checkpoint = Checkpoint {
            state: state.clone(),
            version,
            timestamp: Utc::now(),
            is_consistent: true,
        };

        let mut conn = self.manager.lock().await;
        let key = format!("checkpoint:{}", room_id);
        let data = serde_json::to_string(&checkpoint)?;

        // 使用事务确保原子性
        let mut pipe = redis::pipe();
        pipe.atomic().set(&key, &data).expire(&key, 24 * 60 * 60); // 24小时过期

        let _: () = pipe.query_async(&mut *conn).await?;

        Ok(())
    }

    pub async fn load_latest_checkpoint(&self, room_id: Uuid) -> Result<Option<Checkpoint>> {
        let mut conn = self.manager.lock().await;
        let key = format!("checkpoint:{}", room_id);
        let data: Option<String> = conn.get(&key).await?;

        match data {
            Some(data) => {
                let checkpoint = serde_json::from_str(&data)?;
                Ok(Some(checkpoint))
            }
            None => Ok(None),
        }
    }

    pub async fn verify_state_consistency(&self, room_id: Uuid, state: &GameState) -> Result<bool> {
        let mut conn = self.manager.lock().await;
        let key = format!("state_verification:{}", room_id);

        // 计算状态哈希
        let state_hash = self.calculate_state_hash(state).await?;

        // 获取之前的状态哈希
        let prev_hash: Option<String> = conn.get(&key).await?;

        // 更新状态哈希
        conn.set::<_, _, ()>(&key, &state_hash).await?;

        // 验证一致性
        Ok(prev_hash.map_or(true, |h| h == state_hash))
    }

    async fn calculate_state_hash(&self, state: &GameState) -> Result<String> {
        let state_json = serde_json::to_string(state)?;
        let hash = Sha256::digest(state_json.as_bytes());
        Ok(hex::encode(hash))
    }

    pub async fn recover_from_crash(&self, room_id: Uuid) -> Result<Option<GameState>> {
        // 1. 尝试加载最新的检查点
        if let Some(checkpoint) = self.load_latest_checkpoint(room_id).await? {
            // 2. 验证检查点的一致性
            if checkpoint.is_consistent {
                return Ok(Some(checkpoint.state));
            }
        }

        // 3. 如果没有有效的检查点，尝试从游戏历史恢复
        let mut conn = self.manager.lock().await;
        let history_key = format!("game_history:{}", room_id);
        let latest_state: Option<String> = conn.lindex(&history_key, 0).await?;

        match latest_state {
            Some(state_json) => {
                let state = serde_json::from_str(&state_json)?;
                Ok(Some(state))
            }
            None => Ok(None),
        }
    }

    pub async fn cleanup_old_data(&self) -> Result<()> {
        let mut conn = self.manager.lock().await;

        // 清理过期的检查点
        let pattern = "checkpoint:*";
        let keys: Vec<String> = conn.keys(pattern).await?;
        for key in keys {
            let _: i32 = conn.del(&key).await?;
        }

        // 清理过期的游戏历史
        let pattern = "game_history:*";
        let keys: Vec<String> = conn.keys(pattern).await?;
        for key in keys {
            let _: bool = conn.ltrim(&key, 0, 999).await?; // 只保留最近1000条记录
        }

        Ok(())
    }
}
