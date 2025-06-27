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
use tracing::{debug, error, info};
use std::time::Duration;

/// 房间删除回调函数类型
pub type RoomDeleteCallback = Box<dyn Fn(String) + Send + Sync>;

/// 跨房间玩家踢出回调函数类型
pub type PlayerKickCallback = Box<dyn Fn(String, String) + Send + Sync>;

/// 游戏房间，负责管理房间内的玩家和游戏状态
pub struct Room {
    id: String,
    state: Arc<RwLock<GameState>>,
    players: Arc<DashMap<PlayerId, Player>>,
    word_bank: Arc<WordBank>,
    player_channels: Arc<DashMap<PlayerId, mpsc::Sender<GameMessage>>>,
    player_order: Arc<Mutex<Vec<PlayerId>>>,
    storage: Arc<Storage>,
    last_activity: Arc<Mutex<chrono::DateTime<Utc>>>,
    delete_callback: Option<Arc<RoomDeleteCallback>>,
    player_kick_callback: Option<Arc<PlayerKickCallback>>, // 跨房间玩家踢出回调
    heartbeat_interval: Duration,
    max_idle_time: Duration,
    is_new_room: Arc<Mutex<bool>>, // 标记是否为新创建的房间
    is_deleted: Arc<Mutex<bool>>, // 标记房间是否已被删除
    host: Arc<Mutex<PlayerId>>, // 房主ID
}

impl Room {
    /// 创建新房间
    pub fn new(
        id: String,
        min_players: usize,
        max_players: usize,
        word_bank: Arc<WordBank>,
        storage: Arc<Storage>,
        host: PlayerId,
    ) -> Self {
        let config = crate::config::Config::get();
        let state = Arc::new(RwLock::new(GameState::new(min_players, max_players, host.clone())));

        Room {
            id,
            state,
            players: Arc::new(DashMap::new()),
            word_bank,
            player_channels: Arc::new(DashMap::new()),
            player_order: Arc::new(Mutex::new(Vec::new())),
            storage,
            last_activity: Arc::new(Mutex::new(Utc::now())),
            delete_callback: None,
            player_kick_callback: None,
            heartbeat_interval: config.ping_interval(),
            max_idle_time: Duration::from_secs(300), 
            is_new_room: Arc::new(Mutex::new(true)),
            is_deleted: Arc::new(Mutex::new(false)),
            host: Arc::new(Mutex::new(host)),
        }
    }

    /// 设置房间删除回调
    pub fn set_delete_callback(&mut self, callback: RoomDeleteCallback) {
        self.delete_callback = Some(Arc::new(callback));
    }

    /// 设置跨房间玩家踢出回调
    pub fn set_player_kick_callback(&mut self, callback: PlayerKickCallback) {
        self.player_kick_callback = Some(Arc::new(callback));
    }

    /// 获取房间ID
    pub fn id(&self) -> &str {
        &self.id
    }

    /// 检查房间是否已被删除
    pub async fn is_deleted(&self) -> bool {
        *self.is_deleted.lock().await
    }

    /// 获取房间状态信息
    pub async fn get_status(&self) -> (usize, u64, bool, bool) {
        let player_count = self.players.len();
        let last_activity = self.last_activity.lock().await;
        let idle_time = Utc::now() - *last_activity;
        let idle_seconds = idle_time.num_seconds() as u64;
        
        let state = self.state.read().await;
        let is_game_over = matches!(*state, crate::game::GameState::GameOver { .. });
        drop(state);
        
        (player_count, idle_seconds, is_game_over, self.players.is_empty())
    }

    /// 更新最后活动时间
    pub async fn update_activity(&self) {
        let mut last_activity = self.last_activity.lock().await;
        *last_activity = Utc::now();
    }

