use crate::{
    Result, message::GameMessage, room::Room, storage::Storage, user::UserManager,
    word_bank::WordBank,
};
use axum::{
    Router,
    extract::Query,
    extract::ws::{Message, WebSocket, WebSocketUpgrade},
    response::Html,
    response::Json,
    routing::get,
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, error, info};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct RoomQuery {
    room_id: Option<String>,
    session_id: Option<String>, // 会话ID参数
}

#[derive(Debug, Deserialize)]
struct AuthCallbackQuery {
    #[serde(rename = "openid.ns")]
    openid_ns: Option<String>,
    #[serde(rename = "openid.mode")]
    openid_mode: Option<String>,
    #[serde(rename = "openid.op_endpoint")]
    openid_op_endpoint: Option<String>,
    #[serde(rename = "openid.claimed_id")]
    openid_claimed_id: Option<String>,
    #[serde(rename = "openid.identity")]
    openid_identity: Option<String>,
    #[serde(rename = "openid.return_to")]
    openid_return_to: Option<String>,
    #[serde(rename = "openid.response_nonce")]
    openid_response_nonce: Option<String>,
    #[serde(rename = "openid.assoc_handle")]
    openid_assoc_handle: Option<String>,
    #[serde(rename = "openid.signed")]
    openid_signed: Option<String>,
    #[serde(rename = "openid.sig")]
    openid_sig: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValidateQuery {
    session_id: String,
}

#[derive(Debug, Serialize)]
struct ValidateResponse {
    success: bool,
    user: Option<crate::user::User>,
    message: Option<String>,
}

/// WebSocket服务器，负责处理网络连接和消息传输
pub struct WebSocketServer {
    rooms: Arc<DashMap<String, Room>>,
    word_bank: Arc<WordBank>,
    storage: Arc<Storage>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>, // 添加用户管理器
}

impl WebSocketServer {
    pub async fn new() -> Self {
        let config = crate::config::Config::get();
        let storage = Arc::new(
            Storage::new(&config.redis.url)
                .await
                .expect("Failed to create storage"),
        );

        WebSocketServer {
            rooms: Arc::new(DashMap::new()),
            word_bank: Arc::new(WordBank::new()),
            storage,
            user_manager: Arc::new(tokio::sync::RwLock::new(UserManager::new())),
        }
    }

    /// 启动WebSocket服务器
    pub async fn start(&self, addr: &str) -> Result<()> {
        let rooms = self.rooms.clone();
        let word_bank = self.word_bank.clone();
        let storage = self.storage.clone();
        let user_manager = self.user_manager.clone();

        let app = Router::new()
            .route("/", get(serve_index))
            .route("/index.html", get(serve_index))
            .route(
                "/ws",
                get({
                    let user_manager = user_manager.clone();
                    move |ws: WebSocketUpgrade, Query(query): Query<RoomQuery>| async move {
                        let room_id = query.room_id.unwrap_or_else(|| generate_random_room_id());
                        let session_id = query.session_id.and_then(|id| Uuid::parse_str(&id).ok());
                        debug!(
                            "收到WebSocket连接请求，房间ID: {}, 会话ID: {:?}",
                            room_id, session_id
                        );

                        ws.on_upgrade(move |socket| async move {
                            handle_room_connection(
                                socket,
                                room_id,
                                session_id,
                                rooms.clone(),
                                word_bank.clone(),
                                storage.clone(),
                                user_manager.clone(),
                            )
                            .await;
                        })
                    }
                }),
            )
            .route(
                "/auth/callback",
                get({
                    let user_manager = user_manager.clone();
                    move |Query(query): Query<AuthCallbackQuery>| async move {
                        handle_auth_callback(query, user_manager.clone()).await
                    }
                }),
            )
            .route(
                "/auth/validate",
                get({
                    let user_manager = user_manager.clone();
                    move |Query(query): Query<ValidateQuery>| async move {
                        handle_validate_session(query, user_manager.clone()).await
                    }
                }),
            )
            .route(
                "/auth/login",
                get({
                    let user_manager = user_manager.clone();
                    move || async move { handle_generate_login_url(user_manager.clone()).await }
                }),
            );

        let listener = tokio::net::TcpListener::bind(addr)
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;
        info!("WebSocket服务器启动在 {}", addr);
        axum::serve(listener, app)
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;
        Ok(())
    }

    /// 获取房间列表
    pub fn get_room_list(&self) -> Vec<(String, usize)> {
        self.rooms
            .iter()
            .map(|entry| (entry.key().clone(), entry.value().player_count()))
            .collect()
    }

    /// 生成登录URL
    pub async fn generate_login_url(&self, return_url: &str, realm: &str) -> Result<String> {
        let user_manager = self.user_manager.read().await;
        user_manager.generate_login_url(return_url, realm)
    }

