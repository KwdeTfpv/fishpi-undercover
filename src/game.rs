use crate::message::GameStateType;
use crate::word_bank::WordBank;
use chrono::{DateTime, Utc};
use rand::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

/// 玩家ID类型
pub type PlayerId = String;

/// 玩家角色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Role {
    Undercover,
    Civilian,
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Role::Undercover => write!(f, "卧底"),
            Role::Civilian => write!(f, "平民"),
        }
    }
}

/// 玩家信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Player {
    pub id: PlayerId,
    pub name: String,
    pub role: Option<Role>,
    pub word: Option<String>,
    pub is_alive: bool,
    pub last_action: DateTime<Utc>,
}

/// 聊天消息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub player_id: PlayerId,
    pub player_name: String,
    pub content: String,
    pub timestamp: DateTime<Utc>,
}

/// 游戏状态
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GameState {
    Lobby {
        players: HashMap<PlayerId, Player>,
        min_players: usize,
        max_players: usize,
        ready_players: HashSet<PlayerId>,
        chat_messages: Vec<ChatMessage>,
        eliminated_chat_messages: Vec<ChatMessage>,
        host: PlayerId,
    },
    RoleAssignment {
        players: Vec<Player>,
    },
    DescribePhase {
        players: Vec<Player>,
        current_player_index: usize,
        descriptions: HashMap<PlayerId, String>,
        current_player_start_time: DateTime<Utc>,
        player_duration: Duration,
        remaining_time: Duration,
        chat_messages: Vec<ChatMessage>,
        eliminated_chat_messages: Vec<ChatMessage>,
        host: PlayerId,
    },
    VotePhase {
        players: Vec<Player>,
        votes: HashMap<PlayerId, PlayerId>,
        descriptions: HashMap<PlayerId, String>,
        start_time: DateTime<Utc>,
        duration: Duration,
        remaining_time: Duration,
        chat_messages: Vec<ChatMessage>,
        eliminated_chat_messages: Vec<ChatMessage>,
        host: PlayerId,
    },
    ResultPhase {
        players: Vec<Player>,
        eliminated: PlayerId,
        votes: HashMap<PlayerId, PlayerId>,
        next_round_delay: Duration,
        remaining_time: Duration,
        start_time: DateTime<Utc>,
        chat_messages: Vec<ChatMessage>,
        eliminated_chat_messages: Vec<ChatMessage>,
        host: PlayerId,
    },
    GameOver {
        winner: Role,
        players: Vec<Player>,
        chat_messages: Vec<ChatMessage>,
        eliminated_chat_messages: Vec<ChatMessage>,
        host: PlayerId,
    },
}

/// 游戏事件
#[derive(Debug, Clone)]
pub enum GameEvent {
    PlayerJoined(Player),
    PlayerLeft(Player),
    PlayerReady(PlayerId, bool),
    GameStarted(Vec<Player>),
    DescriptionAdded(PlayerId, String),
    NextPlayer(PlayerId),
    DescribePhaseComplete,
    VoteAdded(PlayerId, PlayerId),
    VotePhaseComplete(HashMap<PlayerId, PlayerId>),
    PlayerEliminated(PlayerId),
    VoteTied,
    RoundComplete,
    GameOver(Role),
    ChatMessageAdded(ChatMessage),
    EliminatedChatMessageAdded(ChatMessage),
    GameReset,
    CountdownUpdate(Duration),
    PlayerKicked(Player, PlayerId),
}

/// 超时检测结果
#[derive(Debug, Clone)]
pub enum TimeoutResult {
    None,
    DescribeTimeout(PlayerId), 
    VoteTimeout,
    ResultTimeout,
}

impl GameState {
    /// 创建新的游戏状态
    pub fn new(min_players: usize, max_players: usize, host: PlayerId) -> Self {
        GameState::Lobby {
            players: HashMap::new(),
            min_players,
            max_players,
            ready_players: HashSet::new(),
            chat_messages: Vec::new(),
            eliminated_chat_messages: Vec::new(),
            host,
        }
    }