    /// 检查房间是否应该被删除
    pub async fn should_be_deleted(&self) -> bool {
        // 如果房间已经被删除，直接返回false
        if *self.is_deleted.lock().await {
            return false;
        }
        
        let last_activity = self.last_activity.lock().await;
        let idle_time = Utc::now() - *last_activity;
        let idle_duration = Duration::from_secs(idle_time.num_seconds() as u64);
        
        // 房间为空且超过最大空闲时间，或者房间状态为游戏结束且超过空闲时间
        let is_empty = self.players.is_empty();
        let is_new = *self.is_new_room.lock().await;
        let state = self.state.read().await;
        let is_game_over = matches!(*state, crate::game::GameState::GameOver { .. });
        drop(state);
        
        let should_delete = (is_empty && !is_new) || (idle_duration > self.max_idle_time) && is_empty;
        
        // 添加详细的调试信息
        debug!(
            "房间 {} 删除检查 - 玩家数: {}, 空闲时间: {}秒, 最大空闲时间: {}秒, 游戏结束: {}, 新房间: {}, 删除: {}",
            self.id,
            self.players.len(),
            idle_duration.as_secs(),
            self.max_idle_time.as_secs(),
            is_game_over,
            is_new,
            should_delete
        );
        
        should_delete
    }

    /// 删除房间
    pub async fn delete(&self) {
        // 检查是否已经被删除
        {
            let mut is_deleted = self.is_deleted.lock().await;
            if *is_deleted {
                debug!("房间 {} 已经被删除，跳过重复删除", self.id);
                return;
            }
            *is_deleted = true;
        }
        
        info!("删除房间: {}", self.id);
        
        // 通知所有玩家房间即将关闭
        self.broadcast(GameMessage {
            type_: "room_closing".to_string(),
            data: serde_json::json!({
                "message": "房间即将关闭"
            }),
        }).await;
        
        // 保存最终状态
        if let Err(e) = self.save_state().await {
            error!("保存房间最终状态失败: {}", e);
        }
        
        // 调用删除回调
        if let Some(callback) = &self.delete_callback {
            callback(self.id.clone());
        }
    }

    /// 启动房间心跳和生命周期管理
    pub fn start_lifecycle_management(self: Arc<Self>) {
        tokio::spawn(async move {
            let mut heartbeat_interval = tokio::time::interval(self.heartbeat_interval);
            let mut countdown_interval = tokio::time::interval(Duration::from_secs(1));
            
            info!("Starting lifecycle management for room {}", self.id);

            loop {
                tokio::select! {
                    _ = heartbeat_interval.tick() => {
                        if self.should_be_deleted().await {
                            self.delete().await;
                            break;
                        }
                        
                        // 检查游戏状态超时
                        if let Err(e) = self.check_timeout().await {
                            error!("检查房间 {} 超时失败: {}", self.id, e);
                        }
                    }
                    _ = countdown_interval.tick() => {
                        // 更新倒计时并广播
                        if let Some(_) = self.update_countdown().await {
                        }
                    }
                }
            }
            debug!("Lifecycle management for room {} stopped.", self.id);
        });
    }