    /// 处理登录回调
    pub async fn handle_login_callback(
        &self,
        params: HashMap<String, String>,
    ) -> Result<(Uuid, crate::user::User)> {
        let mut user_manager = self.user_manager.write().await;
        user_manager.handle_login(&params).await
    }
}

/// 处理认证回调
async fn handle_auth_callback(
    query: AuthCallbackQuery,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Html<String> {
    // 将查询参数转换为HashMap
    let mut params = HashMap::new();

    if let Some(ns) = query.openid_ns {
        params.insert("openid.ns".to_string(), ns);
    }

    if let Some(mode) = query.openid_mode {
        params.insert("openid.mode".to_string(), mode);
    }

    if let Some(op_endpoint) = query.openid_op_endpoint {
        params.insert("openid.op_endpoint".to_string(), op_endpoint);
    }

    if let Some(claimed_id) = query.openid_claimed_id {
        params.insert("openid.claimed_id".to_string(), claimed_id);
    }

    if let Some(identity) = query.openid_identity {
        params.insert("openid.identity".to_string(), identity);
    }

    if let Some(return_to) = query.openid_return_to {
        params.insert("openid.return_to".to_string(), return_to);
    }

    if let Some(response_nonce) = query.openid_response_nonce {
        params.insert("openid.response_nonce".to_string(), response_nonce);
    }

    if let Some(assoc_handle) = query.openid_assoc_handle {
        params.insert("openid.assoc_handle".to_string(), assoc_handle);
    }

    if let Some(signed) = query.openid_signed {
        params.insert("openid.signed".to_string(), signed);
    }

    if let Some(sig) = query.openid_sig {
        params.insert("openid.sig".to_string(), sig);
    }

    let mut user_manager_guard = user_manager.write().await;
    match user_manager_guard.handle_login(&params).await {
        Ok((session_id, user)) => {
            // 构建重定向页面，包含session_id
            let display_name: String = user.nickname.as_ref().unwrap_or(&user.username).clone();
            let html = format!(
                r#"
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>登录成功</title>
</head>
<body>
    <h1>登录成功！</h1>
    <p>欢迎，{}！</p>
    <p>正在跳转到游戏页面...</p>
    <script>
        // 保存session_id到localStorage
        localStorage.setItem('fishpi_session_id', '{}');
        // 跳转到游戏页面，并传递session_id参数
        window.location.href = '/index.html?session_id={}';
    </script>
</body>
</html>
"#,
                display_name, session_id, session_id
            );
            Html(html)
        }
        Err(e) => {
            let html = format!(
                r#"
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>登录失败</title>
</head>
<body>
    <h1>登录失败</h1>
    <p>错误信息: {}</p>
    <p><a href="/index.html">返回首页</a></p>
</body>
</html>
"#,
                e
            );
            Html(html)
        }
    }
}

/// 处理会话验证
async fn handle_validate_session(
    query: ValidateQuery,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Json<ValidateResponse> {
    let session_id = match Uuid::parse_str(&query.session_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(ValidateResponse {
                success: false,
                user: None,
                message: Some("无效的会话ID格式".to_string()),
            });
        }
    };

    let mut user_manager_guard = user_manager.write().await;
    match user_manager_guard.get_user_by_session(&session_id) {
        Ok(user) => Json(ValidateResponse {
            success: true,
            user: Some(user.clone()),
            message: None,
        }),
        Err(e) => Json(ValidateResponse {
            success: false,
            user: None,
            message: Some(format!("会话验证失败: {}", e)),
        }),
    }
}

/// 处理生成登录URL
async fn handle_generate_login_url(
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Json<serde_json::Value> {
    let user_manager_guard = user_manager.read().await;
    let config = crate::config::Config::get();

    // 从配置文件获取domain并拼接URL
    let return_to = format!("{}/auth/callback", config.auth.domain);
    let realm = config.auth.domain.clone();

    match user_manager_guard.generate_login_url(&return_to, &realm) {
        Ok(login_url) => Json(serde_json::json!({
            "success": true,
            "login_url": login_url
        })),
        Err(e) => Json(serde_json::json!({
            "success": false,
            "error": e.to_string()
        })),
    }
}

/// 提供index.html文件
async fn serve_index() -> Html<String> {
    let index_path = Path::new("index.html");
    match fs::read_to_string(index_path) {
        Ok(content) => Html(content),
        Err(e) => {
            error!("读取index.html失败: {}", e);
            Html(format!("<h1>404 Not Found</h1><p>找不到index.html文件</p>"))
        }
    }
}

/// 处理WebSocket连接
async fn handle_room_connection(
    socket: WebSocket,
    room_id: String,
    session_id: Option<Uuid>,
    rooms: Arc<DashMap<String, Room>>,
    word_bank: Arc<WordBank>,
    storage: Arc<Storage>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) {
    debug!(
        "开始处理WebSocket连接，房间ID: {}, 会话ID: {:?}",
        room_id, session_id
    );
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 直接创建匿名用户，跳过登录验证
    let user = if let Some(session_id) = session_id {
        let mut user_manager_guard = user_manager.write().await;
        match user_manager_guard.get_user_by_session(&session_id) {
            Ok(user) => {
                let display_name: String = user.nickname.as_ref().unwrap_or(&user.username).clone();
                debug!("用户已登录: {} ({})", display_name, user.username);
                Some(user.clone())
            }
            Err(_) => {
                // 会话验证失败，创建匿名用户
                debug!("会话验证失败，使用匿名用户");
                Some(crate::user::User {
                    id: format!("anon_{}", uuid::Uuid::new_v4()),
                    username: format!("匿名用户_{}", rand::rng().random_range(1000..9999)),
                    nickname: Some(format!("匿名用户_{}", rand::rng().random_range(1000..9999))),
                    avatar: None,
                    created_at: chrono::Utc::now(),
                    last_login: chrono::Utc::now(),
                })
            }
        }
    } else {
        // 没有会话ID，创建匿名用户
        debug!("没有会话ID，使用匿名用户");
        Some(crate::user::User {
            id: format!("anon_{}", uuid::Uuid::new_v4()),
            username: format!("匿名用户_{}", rand::rng().random_range(1000..9999)),
            nickname: Some(format!("匿名用户_{}", rand::rng().random_range(1000..9999))),
            avatar: None,
            created_at: chrono::Utc::now(),
            last_login: chrono::Utc::now(),
        })
    };

    // 获取或创建房间
    let room = if rooms.contains_key(&room_id) {
        debug!("使用已存在的房间");
        Arc::new(rooms.get(&room_id).unwrap().clone())
    } else {
        debug!("创建新房间");
        let config = crate::config::Config::get();
        let new_room = Arc::new(Room::new(
            room_id.clone(),
            config.game.min_players,
            config.game.max_players,
            word_bank.clone(),
            storage.clone(),
        ));
        rooms.insert(room_id.clone(), new_room.as_ref().clone());

        // 为新房间启动定时器来检查游戏状态超时
        let room_arc = Arc::clone(&new_room);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(config.ping_interval());
            loop {
                interval.tick().await;
                if let Err(e) = room_arc.check_timeout().await {
                    error!("检查超时失败: {}", e);
                    break; // 如果房间出错，停止定时器
                }
            }
        });

        new_room
    };

    // 创建消息通道
    let (_tx, mut rx) = tokio::sync::mpsc::channel::<GameMessage>(100);
    let ws_sender = Arc::new(tokio::sync::Mutex::new(ws_sender));

    // 启动消息接收循环
    let ws_sender_clone = ws_sender.clone();
    tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            debug!("准备发送消息: {:?}", message);
            if let Ok(text) = serde_json::to_string(&message) {
                debug!("消息序列化成功: {}", text);
                let mut sender = ws_sender_clone.lock().await;
                match sender.send(Message::Text(text)).await {
                    Ok(_) => debug!("消息发送成功"),
                    Err(e) => {
                        error!("发送消息失败: {}", e);
                        break;
                    }
                }
            } else {
                error!("消息序列化失败");
            }
        }
    });

    // 发送用户信息
    if let Some(user) = &user {
        let user_info_msg = GameMessage {
            type_: "user_info".to_string(),
            data: serde_json::json!({
                "user_id": user.id,
                "username": user.username,
                "nickname": user.nickname,
                "avatar": user.avatar
            }),
        };
        if let Ok(text) = serde_json::to_string(&user_info_msg) {
            match ws_sender.lock().await.send(Message::Text(text)).await {
                Ok(_) => debug!("用户信息发送成功"),
                Err(e) => error!("发送用户信息失败: {}", e),
            }
        }
    }

    // 发送房间列表
    debug!("准备发送房间列表");
    let room_list = rooms
        .iter()
        .map(|entry| {
            let id = entry.key();
            let player_count = entry.value().player_count();
            debug!("房间信息 - ID: {}, 玩家数: {}", id, player_count);
            serde_json::json!({ "id": id, "player_count": player_count })
        })
        .collect::<Vec<_>>();

    let room_list_msg = GameMessage {
        type_: "room_list".to_string(),
        data: serde_json::json!({ "rooms": room_list }),
    };
    debug!("房间列表消息: {:?}", room_list_msg);
    if let Ok(text) = serde_json::to_string(&room_list_msg) {
        debug!("房间列表序列化成功: {}", text);
        match ws_sender.lock().await.send(Message::Text(text)).await {
            Ok(_) => debug!("房间列表发送成功"),
            Err(e) => error!("发送房间列表失败: {}", e),
        }
    } else {
        error!("房间列表序列化失败");
    }

    // 处理WebSocket消息
    while let Some(msg) = ws_receiver.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                debug!("收到消息: {}", text);
                match serde_json::from_str::<GameMessage>(&text) {
                    Ok(message) => {
                        debug!("解析消息成功: {:?}", message);

                        // 如果是join消息，需要创建新的消息通道
                        if message.type_ == "join" {
                            debug!("处理join消息");
                            let (player_tx, mut player_rx) =
                                tokio::sync::mpsc::channel::<GameMessage>(100);
                            let ws_sender_clone = ws_sender.clone();

                            // 启动一个任务来处理从房间接收到的消息
                            tokio::spawn(async move {
                                while let Some(message) = player_rx.recv().await {
                                    debug!("从房间收到消息: {:?}", message);
                                    if let Ok(text) = serde_json::to_string(&message) {
                                        let mut sender = ws_sender_clone.lock().await;
                                        if let Err(e) = sender.send(Message::Text(text)).await {
                                            error!("发送消息到WebSocket失败: {}", e);
                                            break;
                                        }
                                    }
                                }
                            });

                            // 修改join消息，使用摸鱼派用户的昵称和ID
                            let mut modified_message = message.clone();
                            if let Some(user) = &user {
                                // 使用摸鱼派用户的昵称作为玩家名称，如果没有昵称则使用用户名
                                let player_name = user.nickname.as_ref().unwrap_or(&user.username);
                                modified_message.data["player_name"] =
                                    serde_json::Value::String(player_name.clone());
                                // 使用摸鱼派用户ID作为玩家ID
                                modified_message.data["player_id"] =
                                    serde_json::Value::String(user.id.clone());
                            }

                            // 将player_tx传递给房间
                            if let Err(e) =
                                room.handle_message(modified_message, Some(player_tx)).await
                            {
                                error!("处理消息失败: {}", e);
                                let error = GameMessage {
                                    type_: "error".to_string(),
                                    data: serde_json::json!({
                                        "code": "InternalError",
                                        "message": e.to_string()
                                    }),
                                };
                                if let Ok(text) = serde_json::to_string(&error) {
                                    debug!("发送错误消息: {}", text);
                                    match ws_sender.lock().await.send(Message::Text(text)).await {
                                        Ok(_) => debug!("错误消息发送成功"),
                                        Err(e) => error!("发送错误消息失败: {}", e),
                                    }
                                }
                            }
                        } else {
                            debug!("处理其他类型消息");
                            if let Err(e) = room.handle_message(message, None).await {
                                error!("处理消息失败: {}", e);
                                let error = GameMessage {
                                    type_: "error".to_string(),
                                    data: serde_json::json!({
                                        "code": "InternalError",
                                        "message": e.to_string()
                                    }),
                                };
                                if let Ok(text) = serde_json::to_string(&error) {
                                    debug!("发送错误消息: {}", text);
                                    match ws_sender.lock().await.send(Message::Text(text)).await {
                                        Ok(_) => debug!("错误消息发送成功"),
                                        Err(e) => error!("发送错误消息失败: {}", e),
                                    }
                                }
                            }
                        }
                    }
                    Err(e) => {
                        error!("解析消息失败: {}", e);
                        let error = GameMessage {
                            type_: "error".to_string(),
                            data: serde_json::json!({
                                "code": "InvalidAction",
                                "message": format!("无效的消息格式: {}", e)
                            }),
                        };
                        if let Ok(text) = serde_json::to_string(&error) {
                            debug!("发送错误消息: {}", text);
                            match ws_sender.lock().await.send(Message::Text(text)).await {
                                Ok(_) => debug!("错误消息发送成功"),
                                Err(e) => error!("发送错误消息失败: {}", e),
                            }
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                debug!("WebSocket连接关闭");
                break;
            }
            Ok(Message::Ping(data)) => {
                debug!("收到Ping消息");
                match ws_sender.lock().await.send(Message::Pong(data)).await {
                    Ok(_) => debug!("Pong消息发送成功"),
                    Err(e) => error!("发送Pong消息失败: {}", e),
                }
            }
            _ => {
                debug!("收到其他类型消息");
            }
        }
    }
}

/// 生成随机6个字母的房间ID
fn generate_random_room_id() -> String {
    const CHARSET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut rng = rand::rng();

    (0..6)
        .map(|_| {
            let idx = rng.random_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}