    /// 重置游戏状态（从GameOver状态重置到Lobby状态）
    pub fn reset_game(&mut self) -> Result<GameEvent, String> {
        match self {
            GameState::GameOver { players, chat_messages, .. } => {
                // 使用全局配置中的min_players和max_players设置
                let config = crate::config::Config::get();
                let min_players = config.game.min_players;
                let max_players = config.game.max_players;

                // 获取当前房主（第一个玩家）
                let host = if let Some(first_player) = players.first() {
                    first_player.id.clone()
                } else {
                    return Err("没有玩家可以成为房主".to_string());
                };

                // 重置所有玩家状态
                let mut reset_players = HashMap::new();
                for player in players {
                    let mut reset_player = player.clone();
                    reset_player.role = None;
                    reset_player.word = None;
                    reset_player.is_alive = true;
                    reset_player.last_action = Utc::now();
                    reset_players.insert(player.id.clone(), reset_player);
                }

                *self = GameState::Lobby {
                    players: reset_players,
                    min_players,
                    max_players,
                    ready_players: HashSet::new(),
                    chat_messages: chat_messages.clone(),
                    eliminated_chat_messages: Vec::new(),
                    host,
                };

                Ok(GameEvent::GameReset)
            }
            _ => Err("只有游戏结束状态才能重置".to_string()),
        }
    }

    /// 添加玩家
    pub fn add_player(&mut self, player: Player) -> Result<GameEvent, String> {
        match self {
            GameState::Lobby {
                players,
                max_players,
                ready_players,
                ..
            } => {
                if players.len() >= *max_players {
                    return Err("房间已满".to_string());
                }
                let player_id = player.id.clone();

                // 检查玩家是否已经存在
                if players.contains_key(&player_id) {
                    // 玩家已存在，不触发PlayerJoined事件
                    return Ok(GameEvent::PlayerJoined(player));
                }

                // 确保新加入的玩家不在准备列表中
                ready_players.remove(&player_id);
                
                players.insert(player_id, player.clone());
                Ok(GameEvent::PlayerJoined(player))
            }
            _ => Err("游戏已经开始".to_string()),
        }
    }

    /// 移除玩家
    pub fn remove_player(&mut self, player_id: PlayerId) -> Result<GameEvent, String> {
        match self {
            GameState::Lobby {
                players,
                ready_players,
                ..
            } => {
                // 先获取玩家信息，再移除
                let player = players.get(&player_id)
                    .cloned()
                    .ok_or_else(|| "玩家不存在".to_string())?;
                
                players.remove(&player_id);
                ready_players.remove(&player_id);
                Ok(GameEvent::PlayerLeft(player))
            }
            _ => Err("游戏已经开始".to_string()),
        }
    }

    /// 房主踢人
    pub fn kick_player(&mut self, kicker_id: PlayerId, target_id: PlayerId) -> Result<GameEvent, String> {
        match self {
            GameState::Lobby {
                players,
                ready_players,
                host,
                ..
            } => {
                // 检查踢人者是否为房主
                if *host != kicker_id {
                    return Err("只有房主可以踢人".to_string());
                }

                // 检查目标玩家是否存在
                if !players.contains_key(&target_id) {
                    return Err("目标玩家不存在".to_string());
                }

                // 房主不能踢自己
                if kicker_id == target_id {
                    return Err("房主不能踢自己".to_string());
                }

                // 先获取玩家信息，再移除
                let player = players.get(&target_id)
                    .cloned()
                    .ok_or_else(|| "玩家不存在".to_string())?;
                
                players.remove(&target_id);
                ready_players.remove(&target_id);
                Ok(GameEvent::PlayerKicked(player, kicker_id))
            }
            _ => Err("只有在大厅状态才能踢人".to_string()),
        }
    }

    /// 玩家准备
    pub fn player_ready(&mut self, player_id: PlayerId) -> Result<GameEvent, String> {
        match self {
            GameState::Lobby {
                players,
                ready_players,
                min_players,
                ..
            } => {
                if !players.contains_key(&player_id) {
                    return Err("玩家不存在".to_string());
                }

                let player_id_clone = player_id.clone();
                
                // 如果玩家已经准备，则取消准备
                if ready_players.contains(&player_id) {
                    ready_players.remove(&player_id);
                    let can_start = ready_players.len() >= *min_players;
                    return Ok(GameEvent::PlayerReady(player_id_clone, can_start));
                }
                
                // 玩家未准备，设置为准备状态
                ready_players.insert(player_id);
                let can_start = ready_players.len() >= *min_players;
                Ok(GameEvent::PlayerReady(player_id_clone, can_start))
            }
            GameState::GameOver { .. } => {
                // 游戏结束后，先重置游戏状态
                self.reset_game()?;
                // 然后直接处理准备逻辑，避免递归调用
                match self {
                    GameState::Lobby {
                        players,
                        ready_players,
                        min_players,
                        ..
                    } => {
                        if !players.contains_key(&player_id) {
                            return Err("玩家不存在".to_string());
                        }

                        let player_id_clone = player_id.clone();
                        
                        // 如果玩家已经准备，则取消准备
                        if ready_players.contains(&player_id) {
                            ready_players.remove(&player_id);
                            let can_start = ready_players.len() >= *min_players;
                            return Ok(GameEvent::PlayerReady(player_id_clone, can_start));
                        }
                        
                        // 玩家未准备，设置为准备状态
                        ready_players.insert(player_id);
                        let can_start = ready_players.len() >= *min_players;
                        Ok(GameEvent::PlayerReady(player_id_clone, can_start))
                    }
                    _ => Err("重置游戏状态失败".to_string()),
                }
            }
            _ => Err("游戏已经开始".to_string()),
        }
    }

