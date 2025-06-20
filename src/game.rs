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
    },
    VotePhase {
        players: Vec<Player>,
        votes: HashMap<PlayerId, PlayerId>,
        start_time: DateTime<Utc>,
        duration: Duration,
        chat_messages: Vec<ChatMessage>,
    },
    ResultPhase {
        players: Vec<Player>,
        eliminated: PlayerId,
        votes: HashMap<PlayerId, PlayerId>,
        next_round_delay: Duration,
    },
    GameOver {
        winner: Role,
        players: Vec<Player>,
    },
}

/// 游戏事件
#[derive(Debug, Clone)]
pub enum GameEvent {
    PlayerJoined(Player),
    PlayerLeft(PlayerId),
    PlayerReady(PlayerId, bool),
    GameStarted(Vec<Player>),
    DescriptionAdded(PlayerId, String),
    NextPlayer(PlayerId),
    DescribePhaseComplete,
    VoteAdded(PlayerId, PlayerId),
    VoteChanged(PlayerId, PlayerId, PlayerId),
    VotePhaseComplete(HashMap<PlayerId, PlayerId>),
    PlayerEliminated(PlayerId),
    VoteTied,
    RoundComplete,
    GameOver(Role),
    ChatMessageAdded(ChatMessage),
    GameReset,
}

/// 超时检测结果
#[derive(Debug, Clone)]
pub enum TimeoutResult {
    None,
    DescribeTimeout(PlayerId), // 当前玩家超时
    VoteTimeout,
    ResultTimeout,
}

impl GameState {
    /// 创建新的游戏状态
    pub fn new(min_players: usize, max_players: usize) -> Self {
        GameState::Lobby {
            players: HashMap::new(),
            min_players,
            max_players,
            ready_players: HashSet::new(),
            chat_messages: Vec::new(),
        }
    }