    /// 添加玩家到房间
    pub async fn add_player(
        &self,
        player: &Player,
        channel: mpsc::Sender<GameMessage>,
    ) -> Result<()> {
        // 检查房间是否已被删除
        if *self.is_deleted.lock().await {
            return Err(crate::Error::Game("房间已被删除".to_string()));
        }
        
        // 检查玩家是否已在其他房间，如果是则自动离开原房间
        if let Some(other_room_id) = self.storage.get_player_current_room(&player.id).await? {
            if other_room_id != self.id {
                debug!("玩家 {} 已在房间 {} 中，自动离开原房间", player.name, other_room_id);
                
                // 调用跨房间玩家踢出回调
                if let Some(callback) = &self.player_kick_callback {
                    callback(player.id.clone(), other_room_id.clone());
                }
                
                // 清理玩家在原房间的信息，允许加入新房间
                if let Err(e) = self.storage.clear_player_room_info(&player.id).await {
                    error!("清理玩家原房间信息失败: {}", e);
                }
            }
        }
        
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

        // 保存玩家房间信息到存储
        if let Err(e) = self.storage.save_player_room_info(&player.id, &player.name, &self.id).await {
            error!("保存玩家房间信息失败: {}", e);
        }

        // 当第一个玩家加入时，标记房间不再是新房间
        if self.players.len() == 1 {
            let mut is_new = self.is_new_room.lock().await;
            *is_new = false;
        }

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

        // 清理玩家房间信息
        if let Err(e) = self.storage.clear_player_room_info(&player_id).await {
            error!("清理玩家房间信息失败: {}", e);
        }

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 从其他房间踢出玩家（跨房间踢出）
    pub async fn kick_player_from_other_room(&self, player_id: PlayerId) -> Result<()> {
        // 检查玩家是否在当前房间
        if !self.players.contains_key(&player_id) {
            debug!("玩家 {} 不在当前房间，无需踢出", player_id);
            return Ok(());
        }

        // 发送踢出消息给玩家
        if let Some(channel) = self.player_channels.get(&player_id) {
            let kick_message = GameMessage {
                type_: "kicked_from_other_room".to_string(),
                data: serde_json::json!({
                    "message": "您已加入其他房间，已从当前房间断开连接"
                }),
            };
            
            if let Err(e) = channel.send(kick_message).await {
                error!("向被踢玩家发送踢出消息失败: {}", e);
            }
        }

        // 自动处理状态更新、通知和保存
        self.remove_player(player_id).await?;

        Ok(())
    }

    /// 广播消息给房间内所有玩家
    pub async fn broadcast(&self, message: GameMessage) {
        let message = message.clone();
        let mut failed_players = Vec::new();

        for entry in self.player_channels.iter() {
            let player_id = entry.key().clone();
            match entry.value().send(message.clone()).await {
                Ok(_) => {
                    // 消息发送成功
                }
                Err(e) => {
                    // 记录发送失败的玩家ID
                    error!("向玩家 {} 广播消息失败: {}", player_id, e);
                    failed_players.push(player_id);
                }
            }
        }

        // 移除发送失败的玩家通道
        for player_id in failed_players {
            debug!("移除失效的玩家通道: {}", player_id);
            self.player_channels.remove(&player_id);
        }
    }

    /// 广播消息给被淘汰的玩家
    pub async fn broadcast_to_eliminated_players(&self, message: GameMessage) {
        let message = message.clone();
        let mut failed_players = Vec::new();

        // 获取当前游戏状态中的玩家信息
        let state = self.state.read().await;
        let players = state.get_players_with_roles();
        drop(state);

        // 找出被淘汰的玩家
        let eliminated_players: Vec<PlayerId> = players
            .iter()
            .filter(|p| !p.is_alive)
            .map(|p| p.id.clone())
            .collect();

        // 只向被淘汰的玩家发送消息
        for player_id in eliminated_players {
            if let Some(channel) = self.player_channels.get(&player_id) {
                match channel.send(message.clone()).await {
                    Ok(_) => {
                        // 消息发送成功
                    }
                    Err(e) => {
                        // 记录发送失败的玩家ID
                        error!("向被淘汰玩家 {} 广播消息失败: {}", player_id, e);
                        failed_players.push(player_id);
                    }
                }
            }
        }

        // 移除发送失败的玩家通道
        for player_id in failed_players {
            debug!("移除失效的被淘汰玩家通道: {}", player_id);
            self.player_channels.remove(&player_id);
        }
    }

    /// 处理房间消息
    pub(crate) async fn handle_message(
        &self,
        message: GameMessage,
        player_tx: Option<tokio::sync::mpsc::Sender<GameMessage>>,
    ) -> Result<()> {
        // 检查房间是否已被删除
        if *self.is_deleted.lock().await {
            return Err(crate::Error::Game("房间已被删除".to_string()));
        }
        
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
            "eliminated_chat" => {
                self.handle_eliminated_chat(message).await?;
            }
            "leave" => {
                self.handle_leave(message).await?;
            }
            "kick" => {
                self.handle_kick(message).await?;
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
            .ok_or_else(|| crate::Error::Game("无效的玩家名称".to_string()))?
            .to_string();
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

            // 检查玩家是否已在其他房间，如果是则自动离开原房间
            if let Some(other_room_id) = self.storage.get_player_current_room(&player.id).await? {
                if other_room_id != self.id {
                    debug!("玩家 {} 从房间 {} 切换到房间 {}", player_name, other_room_id, self.id);
                    
                    // 调用跨房间玩家踢出回调
                    if let Some(callback) = &self.player_kick_callback {
                        callback(player.id.clone(), other_room_id.clone());
                    }
                    
                    // 清理玩家在原房间的信息
                    if let Err(e) = self.storage.clear_player_room_info(&player.id).await {
                        error!("清理玩家原房间信息失败: {}", e);
                    }
                }
            }

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

    /// 处理被淘汰玩家聊天消息
    async fn handle_eliminated_chat(&self, message: GameMessage) -> Result<()> {
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
            .add_eliminated_chat_message(player_id, content.to_string())
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

        // 检查玩家是否还存在（可能已经被踢出或其他原因移除）
        if !self.players.contains_key(&player_id) {
            debug!("玩家 {} 已经不在房间中，忽略离开消息", player_id);
            return Ok(());
        }

        // 获取当前游戏状态
        let state_type = {
            let state = self.state.read().await;
            state.get_state_type()
        };

        // 如果游戏已经开始，先标记玩家为非活跃状态，而不是完全移除
        match state_type {
            crate::message::GameStateType::Lobby => {
                // 在大厅状态，直接移除玩家
                self.remove_player(player_id).await?;
            },
            crate::message::GameStateType::GameOver => {
                // 游戏结束状态，直接移除玩家
                self.remove_player(player_id).await?;
            },
            _ => {
                // 游戏进行中，标记玩家为非活跃
                let state = self.state.write().await;
                
                // 获取玩家列表
                let mut players = state.get_players();
                
                // 找到要离开的玩家并获取其名字
                let mut player_name = "未知玩家".to_string();
                for player in &mut players {
                    if player.id == player_id {
                        player_name = player.name.clone();
                        player.is_alive = false;
                        break;
                    }
                }
                
                // 从通信通道中移除玩家
                self.player_channels.remove(&player_id);
                
                // 清理玩家的房间信息（游戏进行中离开时也要清理）
                if let Err(e) = self.storage.clear_player_room_info(&player_id).await {
                    error!("清理离开玩家房间信息失败: {}", e);
                }
                
                // 广播玩家离开的消息
                drop(state);
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 离开了游戏", player_name)
                    }),
                }).await;
                self.broadcast_state_update().await;
            }
        }