    /// 开始游戏
    pub fn start_game(
        &mut self,
        word_bank: Arc<WordBank>,
        player_order: &[PlayerId],
    ) -> Result<GameEvent, String> {
        match self {
            GameState::Lobby {
                players,
                ready_players,
                min_players,
                chat_messages,
                host,
                ..
            } => {
                if ready_players.len() < *min_players {
                    return Err(format!("准备玩家数量不足，需要至少 {} 名玩家", min_players));
                }

                let mut players_vec: Vec<Player> = player_order
                    .iter()
                    .filter_map(|id| players.get(id).cloned())
                    .collect();

                let undercover_count = if players_vec.len() <= 6 {
                    1
                } else {
                    (players_vec.len() as f32 * 0.25).ceil() as usize
                };

                let mut rng = rand::rng();
                let mut indices: Vec<usize> = (0..players_vec.len()).collect();
                indices.shuffle(&mut rng);

                for player in &mut players_vec {
                    player.role = Some(Role::Civilian);
                }

                for i in 0..undercover_count {
                    if i < indices.len() {
                        players_vec[indices[i]].role = Some(Role::Undercover);
                    }
                }

                if let Some(word_pair) = word_bank.get_random_word_pair() {
                    for player in &mut players_vec {
                        player.word = Some(match player.role {
                            Some(Role::Undercover) => word_pair.undercover_word.clone(),
                            _ => word_pair.civilian_word.clone(),
                        });
                    }
                } else {
                    return Err("无法获取词语".to_string());
                }

                *self = GameState::DescribePhase {
                    players: players_vec.clone(),
                    current_player_index: 0,
                    descriptions: HashMap::new(),
                    current_player_start_time: Utc::now(),
                    player_duration: crate::config::Config::get().describe_time_limit(),
                    remaining_time: crate::config::Config::get().describe_time_limit(),
                    chat_messages: chat_messages.clone(),
                    eliminated_chat_messages: Vec::new(),
                    host: host.clone(),
                };

                // 创建不包含角色信息的玩家列表用于事件
                let players_without_roles: Vec<Player> = players_vec.iter().map(|p| Player {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    role: None,
                    word: p.word.clone(),
                    is_alive: p.is_alive,
                    last_action: p.last_action,
                }).collect();

                Ok(GameEvent::GameStarted(players_without_roles))
            }
            _ => Err("游戏已经开始".to_string()),
        }
    }

    /// 添加描述
    pub fn add_description(
        &mut self,
        player_id: PlayerId,
        description: String,
    ) -> Result<GameEvent, String> {
        match self {
            GameState::DescribePhase {
                players,
                current_player_index,
                descriptions,
                ..
            } => {
                if *current_player_index >= players.len() {
                    return Err("描述阶段已结束".to_string());
                }

                let current_player = &players[*current_player_index];
                if current_player.id != player_id {
                    return Err("还没轮到您描述".to_string());
                }

                if !current_player.is_alive {
                    return Err("您已被淘汰".to_string());
                }

                descriptions.insert(player_id.clone(), description.clone());

                // 返回 DescriptionAdded 事件，让调用者处理后续逻辑
                Ok(GameEvent::DescriptionAdded(player_id, description))
            }
            _ => Err("当前不是描述阶段".to_string()),
        }
    }

    /// 推进描述阶段（移动到下一个玩家或结束阶段）
    pub fn advance_describe_phase(&mut self) -> Result<GameEvent, String> {
        match self {
            GameState::DescribePhase {
                players,
                current_player_index,
                descriptions,
                current_player_start_time,
                chat_messages,
                host,
                ..
            } => {
                let next_alive_index = players
                    .iter()
                    .enumerate()
                    .skip(*current_player_index + 1)
                    .find(|(_, p)| p.is_alive)
                    .map(|(i, _)| i);

                match next_alive_index {
                    Some(index) => {
                        *current_player_index = index;
                        *current_player_start_time = Utc::now();
                        let player_id = players[index].id.clone();
                        Ok(GameEvent::NextPlayer(player_id))
                    }
                    None => {
                        *self = GameState::VotePhase {
                            players: players.clone(),
                            votes: HashMap::new(),
                            descriptions: descriptions.clone(),
                            start_time: Utc::now(),
                            duration: crate::config::Config::get().vote_time_limit(),
                            remaining_time: crate::config::Config::get().vote_time_limit(),
                            chat_messages: chat_messages.clone(),
                            eliminated_chat_messages: chat_messages.clone(),
                            host: host.clone(),
                        };
                        Ok(GameEvent::DescribePhaseComplete)
                    }
                }
            }
            _ => Err("当前不是描述阶段".to_string()),
        }
    }

