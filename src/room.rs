use crate::Result;
use crate::config;
use crate::game::{GameEvent, GameState, Player, PlayerId, TimeoutResult};
use crate::message::GameMessage;
use crate::storage::Storage;
use crate::word_bank::WordBank;
use chrono::Utc;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, RwLock, mpsc};
use tracing::{debug, error};

/// 游戏房间，负责管理房间内的玩家和游戏状态
pub struct Room {
    id: String,
    state: Arc<RwLock<GameState>>,
    players: Arc<DashMap<PlayerId, Player>>,
    word_bank: Arc<WordBank>,
    player_channels: Arc<DashMap<PlayerId, mpsc::Sender<GameMessage>>>,
    player_order: Arc<Mutex<Vec<PlayerId>>>,
    storage: Arc<Storage>,
}

impl Room {
    /// 创建新房间
    pub fn new(
        id: String,
        min_players: usize,
        max_players: usize,
        word_bank: Arc<WordBank>,
        storage: Arc<Storage>,
    ) -> Self {
        let state = Arc::new(RwLock::new(GameState::new(min_players, max_players)));

        Room {
            id,
            state,
            players: Arc::new(DashMap::new()),
            word_bank,
            player_channels: Arc::new(DashMap::new()),
            player_order: Arc::new(Mutex::new(Vec::new())),
            storage,
        }
    }