        Ok(())
    }

    /// 处理房主踢人消息
    async fn handle_kick(&self, message: GameMessage) -> Result<()> {
        let message_data = message.data.clone();
        let kicker_id = message_data["player_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的踢人者ID".to_string()))?
            .to_string();
        let target_id = message_data["target_id"]
            .as_str()
            .ok_or_else(|| crate::Error::Game("无效的目标ID".to_string()))?
            .to_string();

        let mut state = self.state.write().await;
        let event = state
            .kick_player(kicker_id, target_id)
            .map_err(|e| crate::Error::Game(e))?;

        // 处理事件
        drop(state);
        Box::pin(self.handle_game_event(event)).await?;

        Ok(())
    }

    /// 获取房间内玩家数量
    pub fn player_count(&self) -> usize {
        self.players.len()
    }

    /// 获取房主ID
    pub async fn get_host(&self) -> PlayerId {
        self.host.lock().await.clone()
    }

    /// 检查玩家是否为房主
    pub async fn is_host(&self, player_id: &PlayerId) -> bool {
        let host = self.host.lock().await;
        *host == *player_id
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
            GameEvent::PlayerLeft(player) => {
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 离开了游戏", player.name)
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

                // 广播状态更新
                self.broadcast_state_update().await;

                // 只有当所有玩家都准备好时，才自动开始游戏
                if can_start {
                    // 检查是否所有玩家都已准备
                    let state = self.state.read().await;
                    let all_players_ready = match &*state {
                        GameState::Lobby { players, ready_players, .. } => {
                            players.len() == ready_players.len() && ready_players.len() >= config::Config::get().game.min_players
                        },
                        _ => false
                    };
                    drop(state);
                    
                    if all_players_ready {
                        Box::pin(self.start_game()).await?;
                    }
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
            GameEvent::DescriptionAdded(player_id, ..) => {
                // 获取当前状态中的描述列表
                let state = self.state.read().await;
                if let Some(descriptions) = state.get_descriptions() {
                    let players = state.get_players();
                    
                    // 构建完整的描述列表
                    let descriptions_list: Vec<serde_json::Value> = descriptions
                        .iter()
                        .filter_map(|(player_id, description)| {
                            players.iter()
                                .find(|p| p.id == *player_id)
                                .map(|player| {
                                    serde_json::json!({
                                        "player_id": player_id,
                                        "player_name": player.name,
                                        "description": description
                                    })
                                })
                        })
                        .collect();

                    // 广播完整的描述列表
                    self.broadcast(GameMessage {
                        type_: "descriptions_update".to_string(),
                        data: serde_json::json!({
                            "message": format!("玩家 {} 完成了描述", player_id),
                            "descriptions": descriptions_list
                        }),
                    })
                    .await;
                }
                drop(state);
                
                // 推进描述阶段到下一个玩家
                let mut state = self.state.write().await;
                let advance_event = state
                    .advance_describe_phase()
                    .map_err(|e| crate::Error::Game(e))?;
                drop(state);
                
                // 递归处理推进事件
                Box::pin(self.handle_game_event(advance_event)).await?;
                
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
                // 获取当前状态中的描述列表
                let state = self.state.read().await;
                if let Some(descriptions) = state.get_descriptions() {
                    let players = state.get_players();
                    
                    // 构建完整的描述列表
                    let descriptions_list: Vec<serde_json::Value> = descriptions
                        .iter()
                        .filter_map(|(player_id, description)| {
                            players.iter()
                                .find(|p| p.id == *player_id)
                                .map(|player| {
                                    serde_json::json!({
                                        "player_id": player_id,
                                        "player_name": player.name,
                                        "description": description
                                    })
                                })
                        })
                        .collect();

                    // 广播完整的描述列表
                    self.broadcast(GameMessage {
                        type_: "descriptions_update".to_string(),
                        data: serde_json::json!({
                            "message": format!("轮到玩家 {} 描述", player_name),
                            "descriptions": descriptions_list
                        }),
                    })
                    .await;
                }
                drop(state);
                self.broadcast_state_update().await;
                // 保存状态
                self.save_state().await?;
            }
            GameEvent::DescribePhaseComplete => {
                // 先广播所有描述
                let state = self.state.read().await;
                if let Some(descriptions) = state.get_descriptions() {
                    let players = state.get_players();
                    
                    // 构建描述列表
                    let descriptions_list: Vec<serde_json::Value> = descriptions
                        .iter()
                        .filter_map(|(player_id, description)| {
                            players.iter()
                                .find(|p| p.id == *player_id)
                                .map(|player| {
                                    serde_json::json!({
                                        "player_id": player_id,
                                        "player_name": player.name,
                                        "description": description
                                    })
                                })
                        })
                        .collect();

                    // 广播描述列表
                    self.broadcast(GameMessage {
                        type_: "descriptions_update".to_string(),
                        data: serde_json::json!({
                            "descriptions": descriptions_list
                        }),
                    })
                    .await;
                }
                drop(state);

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
                // 获取投票者和被投票者的名称
                let voter_name = self
                    .players
                    .get(&voter_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "未知玩家".to_string());
                let target_name = self
                    .players
                    .get(&target_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "未知玩家".to_string());

                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 投票给了 {}", voter_name, target_name),
                        "voter_id": voter_id,
                        "voter_name": voter_name,
                        "target_id": target_id,
                        "target_name": target_name
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
            GameEvent::EliminatedChatMessageAdded(chat_message) => {
                // 只向被淘汰的玩家广播被淘汰聊天消息
                self.broadcast_to_eliminated_players(GameMessage {
                    type_: "eliminated_chat".to_string(),
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
            GameEvent::CountdownUpdate(remaining_time) => {
                // 倒计时更新事件，直接广播给所有玩家
                self.broadcast(GameMessage {
                    type_: "countdown".to_string(),
                    data: serde_json::json!({
                        "seconds": remaining_time.as_secs()
                    }),
                })
                .await;
            }
            GameEvent::PlayerKicked(kicked_player, kicker_id) => {
                // 获取踢人者的信息
                let kicker_player = self.players.get(&kicker_id).map(|r| r.clone());
                
                // 在被踢玩家被移除之前，先发送踢出消息给被踢玩家
                if let Some(kicked_channel) = self.player_channels.get(&kicked_player.id) {
                    let kicker_name = kicker_player.as_ref().map(|p| p.name.clone()).unwrap_or_else(|| "房主".to_string());
                    let kick_message = GameMessage {
                        type_: "kicked".to_string(),
                        data: serde_json::json!({
                            "message": format!("您被房主 {} 踢出了房间", kicker_name)
                        }),
                    };
                    
                    if let Err(e) = kicked_channel.send(kick_message).await {
                        error!("向被踢玩家发送踢出消息失败: {}", e);
                    }
                }
                
                // 从房间中移除被踢玩家
                self.players.remove(&kicked_player.id);
                self.player_channels.remove(&kicked_player.id);
                self.player_order.lock().await.retain(|id| id != &kicked_player.id);
                
                // 清理被踢玩家的房间信息
                if let Err(e) = self.storage.clear_player_room_info(&kicked_player.id).await {
                    error!("清理被踢玩家房间信息失败: {}", e);
                }
                
                // 广播踢人消息给其他玩家
                let kicker_name = kicker_player.map(|p| p.name).unwrap_or_else(|| "房主".to_string());
                self.broadcast(GameMessage {
                    type_: "notification".to_string(),
                    data: serde_json::json!({
                        "message": format!("玩家 {} 被房主 {} 踢出了房间", kicked_player.name, kicker_name)
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

            // 添加房主信息
            if let Some(host_id) = state.get_host() {
                state_data["host"] = serde_json::Value::String(host_id);
            }

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

            // 为被淘汰的玩家添加被淘汰聊天消息
            if let Some(eliminated_chat_messages) = state.get_eliminated_chat_messages() {
                state_data["eliminated_chat_messages"] = serde_json::Value::Array(
                    eliminated_chat_messages
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

            // 添加投票信息
            if let Some(votes) = state.get_votes() {
                state_data["votes"] = serde_json::Value::Array(
                    votes
                        .iter()
                        .map(|(voter_id, target_id)| {
                            serde_json::json!({
                                "player_id": voter_id.to_string(),
                                "target_id": target_id.to_string()
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

    /// 更新倒计时并广播
    pub async fn update_countdown(&self) -> Option<Duration> {
        let mut state = self.state.write().await;
        if let Some(remaining_time) = state.update_countdown() {
            drop(state);
            
            // 广播倒计时更新
            self.broadcast(GameMessage {
                type_: "countdown".to_string(),
                data: serde_json::json!({
                    "seconds": remaining_time.as_secs()
                }),
            }).await;
            
            Some(remaining_time)
        } else {
            // 倒计时为0时，立即检查超时
            drop(state);
            
            // 检查并处理超时
            if let Err(e) = self.check_timeout().await {
                error!("倒计时结束时检查超时失败: {}", e);
            }
            
            None
        }
    }
}