    /// 添加投票
    pub fn add_vote(
        &mut self,
        voter_id: PlayerId,
        target_id: PlayerId,
    ) -> Result<GameEvent, String> {
        match self {
            GameState::VotePhase { votes, players, .. } => {
                if !players.iter().any(|p| p.id == voter_id && p.is_alive) {
                    return Err("您已被淘汰，无法投票".to_string());
                }
                
                if votes.contains_key(&voter_id) {
                    return Err("您已经投过票了".to_string());
                }

                if !players.iter().any(|p| p.id == target_id && p.is_alive) {
                    return Err("目标玩家已被淘汰".to_string());
                }

                votes.insert(voter_id.clone(), target_id.clone());

                if votes.len() == players.iter().filter(|p| p.is_alive).count() {
                    let votes_clone = votes.clone();
                    self.process_votes()?;
                    Ok(GameEvent::VotePhaseComplete(votes_clone))
                } else {
                    Ok(GameEvent::VoteAdded(voter_id, target_id))
                }
            }
            _ => Err("当前不是投票阶段".to_string()),
        }
    }

    /// 添加聊天消息
    pub fn add_chat_message(
        &mut self,
        player_id: PlayerId,
        content: String,
    ) -> Result<GameEvent, String> {
        // 检查当前阶段是否允许聊天
        match self {
            GameState::Lobby { .. } | GameState::VotePhase { .. } | GameState::GameOver { .. } => {
                // 允许聊天的阶段
            }
            _ => {
                return Err("当前阶段不能喷垃圾话".to_string());
            }
        }

        // 查找玩家名称
        let player_name = match self {
            GameState::Lobby { players, .. } => players.get(&player_id).map(|p| p.name.clone()),
            GameState::VotePhase { players, .. } => players.iter().find(|p| p.id == player_id).map(|p| p.name.clone()),
            GameState::GameOver { players, .. } => players.iter().find(|p| p.id == player_id).map(|p| p.name.clone()),
            _ => unreachable!(), 
        }.ok_or_else(|| "玩家不存在".to_string())?;

        let message = ChatMessage {
            player_id: player_id.clone(),
            player_name,
            content,
            timestamp: Utc::now(),
        };

        // 将消息添加到聊天记录
        match self {
            GameState::Lobby { chat_messages, .. } => {
                chat_messages.push(message.clone());
            }
            GameState::VotePhase { chat_messages, .. } => {
                chat_messages.push(message.clone());
            }
            GameState::GameOver { chat_messages, .. } => {
                chat_messages.push(message.clone());
            }
            _ => {}
        }

        Ok(GameEvent::ChatMessageAdded(message))
    }

    /// 添加被淘汰玩家聊天消息
    pub fn add_eliminated_chat_message(
        &mut self,
        player_id: PlayerId,
        content: String,
    ) -> Result<GameEvent, String> {
        // 检查玩家是否已被淘汰
        let is_eliminated = match self {
            GameState::DescribePhase { players, .. } |
            GameState::VotePhase { players, .. } |
            GameState::ResultPhase { players, .. } |
            GameState::GameOver { players, .. } => {
                players.iter().any(|p| p.id == player_id && !p.is_alive)
            }
            _ => false,
        };

        if !is_eliminated {
            return Err("只有被淘汰的玩家才能在被淘汰聊天区发言".to_string());
        }

        // 查找玩家名称
        let player_name = match self {
            GameState::DescribePhase { players, .. } |
            GameState::VotePhase { players, .. } |
            GameState::ResultPhase { players, .. } |
            GameState::GameOver { players, .. } => {
                players.iter().find(|p| p.id == player_id).map(|p| p.name.clone())
            }
            _ => None,
        }.ok_or_else(|| "玩家不存在".to_string())?;

        let message = ChatMessage {
            player_id: player_id.clone(),
            player_name,
            content,
            timestamp: Utc::now(),
        };

        // 将消息添加到被淘汰玩家聊天记录
        match self {
            GameState::DescribePhase { eliminated_chat_messages, .. } |
            GameState::VotePhase { eliminated_chat_messages, .. } |
            GameState::ResultPhase { eliminated_chat_messages, .. } |
            GameState::GameOver { eliminated_chat_messages, .. } => {
                eliminated_chat_messages.push(message.clone());
            }
            _ => {}
        }

        Ok(GameEvent::EliminatedChatMessageAdded(message))
    }