    /// 添加玩家到房间
    pub async fn add_player(
        &self,
        player: &Player,
        channel: mpsc::Sender<GameMessage>,
    ) -> Result<()> {
        // 检查玩家是否已经存在
        if self.players.contains_key(&player.id) {
            debug!("玩家 {} 已经存在于房间中，更新连接通道", player.name);
            // 只更新连接通道，不重新添加玩家
            if self.player_channels.contains_key(&player.id) {
                self.player_channels.remove(&player.id);
            }
            self.player_channels.insert(player.id.clone(), channel);
            return Ok(());
        }

        let mut state = self.state.write().await;
        let event = state
            .add_player(player.clone())
            .map_err(|e| crate::Error::Game(e))?;

        self.players.insert(player.id.clone(), player.clone());

        if self.player_channels.contains_key(&player.id) {
            self.player_channels.remove(&player.id);
        }

        self.player_channels.insert(player.id.clone(), channel);
        self.player_order.lock().await.push(player.id.clone());

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 从房间移除玩家
    pub async fn remove_player(&self, player_id: PlayerId) -> Result<()> {
        let mut state = self.state.write().await;
        let event = state
            .remove_player(player_id.clone())
            .map_err(|e| crate::Error::Game(e))?;

        self.players.remove(&player_id);
        self.player_channels.remove(&player_id);
        self.player_order.lock().await.retain(|id| id != &player_id);

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 广播消息给房间内所有玩家
    pub async fn broadcast(&self, message: GameMessage) {
        let message = message.clone();

        for entry in self.player_channels.iter() {
            if let Err(e) = entry.value().send(message.clone()).await {
                error!("广播消息失败: {}", e);
                self.player_channels.remove(entry.key());
            }
        }
    }

    /// 处理房间消息
    pub(crate) async fn handle_message(
        &self,
        message: GameMessage,
        player_tx: Option<tokio::sync::mpsc::Sender<GameMessage>>,
    ) -> Result<()> {
        let message_type = message.type_.clone();

        match message_type.as_str() {
            "join" => {
                if let Some(tx) = player_tx {
                    self.handle_join(message, tx).await?;
                } else {
                    return Err(crate::Error::Game("join消息需要player_tx".to_string()));
                }
            }
            "ready" => {
                self.handle_ready(message).await?;
            }
            "describe" => {
                self.handle_describe(message).await?;
            }
            "vote" => {
                self.handle_vote(message).await?;
            }
            "chat" => {
                self.handle_chat(message).await?;
            }
            "leave" => {
                self.handle_leave(message).await?;
            }
            _ => return Err(crate::Error::Game("未知的消息类型".to_string())),
        }
        Ok(())
    }

    /// 处理玩家加入消息
    async fn handle_join(
        &self,
        message: GameMessage,
        player_tx: tokio::sync::mpsc::Sender<GameMessage>,
    ) -> Result<Player> {
        let message_data = message.data.clone();
        let player_name = message_data["player_name"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的玩家名称".to_string()))?;
        let player_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的玩家ID".to_string()))?
            .to_string();

        // 检查玩家是否已经存在
        let is_reconnect = self.players.contains_key(&player_id);

        if is_reconnect {
            // 重新连接：使用原有的玩家数据，只更新连接通道
            debug!("玩家 {} 重新连接", player_name);
            let existing_player = self.players.get(&player_id).unwrap().clone();

            if self.player_channels.contains_key(&player_id) {
                self.player_channels.remove(&player_id);
            }
            self.player_channels.insert(player_id.clone(), player_tx);

            // 发送重新连接通知
            self.broadcast(GameMessage {
                type_: "notification".to_string(),
                data: serde_json::json!({
                    "message": format!("玩家 {} 重新连接", player_name)
                }),
            })
            .await;
            // 发送当前状态更新
            self.broadcast_state_update().await;

            Ok(existing_player)
        } else {
            // 新玩家加入：创建新的Player对象
            let player = Player {
                id: player_id.clone(),
                name: player_name.to_string(),
                role: None,
                word: None,
                is_alive: true,
                last_action: Utc::now(),
            };

            self.add_player(&player, player_tx).await?;
            Ok(player)
        }
    }

    /// 处理玩家准备消息
    async fn handle_ready(&self, message: GameMessage) -> Result<()> {
        let message_data = message.data.clone();
        let player_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的玩家ID".to_string()))?
            .to_string();

        let mut state = self.state.write().await;
        let event = state
            .player_ready(player_id)
            .map_err(|e| crate::Error::Game(e))?;

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 处理玩家描述消息
    async fn handle_describe(&self, message: GameMessage) -> Result<()> {
        let message_data = message.data.clone();
        let player_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的玩家ID".to_string()))?
            .to_string();
        let content = message_data["content"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的描述内容".to_string()))?;

        let mut state = self.state.write().await;
        let event = state
            .add_description(player_id, content.to_string())
            .map_err(|e| crate::Error::Game(e))?;

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 处理玩家投票消息
    async fn handle_vote(&self, message: GameMessage) -> Result<()> {
        let message_data = message.data.clone();
        let voter_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的投票者ID".to_string()))?
            .to_string();
        let target_id = message_data["target_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的目标ID".to_string()))?
            .to_string();

        let mut state = self.state.write().await;
        let event = state
            .add_vote(voter_id, target_id)
            .map_err(|e| crate::Error::Game(e))?;

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 处理玩家聊天消息
    async fn handle_chat(&self, message: GameMessage) -> Result<()> {
        let message_data = message.data.clone();
        let player_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的玩家ID".to_string()))?
            .to_string();
        let content = message_data["content"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的聊天内容".to_string()))?;

        let mut state = self.state.write().await;
        let event = state
            .add_chat_message(player_id, content.to_string())
            .map_err(|e| crate::Error::Game(e))?;

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 处理玩家离开消息
    async fn handle_leave(&self, message: GameMessage) -> Result<()> {
        let message_data = message.data.clone();
        let player_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的玩家ID".to_string()))?
            .to_string();

        self.remove_player(player_id).await?;
        Ok(())
    }

    /// 获取房间内玩家数量
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// 保存房间状态到存储
    pub async fn save_state(&self) -> Result<()> {
        let state = self.state.read().await;
        self.storage
            .save_room_state(self.id.clone(), &*state)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        Ok(())
    }

    /// 从存储加载房间状态
    pub async fn load_state(&self) -> Result<()> {
        if let Some(state) = self
            .storage
            .load_room_state(self.id.clone())
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?
        {
            let mut current_state = self.state.write().await;
            *current_state = state;
        }
        Ok(())
    }

    /// 保存游戏结果
    pub async fn save_game_result(&self, winner: crate::game::Role) -> Result<()> {
        let state = self.state.read().await;
        let players = state.get_players();
        self.storage
            .save_game_result(self.id.clone(), winner, &players)
            .await
            .map_err(|e| crate::Error::Storage(e.to_string()))?;
        Ok(())
    }

    /// 检查游戏状态超时
    pub async fn check_timeout(&self) -> Result<()> {
        let state = self.state.read().await;
        let timeout_result = state.check_timeout();
        drop(state);

        match timeout_result {
            TimeoutResult::None => Ok(()),
            TimeoutResult::DescribeTimeout(_) => {
                let mut state = self.state.write().await;
                let event = state
                    .handle_describe_timeout()
                    .map_err(|e| crate::Error::Game(e))?;
                drop(state);
                Box::pin(self.handle_game_event(event)).await?;
                Ok(())
            }
            TimeoutResult::VoteTimeout => {
                let mut state = self.state.write().await;
                let event = state
                    .handle_vote_timeout()
                    .map_err(|e| crate::Error::Game(e))?;
                drop(state);
                Box::pin(self.handle_game_event(event)).await?;
                Ok(())
            }
            TimeoutResult::ResultTimeout => {
                let mut state = self.state.write().await;
                let event = state
                    .process_result_phase()
                    .map_err(|e| crate::Error::Game(e))?;
                drop(state);
                Box::pin(self.handle_game_event(event)).await?;
                Ok(())
            }
        }
    }

    /// 处理游戏事件
    async fn handle_game_event(&self, event: GameEvent) -> Result<()> {
        match event {
            GameEvent::PlayerJoined(player) => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 加入了游戏", player.name),
                        "total_players": self.players.len()
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::PlayerLeft(player_id) => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 离开了游戏", player_id)
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::PlayerReady(player_id, can_start) => {
                let player_name = self
                    .players
                    .get(&player_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "未知玩家".to_string());

                // 从GameState中获取准备玩家数量
                let state = self.state.read().await;
                let ready_count = match &*state {
                    GameState::Lobby { ready_players, .. } => ready_players.len(),
                    _ => 0,
                };
                drop(state);

                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 已准备", player_name),
                        "ready_count": ready_count,
                        "min_players": config::Config::get().game.min_players
                    }),
                })
                .await;

                if can_start {
                    Box::pin(self.start_game()).await?;
                }
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::GameStarted(..) => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": "游戏开始，进入描述阶段"
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::DescriptionAdded(player_id, content) => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 完成了描述", player_id),
                        "content": content
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::NextPlayer(player_id) => {
                let player_name = self
                    .players
                    .get(&player_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "未知玩家".to_string());

                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("轮到玩家 {} 描述", player_name)
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::DescribePhaseComplete => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": "描述阶段结束，进入投票阶段"
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::VoteAdded(voter_id, target_id) => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 投票给了 {}", voter_id, target_id)
                    }),
                })
                .await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::ChatMessageAdded(chat_message) => {
                self.broadcast(GameMessage {
                    type_: "chat".to_string(),
                    data: serde_json::json!({
                        "player_id": chat_message.player_id.to_string(),
                        "player_name": chat_message.player_name,
                        "content": chat_message.content,
                        "timestamp": chat_message.timestamp.timestamp()
                    }),
                })
                .await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::VotePhaseComplete(votes) => {
                // 处理投票结果
                let state = self.state.read().await;
                let players = state.get_players();
                let eliminated = state.get_eliminated_player();

                // 统计投票
                let mut vote_count: HashMap<PlayerId, usize> = HashMap::new();
                let mut vote_notifications = Vec::new();

                for (voter_id, target_id) in &votes {
                    *vote_count.entry(target_id.clone()).or_insert(0) += 1;

                    let voter = players.iter().find(|p| p.id == *voter_id);
                    let target = players.iter().find(|p| p.id == *target_id);
                    if let (Some(v), Some(t)) = (voter, target) {
                        vote_notifications.push(format!("{} 投给了 {}", v.name, t.name));
                    }
                }

                // 检查是否是平票
                let result_message = if let Some(eliminated) = eliminated {
                    if eliminated == "tie" {
                        "投票平票，没有人被淘汰！".to_string()
                    } else {
                        let eliminated_player = players.iter().find(|p| p.id == eliminated);
                        if let Some(player) = eliminated_player {
                            format!("玩家 {} 被淘汰了！", player.name)
                        } else {
                            "有玩家被淘汰了！".to_string()
                        }
                    }
                } else {
                    "投票完成".to_string()
                };

                // 广播投票结果
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": result_message,
                        "vote_count": vote_count.iter().map(|(player_id, count)| {
                            let player = players.iter().find(|p| p.id == *player_id);
                            serde_json::json!({
                                "player_id": player_id,
                                "player_name": player.map(|p| p.name.clone()).unwrap_or_else(|| "未知玩家".to_string()),
                                "votes": count
                            })
                        }).collect::<Vec<_>>()
                    }),
                }).await;

                // 发送投票详情
                for vote_notification in vote_notifications {
                    self.broadcast(GameMessage {
                        type_: "notification".to_string(),
                        data: serde_json::json!({
                            "message": vote_notification
                        }),
                    })
                    .await;
                }

                self.broadcast_state_update().await;

                // 处理结果阶段
                drop(state);
                let mut state = self.state.write().await;
                let event = state
                    .process_result_phase()
                    .map_err(|e| crate::Error::Game(e))?;
                drop(state);

                // 递归处理结果事件
                Box::pin(self.handle_game_event(event)).await?;
            }
            GameEvent::PlayerEliminated(player_id) => {
                let player_name = self
                    .players
                    .get(&player_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "未知玩家".to_string());

                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 被淘汰了！", player_name)
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::VoteTied => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": "投票平票，没有人被淘汰！"
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::RoundComplete => {
                let state = self.state.read().await;
                if let Some(current_player_index) = state.get_current_player_index() {
                    let players = state.get_players();
                    let current_player = &players[current_player_index];

                    self.broadcast(GameMessage {
                        type_: "notification".to_string(),
                        data: serde_json::json!({
                            "message": format!("开始新一轮，轮到玩家 {} 描述", current_player.name)
                        }),
                    })
                    .await;
                }
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::GameOver(winner) => {
                // 保存游戏结果
                self.save_game_result(winner).await?;
                self.broadcast_game_over(winner).await;
            }
            GameEvent::GameReset => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": "开始游戏"
                    }),
                })
                .await;
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
        }
        Ok(())
    }

    /// 开始游戏
    async fn start_game(&self) -> Result<()> {
        let mut state = self.state.write().await;
        let player_order = self.player_order.lock().await.clone();
        let event = state
            .start_game(self.word_bank.clone(), &player_order)
            .map_err(|e| crate::Error::Game(e))?;
        drop(state);

        Box::pin(self.handle_game_event(event)).await?;
        Ok(())
    }

    /// 广播游戏结束消息
    async fn broadcast_game_over(&self, winner: crate::game::Role) {
        let state = self.state.read().await;
        let players = state.get_players();

        // 收集词语信息
        let mut civilian_word = None;
        let mut undercover_word = None;

        for player in &players {
            if let Some(role) = player.role {
                if let Some(word) = &player.word {
                    match role {
                        crate::game::Role::Civilian => {
                            if civilian_word.is_none() {
                                civilian_word = Some(word.clone());
                            }
                        }
                        crate::game::Role::Undercover => {
                            if undercover_word.is_none() {
                                undercover_word = Some(word.clone());
                            }
                        }
                    }
                }
            }
        }

        // 游戏结束时，为所有玩家发送完整的状态信息（包括所有玩家的角色和词语）
        for entry in self.player_channels.iter() {
            // let target_player_id = entry.key();
            let channel = entry.value();

            let state_data = serde_json::json!({
                "state": state.get_state_type(),
                "winner": winner.to_string(),
                "players": players.iter().map(|player| {
                    serde_json::json!({
                        "id": player.id.to_string(),
                        "name": player.name,
                        "is_alive": player.is_alive,
                        "role": player.role.map(|r| r.to_string()),
                        "word": player.word.clone()
                    })
                }).collect::<Vec<_>>(),
                "total_players": players.len(),
                "civilian_word": civilian_word.clone(),
                "undercover_word": undercover_word.clone()
            });

            let state_update = GameMessage {
                type_: "state_update".to_string(),
                data: state_data,
            };

            if let Err(e) = channel.send(state_update).await {
                error!("发送游戏结束状态更新失败: {}", e);
            }
        }

        self.broadcast(GameMessage {
            type_: "notification".to_string(),
            data: serde_json::json!({
                "message": format!("游戏结束，{}胜利！平民词语：{}，卧底词语：{}",
                    winner,
                    civilian_word.unwrap_or_else(|| "未知".to_string()),
                    undercover_word.unwrap_or_else(|| "未知".to_string()))
            }),
        })
        .await;
    }

    /// 广播状态更新
    async fn broadcast_state_update(&self) {
        let state = self.state.read().await;

        for entry in self.player_channels.iter() {
            let target_player_id = entry.key();
            let channel = entry.value();

            let mut state_data = serde_json::json!({
                "state": state.get_state_type(),
                "players": state.get_players().iter().map(|player| {
                    let mut player_data = serde_json::json!({
                        "id": player.id.to_string(),
                        "name": player.name,
                        "is_alive": player.is_alive,
                    });

                    // 在Lobby状态下，添加准备状态
                    if let GameState::Lobby { ready_players, .. } = &*state {
                        player_data["is_ready"] = serde_json::Value::Bool(ready_players.contains(&player.id));
                    }

                    if player.id == *target_player_id {
                        if let Some(role) = player.role {
                            player_data["role"] = serde_json::to_value(role).unwrap_or(serde_json::Value::Null);
                        }
                        if let Some(word) = &player.word {
                            player_data["word"] = serde_json::to_value(word).unwrap_or(serde_json::Value::Null);
                        }
                    }

                    player_data
                }).collect::<Vec<_>>(),
                "total_players": state.get_players().len()
            });

            // 添加特定状态的数据
            if let Some(current_player_index) = state.get_current_player_index() {
                let players = state.get_players();
                if current_player_index < players.len() {
                    state_data["current_player"] =
                        serde_json::Value::String(players[current_player_index].id.to_string());
                }
            }

            if let Some(descriptions) = state.get_descriptions() {
                state_data["descriptions"] = serde_json::Value::Array(
                    descriptions
                        .iter()
                        .map(|(id, desc)| {
                            serde_json::json!({
                                "player_id": id.to_string(),
                                "content": desc
                            })
                        })
                        .collect(),
                );
            }

            if let Some(eliminated) = state.get_eliminated_player() {
                if eliminated == "tie" {
                    state_data["eliminated"] = serde_json::Value::Null;
                } else {
                    state_data["eliminated"] = serde_json::Value::String(eliminated);
                }
            }

            if let Some(chat_messages) = state.get_chat_messages() {
                state_data["chat_messages"] = serde_json::Value::Array(
                    chat_messages
                        .iter()
                        .map(|msg| {
                            serde_json::json!({
                                "player_id": msg.player_id.to_string(),
                                "player_name": msg.player_name,
                                "content": msg.content,
                                "timestamp": msg.timestamp.timestamp()
                            })
                        })
                        .collect(),
                );
            }

            let state_update = GameMessage {
                type_: "state_update".to_string(),
                data: state_data,
            };

            if let Err(e) = channel.send(state_update).await {
                error!("发送状态更新失败: {}", e);
            }
        }
    }
}

impl Clone for Room {
    fn clone(&self) -> Self {
        Room {
            id: self.id.clone(),
            state: self.state.clone(),
            players: self.players.clone(),
            word_bank: self.word_bank.clone(),
            player_channels: self.player_channels.clone(),
            player_order: self.player_order.clone(),
            storage: self.storage.clone(),
        }
    }
}