    /// 重置游戏状态（从GameOver状态重置到Lobby状态）
    pub fn reset_game(&mut self) -> Result<GameEvent, String> {
        match self {
            GameState::GameOver { players, .. } => {
                // 使用全局配置中的min_players和max_players设置
                let config = crate::config::Config::get();
                let min_players = config.game.min_players;
                let max_players = config.game.max_players;

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
                    chat_messages: Vec::new(),
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
                players.remove(&player_id);
                ready_players.remove(&player_id);
                Ok(GameEvent::PlayerLeft(player_id))
            }
            _ => Err("游戏已经开始".to_string()),
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

                // 检查玩家是否已经准备过
                if ready_players.contains(&player_id) {
                    return Err("您已经准备过了".to_string());
                }

                let player_id_clone = player_id.clone();
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

                        if ready_players.contains(&player_id) {
                            return Err("您已经准备过了".to_string());
                        }

                        let player_id_clone = player_id.clone();
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
                };

                Ok(GameEvent::GameStarted(players_vec))
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
                current_player_start_time,
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

                descriptions.insert(player_id, description.clone());

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
                            start_time: Utc::now(),
                            duration: crate::config::Config::get().vote_time_limit(),
                            chat_messages: Vec::new(),
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

                if !players.iter().any(|p| p.id == target_id && p.is_alive) {
                    return Err("目标玩家已被淘汰".to_string());
                }

                // 检查是否已经投过票
                if let Some(previous_target) = votes.get(&voter_id) {
                    // 更改投票
                    if previous_target == &target_id {
                        return Err("您已经投给这个玩家了".to_string());
                    }
                    
                    let previous_target = previous_target.clone();
                    votes.insert(voter_id.clone(), target_id.clone());
                    
                    Ok(GameEvent::VoteChanged(voter_id, previous_target, target_id))
                } else {
                    // 首次投票
                    votes.insert(voter_id.clone(), target_id.clone());
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
        match self {
            GameState::Lobby {
                chat_messages,
                players,
                ..
            } => {
                let player = players
                    .get(&player_id)
                    .ok_or_else(|| "玩家不存在".to_string())?;

                let chat_message = ChatMessage {
                    player_id,
                    player_name: player.name.clone(),
                    content,
                    timestamp: Utc::now(),
                };

                chat_messages.push(chat_message.clone());

                Ok(GameEvent::ChatMessageAdded(chat_message))
            }
            GameState::VotePhase {
                chat_messages,
                players,
                ..
            } => {
                if !players.iter().any(|p| p.id == player_id && p.is_alive) {
                    return Err("您已被淘汰，无法发言".to_string());
                }

                let player = players
                    .iter()
                    .find(|p| p.id == player_id)
                    .ok_or_else(|| "玩家不存在".to_string())?;

                let chat_message = ChatMessage {
                    player_id,
                    player_name: player.name.clone(),
                    content,
                    timestamp: Utc::now(),
                };

                chat_messages.push(chat_message.clone());

                Ok(GameEvent::ChatMessageAdded(chat_message))
            }
            _ => Err("当前阶段不支持聊天".to_string()),
        }
    }

    /// 处理投票结果
    fn process_votes(&mut self) -> Result<(), String> {
        match self {
            GameState::VotePhase { votes, players, .. } => {
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
                    };
                } else {
                    let tie_id = "tie".to_string();
                    *self = GameState::ResultPhase {
                        players: players.clone(),
                        eliminated: tie_id,
                        votes: votes.clone(),
                        next_round_delay: crate::config::Config::get().round_delay(),
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
                ..
            } => {
                if *eliminated != "tie" {
                    if let Some(player) = players.iter_mut().find(|p| p.id == *eliminated) {
                        player.is_alive = false;
                    }
                }

                let alive_players: Vec<&Player> = players.iter().filter(|p| p.is_alive).collect();

                let undercover_count = alive_players
                    .iter()
                    .filter(|p| p.role == Some(Role::Undercover))
                    .count();

                let civilian_count = alive_players.len() - undercover_count;

                if undercover_count == 0 {
                    *self = GameState::GameOver {
                        winner: Role::Civilian,
                        players: players.clone(),
                    };
                    Ok(GameEvent::GameOver(Role::Civilian))
                } else if undercover_count > civilian_count {
                    *self = GameState::GameOver {
                        winner: Role::Undercover,
                        players: players.clone(),
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
                ..
            } => {
                if *current_player_index < players.len() {
                    if Utc::now() - *current_player_start_time
                        > chrono::Duration::from_std(*player_duration).unwrap()
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
                ..
            } => {
                let alive_count = players.iter().filter(|p| p.is_alive).count();
                if votes.len() == alive_count {
                    return TimeoutResult::None; // 所有玩家都投票了
                }

                if Utc::now() - *start_time > chrono::Duration::from_std(*duration).unwrap() {
                    return TimeoutResult::VoteTimeout;
                }
                TimeoutResult::None
            }
            GameState::ResultPhase { .. } => TimeoutResult::ResultTimeout,
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
                            start_time: Utc::now(),
                            duration: crate::config::Config::get().vote_time_limit(),
                            chat_messages: Vec::new(),
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
            GameState::VotePhase { votes, players, .. } => {
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

    /// 获取当前玩家列表
    pub fn get_players(&self) -> Vec<Player> {
        match self {
            GameState::Lobby { players, .. } => players.values().cloned().collect(),
            GameState::RoleAssignment { players } => players.clone(),
            GameState::DescribePhase { players, .. } => players.clone(),
            GameState::VotePhase { players, .. } => players.clone(),
            GameState::ResultPhase { players, .. } => players.clone(),
            GameState::GameOver { players, .. } => players.clone(),
        }
    }

    /// 获取当前描述列表
    pub fn get_descriptions(&self) -> Option<HashMap<PlayerId, String>> {
        match self {
            GameState::DescribePhase { descriptions, .. } => Some(descriptions.clone()),
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
            GameState::VotePhase { chat_messages, .. } => Some(chat_messages.clone()),
            _ => None,
        }
    }
}