    /// 处理投票结果
    fn process_votes(&mut self) -> Result<(), String> {
        match self {
            GameState::VotePhase { votes, players, chat_messages, host, .. } => {
                let mut vote_count: HashMap<PlayerId, usize> = HashMap::new();
                for target_id in votes.values() {
                    *vote_count.entry(target_id.clone()).or_insert(0) += 1;
                }

                let max_votes = vote_count.values().max().unwrap_or(&0);
                let eliminated: Vec<PlayerId> = vote_count
                    .iter()
                    .filter(|(_, count)| **count == *max_votes)
                    .map(|(id, _)| id.clone())
                    .collect();

                if eliminated.len() == 1 {
                    let eliminated_id = eliminated[0].clone();
                    *self = GameState::ResultPhase {
                        players: players.clone(),
                        eliminated: eliminated_id,
                        votes: votes.clone(),
                        next_round_delay: crate::config::Config::get().round_delay(),
                        remaining_time: crate::config::Config::get().round_delay(),
                        start_time: Utc::now(),
                        chat_messages: chat_messages.clone(),
                        eliminated_chat_messages: chat_messages.clone(),
                        host: host.clone(),
                    };
                } else {
                    let tie_id = "tie".to_string();
                    *self = GameState::ResultPhase {
                        players: players.clone(),
                        eliminated: tie_id,
                        votes: votes.clone(),
                        next_round_delay: crate::config::Config::get().round_delay(),
                        remaining_time: crate::config::Config::get().round_delay(),
                        start_time: Utc::now(),
                        chat_messages: chat_messages.clone(),
                        eliminated_chat_messages: chat_messages.clone(),
                        host: host.clone(),
                    };
                }

                Ok(())
            }
            _ => Err("当前不是投票阶段".to_string()),
        }
    }

    /// 处理结果阶段
    pub fn process_result_phase(&mut self) -> Result<GameEvent, String> {
        match self {
            GameState::ResultPhase {
                players,
                eliminated,
                chat_messages,
                host,
                ..
            } => {
                if *eliminated != "tie" {
                    if let Some(player) = players.iter_mut().find(|p| p.id == *eliminated) {
                        player.is_alive = false;
                    }
                }

                // 使用包含角色的玩家信息进行游戏逻辑判断
                let alive_players: Vec<&Player> = players.iter().filter(|p| p.is_alive).collect();

                let undercover_count = alive_players
                    .iter()
                    .filter(|p| p.role == Some(Role::Undercover))
                    .count();

                let civilian_count = alive_players.len() - undercover_count;

                if undercover_count == 0 {
                    // 调试：检查玩家信息是否完整
                    println!("DEBUG: GameOver - Civilian wins");
                    for player in players.iter() {
                        println!("DEBUG: Player {} - Role: {:?}, Word: {:?}", 
                                player.name, player.role, player.word);
                    }
                    
                    *self = GameState::GameOver {
                        winner: Role::Civilian,
                        players: players.clone(),
                        chat_messages: chat_messages.clone(),
                        eliminated_chat_messages: chat_messages.clone(),
                        host: host.clone(),
                    };
                    Ok(GameEvent::GameOver(Role::Civilian))
                } else if undercover_count > civilian_count || (alive_players.len() <= 2 && undercover_count > 0) {
                    // 调试：检查玩家信息是否完整
                    println!("DEBUG: GameOver - Undercover wins");
                    for player in players.iter() {
                        println!("DEBUG: Player {} - Role: {:?}, Word: {:?}", 
                                player.name, player.role, player.word);
                    }
                    
                    *self = GameState::GameOver {
                        winner: Role::Undercover,
                        players: players.clone(),
                        chat_messages: chat_messages.clone(),
                        eliminated_chat_messages: chat_messages.clone(),
                        host: host.clone(),
                    };
                    Ok(GameEvent::GameOver(Role::Undercover))
                } else {
                    let first_alive_index = players
                        .iter()
                        .position(|p| p.is_alive)
                        .ok_or_else(|| "没有存活的玩家".to_string())?;

                    *self = GameState::DescribePhase {
                        players: players.clone(),
                        current_player_index: first_alive_index,
                        descriptions: HashMap::new(),
                        current_player_start_time: Utc::now(),
                        player_duration: crate::config::Config::get().describe_time_limit(),
                        remaining_time: crate::config::Config::get().describe_time_limit(),
                        chat_messages: chat_messages.clone(),
                        eliminated_chat_messages: chat_messages.clone(),
                        host: host.clone(),
                    };
                    Ok(GameEvent::RoundComplete)
                }
            }
            _ => Err("当前不是结果阶段".to_string()),
        }
    }

    /// 检查超时
    pub fn check_timeout(&self) -> TimeoutResult {
        match self {
            GameState::DescribePhase {
                current_player_index,
                players,
                current_player_start_time,
                player_duration,
                remaining_time,
                ..
            } => {
                if *current_player_index < players.len() {
                    // 检查是否超时：时间已过或者倒计时为0
                    if Utc::now() - *current_player_start_time
                        > chrono::Duration::from_std(*player_duration).unwrap()
                        || remaining_time.as_secs() == 0
                    {
                        let player_id = players[*current_player_index].id.clone();
                        return TimeoutResult::DescribeTimeout(player_id);
                    }
                }
                TimeoutResult::None
            }
            GameState::VotePhase {
                start_time,
                duration,
                votes,
                players,
                remaining_time,
                ..
            } => {
                let alive_count = players.iter().filter(|p| p.is_alive).count();
                if votes.len() == alive_count {
                    return TimeoutResult::None; // 所有玩家都投票了
                }

                // 检查是否超时：时间已过或者倒计时为0
                if Utc::now() - *start_time > chrono::Duration::from_std(*duration).unwrap()
                    || remaining_time.as_secs() == 0
                {
                    return TimeoutResult::VoteTimeout;
                }
                TimeoutResult::None
            }
            GameState::ResultPhase { 
                start_time, 
                next_round_delay,
                remaining_time,
                .. 
            } => {
                // 检查是否超时：时间已过或者倒计时为0
                if Utc::now() - *start_time > chrono::Duration::from_std(*next_round_delay).unwrap()
                    || remaining_time.as_secs() == 0
                {
                    return TimeoutResult::ResultTimeout;
                }
                TimeoutResult::None
            }
            _ => TimeoutResult::None,
        }
    }

    /// 处理描述超时
    pub fn handle_describe_timeout(&mut self) -> Result<GameEvent, String> {
        match self {
            GameState::DescribePhase {
                players,
                current_player_index,
                current_player_start_time,
                chat_messages,
                descriptions,
                host,
                ..
            } => {
                // 找到下一个存活的玩家
                let players_clone = players.clone();
                let next_alive_index = players_clone
                    .iter()
                    .enumerate()
                    .skip(*current_player_index + 1)
                    .find(|(_, p)| p.is_alive)
                    .map(|(i, _)| i);

                match next_alive_index {
                    Some(index) => {
                        *current_player_index = index;
                        *current_player_start_time = Utc::now();
                        let player_id = players_clone[index].id.clone();
                        Ok(GameEvent::NextPlayer(player_id))
                    }
                    None => {
                        // 没有更多存活的玩家，进入投票阶段
                        *self = GameState::VotePhase {
                            players: players.clone(),
                            votes: HashMap::new(),
                            descriptions: descriptions.clone(),
                            start_time: Utc::now(),
                            duration: crate::config::Config::get().vote_time_limit(),
                            remaining_time: crate::config::Config::get().vote_time_limit(),
                            chat_messages: chat_messages.clone(),
                            eliminated_chat_messages: chat_messages.clone(),
                            host: host.clone(),
                        };
                        Ok(GameEvent::DescribePhaseComplete)
                    }
                }
            }
            _ => Err("当前不是描述阶段".to_string()),
        }
    }

    /// 处理投票超时
    pub fn handle_vote_timeout(&mut self) -> Result<GameEvent, String> {
        match self {
            GameState::VotePhase { votes, players, chat_messages, host, .. } => {
                let players_clone = players.clone();
                let votes_clone = votes.clone();

                let alive_players: Vec<PlayerId> = players_clone
                    .iter()
                    .filter(|p| p.is_alive)
                    .map(|p| p.id.clone())
                    .collect();

                // 如果只有一个存活玩家，直接进入结果阶段
                if alive_players.len() <= 1 {
                    let eliminated = if alive_players.is_empty() {
                        "tie".to_string()
                    } else {
                        alive_players[0].clone()
                    };

                    *self = GameState::ResultPhase {
                        players: players_clone,
                        eliminated,
                        votes: votes_clone.clone(),
                        next_round_delay: crate::config::Config::get().round_delay(),
                        remaining_time: crate::config::Config::get().round_delay(),
                        start_time: Utc::now(),
                        chat_messages: chat_messages.clone(),
                        eliminated_chat_messages: chat_messages.clone(),
                        host: host.clone(),
                    };

                    return Ok(GameEvent::VotePhaseComplete(votes_clone));
                }

                // 为未投票的玩家随机分配投票
                let mut rng = rand::rng();
                for player_id in alive_players.clone() {
                    if !votes.contains_key(&player_id) {
                        let available_targets: Vec<PlayerId> = alive_players
                            .iter()
                            .filter(|id| *id != &player_id)
                            .cloned()
                            .collect();

                        if let Some(target) = available_targets.choose(&mut rng) {
                            votes.insert(player_id, target.clone());
                        } else {
                            return Err("无法选择投票目标".to_string());
                        }
                    }
                }

                let final_votes = votes.clone();
                self.process_votes()?;
                Ok(GameEvent::VotePhaseComplete(final_votes))
            }
            _ => Err("当前不是投票阶段".to_string()),
        }
    }

    /// 获取游戏状态类型
    pub fn get_state_type(&self) -> GameStateType {
        match self {
            GameState::Lobby { .. } => GameStateType::Lobby,
            GameState::RoleAssignment { .. } => GameStateType::RoleAssignment,
            GameState::DescribePhase { .. } => GameStateType::DescribePhase,
            GameState::VotePhase { .. } => GameStateType::VotePhase,
            GameState::ResultPhase { .. } => GameStateType::ResultPhase,
            GameState::GameOver { .. } => GameStateType::GameOver,
        }
    }

    /// 获取当前玩家列表（游戏结束前不显示角色）
    pub fn get_players(&self) -> Vec<Player> {
        match self {
            GameState::Lobby { players, .. } => players.values().cloned().collect(),
            GameState::RoleAssignment { players } => players.clone(),
            GameState::DescribePhase { players, .. } => {
                // 游戏进行中不显示角色信息
                players.iter().map(|p| Player {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    role: None, // 隐藏角色信息
                    word: p.word.clone(),
                    is_alive: p.is_alive,
                    last_action: p.last_action,
                }).collect()
            },
            GameState::VotePhase { players, .. } => {
                // 游戏进行中不显示角色信息
                players.iter().map(|p| Player {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    role: None, // 隐藏角色信息
                    word: p.word.clone(),
                    is_alive: p.is_alive,
                    last_action: p.last_action,
                }).collect()
            },
            GameState::ResultPhase { players, .. } => {
                // 游戏进行中不显示角色信息
                players.iter().map(|p| Player {
                    id: p.id.clone(),
                    name: p.name.clone(),
                    role: None, // 隐藏角色信息
                    word: p.word.clone(),
                    is_alive: p.is_alive,
                    last_action: p.last_action,
                }).collect()
            },
            GameState::GameOver { .. } => {
                // 游戏结束时显示完整信息
                self.get_players_with_roles()
            },
        }
    }

    /// 获取玩家完整信息（包含角色，仅用于内部逻辑）
    pub fn get_players_with_roles(&self) -> Vec<Player> {
        match self {
            GameState::Lobby { players, .. } => players.values().cloned().collect(),
            GameState::RoleAssignment { players } => players.clone(),
            GameState::DescribePhase { players, .. } => players.clone(),
            GameState::VotePhase { players, .. } => players.clone(),
            GameState::ResultPhase { players, .. } => players.clone(),
            GameState::GameOver { players, .. } => players.clone(),
        }
    }

    /// 获取当前描述列表（按玩家顺序）
    pub fn get_descriptions(&self) -> Option<Vec<(PlayerId, String)>> {
        match self {
            GameState::DescribePhase { descriptions, players, .. } => {
                let mut ordered_descriptions = Vec::new();
                for player in players {
                    if let Some(description) = descriptions.get(&player.id) {
                        ordered_descriptions.push((player.id.clone(), description.clone()));
                    }
                }
                Some(ordered_descriptions)
            }
            GameState::VotePhase { descriptions, players, .. } => {
                let mut ordered_descriptions = Vec::new();
                for player in players {
                    if let Some(description) = descriptions.get(&player.id) {
                        ordered_descriptions.push((player.id.clone(), description.clone()));
                    }
                }
                Some(ordered_descriptions)
            }
            _ => None,
        }
    }

    /// 获取当前玩家索引
    pub fn get_current_player_index(&self) -> Option<usize> {
        match self {
            GameState::DescribePhase {
                current_player_index,
                ..
            } => Some(*current_player_index),
            _ => None,
        }
    }

    /// 获取被淘汰的玩家
    pub fn get_eliminated_player(&self) -> Option<PlayerId> {
        match self {
            GameState::ResultPhase { eliminated, .. } => Some(eliminated.clone()),
            _ => None,
        }
    }

    /// 获取投票信息
    pub fn get_votes(&self) -> Option<HashMap<PlayerId, PlayerId>> {
        match self {
            GameState::VotePhase { votes, .. } => Some(votes.clone()),
            GameState::ResultPhase { votes, .. } => Some(votes.clone()),
            _ => None,
        }
    }

    /// 获取聊天消息
    pub fn get_chat_messages(&self) -> Option<Vec<ChatMessage>> {
        match self {
            GameState::Lobby { chat_messages, .. } => Some(chat_messages.clone()),
            GameState::DescribePhase { chat_messages, .. } => Some(chat_messages.clone()),
            GameState::VotePhase { chat_messages, .. } => Some(chat_messages.clone()),
            GameState::ResultPhase { chat_messages, .. } => Some(chat_messages.clone()),
            GameState::GameOver { chat_messages, .. } => Some(chat_messages.clone()),
            _ => None,
        }
    }

    /// 获取被淘汰玩家聊天消息
    pub fn get_eliminated_chat_messages(&self) -> Option<Vec<ChatMessage>> {
        match self {
            GameState::Lobby { eliminated_chat_messages, .. } => Some(eliminated_chat_messages.clone()),
            GameState::DescribePhase { eliminated_chat_messages, .. } => Some(eliminated_chat_messages.clone()),
            GameState::VotePhase { eliminated_chat_messages, .. } => Some(eliminated_chat_messages.clone()),
            GameState::ResultPhase { eliminated_chat_messages, .. } => Some(eliminated_chat_messages.clone()),
            GameState::GameOver { eliminated_chat_messages, .. } => Some(eliminated_chat_messages.clone()),
            _ => None,
        }
    }

    /// 更新倒计时
    pub fn update_countdown(&mut self) -> Option<Duration> {
        match self {
            GameState::DescribePhase {
                current_player_start_time,
                player_duration,
                remaining_time,
                ..
            } => {
                let elapsed = Utc::now() - *current_player_start_time;
                let elapsed_duration = chrono::Duration::from_std(*player_duration).unwrap();
                let remaining = if elapsed < elapsed_duration {
                    elapsed_duration - elapsed
                } else {
                    chrono::Duration::zero()
                };
                
                let remaining_std = Duration::from_secs(remaining.num_seconds() as u64);
                *remaining_time = remaining_std;
                
                // 当倒计时为0时，返回None，避免持续广播
                if remaining_std.as_secs() == 0 {
                    None
                } else {
                    Some(remaining_std)
                }
            }
            GameState::VotePhase {
                start_time,
                duration,
                remaining_time,
                ..
            } => {
                let elapsed = Utc::now() - *start_time;
                let elapsed_duration = chrono::Duration::from_std(*duration).unwrap();
                let remaining = if elapsed < elapsed_duration {
                    elapsed_duration - elapsed
                } else {
                    chrono::Duration::zero()
                };
                
                let remaining_std = Duration::from_secs(remaining.num_seconds() as u64);
                *remaining_time = remaining_std;
                
                // 当倒计时为0时，返回None，避免持续广播
                if remaining_std.as_secs() == 0 {
                    None
                } else {
                    Some(remaining_std)
                }
            }
            GameState::ResultPhase {
                next_round_delay,
                remaining_time,
                start_time,
                ..
            } => {
                // 计算结果阶段的倒计时
                let elapsed = Utc::now() - *start_time;
                let elapsed_duration = chrono::Duration::from_std(*next_round_delay).unwrap();
                let remaining = if elapsed < elapsed_duration {
                    elapsed_duration - elapsed
                } else {
                    chrono::Duration::zero()
                };
                
                let remaining_std = Duration::from_secs(remaining.num_seconds() as u64);
                *remaining_time = remaining_std;
                
                // 当倒计时为0时，返回None，避免持续广播
                if remaining_std.as_secs() == 0 {
                    None
                } else {
                    Some(remaining_std)
                }
            }
            _ => None,
        }
    }

    /// 获取房主ID
    pub fn get_host(&self) -> Option<PlayerId> {
        match self {
            GameState::Lobby { host, .. } => Some(host.clone()),
            GameState::DescribePhase { host, .. } => Some(host.clone()),
            GameState::VotePhase { host, .. } => Some(host.clone()),
            GameState::ResultPhase { host, .. } => Some(host.clone()),
            GameState::GameOver { host, .. } => Some(host.clone()),
            _ => None,
        }
    }
}
