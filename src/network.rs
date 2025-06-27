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
    routing::{get, post},
};
use dashmap::DashMap;
use futures_util::{SinkExt, StreamExt};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use tracing::{debug, error};
use uuid::Uuid;
use tower_http::cors::{CorsLayer, Any};
use urlencoding;
use crate::game::PlayerId;
use tokio::sync::mpsc;

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
    // 自定义重定向参数
    callback_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct ValidateQuery {
    session_id: String,
}

#[derive(Debug, Deserialize)]
struct LoginQuery {
    callback_url: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CreateRoomQuery {
    session_id: String,
    room_id: Option<String>, // 可选的房间ID，如果不提供则自动生成
}

#[derive(Debug, Deserialize)]
struct AdminQuery {
    session_id: String,
}

#[derive(Debug, Serialize)]
struct ValidateResponse {
    success: bool,
    user: Option<crate::user::User>,
    message: Option<String>,
}

#[derive(Debug, Serialize)]
struct CreateRoomResponse {
    success: bool,
    room_id: Option<String>,
    message: Option<String>,
}

/// WebSocket服务器，负责处理网络连接和消息传输
pub struct WebSocketServer {
    rooms: Arc<DashMap<String, Arc<Room>>>,
    word_bank: Arc<WordBank>,
    storage: Arc<Storage>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>, // 添加用户管理器
    connection_manager: Arc<ConnectionManager>, // 添加连接管理器
}

/// 全局连接管理器，用于跟踪每个玩家的WebSocket连接
pub struct ConnectionManager {
    /// 玩家ID -> (房间ID, 连接发送器) 的映射
    player_connections: Arc<DashMap<PlayerId, (String, mpsc::Sender<GameMessage>)>>,
}

impl ConnectionManager {
    pub fn new() -> Self {
        Self {
            player_connections: Arc::new(DashMap::new()),
        }
    }

    /// 注册玩家的连接
    pub async fn register_connection(
        &self,
        player_id: PlayerId,
        room_id: String,
        tx: mpsc::Sender<GameMessage>,
    ) -> Option<(String, mpsc::Sender<GameMessage>)> {
        // 如果玩家已有连接，返回旧连接信息
        let old_connection = self.player_connections.insert(player_id.clone(), (room_id, tx));
        old_connection
    }

    /// 移除玩家的连接
    pub async fn remove_connection(&self, player_id: &PlayerId) {
        self.player_connections.remove(player_id);
    }

    /// 获取玩家的当前连接信息
    pub async fn get_connection(&self, player_id: &PlayerId) -> Option<(String, mpsc::Sender<GameMessage>)> {
        self.player_connections.get(player_id).map(|entry| entry.value().clone())
    }
}

impl WebSocketServer {
    pub async fn new() -> Self {
        let config = crate::config::Config::get();
        let storage = Arc::new(
            Storage::new(&config.redis.url)
                .await
                .expect("Failed to create storage"),
        );

        // 创建UserManager实例，传入Storage
        let user_manager = UserManager::new(Storage::new(&config.redis.url).await.expect("Failed to create storage"));

        WebSocketServer {
            rooms: Arc::new(DashMap::new()),
            word_bank: Arc::new(WordBank::new()),
            storage,
            user_manager: Arc::new(tokio::sync::RwLock::new(user_manager)),
            connection_manager: Arc::new(ConnectionManager::new()),
        }
    }

    /// 启动HTTP服务器（用于认证回调等）
    pub async fn start_http_server(&self, http_addr: &str) -> Result<()> {
        let user_manager = self.user_manager.clone();
        let config = crate::config::Config::get();

        // 根据配置文件设置CORS
        let cors = if config.cors.allow_all_origins.unwrap_or(true) {
            // 开发环境：允许所有来源，不发送凭证
            debug!("CORS配置: 开发环境 - 允许所有来源");
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_credentials(false)
        } else if let Some(allowed_origins) = &config.cors.allowed_origins {
            if allowed_origins.is_empty() {
                // 生产环境但没有配置允许的来源，默认允许所有来源
                debug!("CORS配置: 生产环境 - 没有设置允许的来源，默认允许所有来源");
                CorsLayer::new()
                    .allow_origin(Any)
                    .allow_methods(Any)
                    .allow_credentials(false)
            } else {
                // 生产环境：限制特定来源，允许凭证
                debug!("CORS配置: 生产环境 - 限制特定来源");
                let origins = allowed_origins
                    .iter()
                    .filter_map(|origin| origin.parse::<axum::http::HeaderValue>().ok())
                    .collect::<Vec<_>>();
                
                debug!("CORS允许的来源: {:?}", origins);
                CorsLayer::new()
                    .allow_origin(origins)
                    .allow_methods([
                        axum::http::Method::GET,
                        axum::http::Method::POST,
                        axum::http::Method::PUT,
                        axum::http::Method::DELETE,
                        axum::http::Method::OPTIONS,
                    ])
                    .allow_credentials(true)
            }
        } else {
            // 生产环境但没有配置，默认允许所有来源
            debug!("CORS配置: 生产环境 - 没有配置，默认允许所有来源");
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_credentials(false)
        };

        let http_app = Router::new()
            .route("/", get(serve_index))
            .route("/index.html", get(serve_index))
            .route("/admin.html", get(serve_admin))
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
                    move |Query(query): Query<LoginQuery>| async move { 
                        handle_generate_login_url(query, user_manager.clone()).await 
                    }
                }),
            )
            .route(
                "/rooms/create",
                get({
                    let rooms = self.rooms.clone();
                    let word_bank = self.word_bank.clone();
                    let storage = self.storage.clone();
                    let user_manager = self.user_manager.clone();
                    move |Query(query): Query<CreateRoomQuery>| async move {
                        handle_create_room(query, rooms.clone(), word_bank.clone(), storage.clone(), user_manager.clone()).await
                    }
                }),
            )
            .route(
                "/rooms/status",
                get({
                    let rooms = self.rooms.clone();
                    move || async move { handle_rooms_status(rooms.clone()).await }
                }),
            )
            .route(
                "/admin/rooms",
                get({
                    let rooms = self.rooms.clone();
                    let user_manager = self.user_manager.clone();
                    move |Query(query): Query<AdminQuery>| async move {
                        handle_admin_rooms(query, rooms.clone(), user_manager.clone()).await
                    }
                }),
            )
            .route(
                "/admin/rooms/:room_id/delete",
                post({
                    let rooms = self.rooms.clone();
                    let user_manager = self.user_manager.clone();
                    move |axum::extract::Path(room_id): axum::extract::Path<String>, Query(query): Query<AdminQuery>| async move {
                        handle_admin_delete_room(room_id, query, rooms.clone(), user_manager.clone()).await
                    }
                }),
            )
            .layer(cors);

        let http_listener = tokio::net::TcpListener::bind(http_addr)
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;
        
        // 启动HTTP服务器
        tokio::spawn(async move {
            axum::serve(http_listener, http_app)
                .await
                .map_err(|e| {
                    error!("HTTP服务器错误: {}", e);
                    crate::Error::Network(anyhow::anyhow!(e))
                })
        });

        Ok(())
    }

    /// 启动WebSocket服务器
    pub async fn start_ws_server(&self, ws_addr: &str) -> Result<()> {
        let rooms = self.rooms.clone();
        // let word_bank = self.word_bank.clone();
        // let storage = self.storage.clone();
        let user_manager = self.user_manager.clone();
        let connection_manager = self.connection_manager.clone();

        let ws_app = Router::new()
            .route(
                "/ws",
                get({
                    let user_manager = user_manager.clone();
                    let connection_manager = connection_manager.clone();
                    move |ws: WebSocketUpgrade, Query(query): Query<RoomQuery>| async move {
                        // 必须提供room_id，不再自动生成
                        let room_id = match query.room_id {
                            Some(id) => id,
                            None => {
                                // 如果没有提供room_id，返回错误
                                return ws.on_upgrade(|_| async {
                                    // 这里应该发送错误消息，但WebSocket升级后无法直接返回HTTP错误
                                    // 所以我们在连接处理中处理这个错误
                                });
                            }
                        };
                        
                        let session_id = query.session_id.and_then(|id| {
                            match Uuid::parse_str(&id) {
                                Ok(uuid) => {
                                    debug!("解析会话ID成功: {}", uuid);
                                    Some(uuid)
                                }
                                Err(e) => {
                                    error!("解析会话ID失败: {}, 原始值: {}", e, id);
                                    None
                                }
                            }
                        });
                        
                        debug!(
                            "WebSocket连接请求详情 - 房间ID: {}, 会话ID: {:?}",
                            room_id, session_id
                        );

                        // 添加CORS和WebSocket升级头
                        ws.on_upgrade(move |socket| async move {
                            debug!("WebSocket连接已升级，开始处理连接");
                            handle_room_connection(
                                socket,
                                room_id,
                                session_id,
                                rooms.clone(),
                                user_manager.clone(),
                                connection_manager.clone(),
                            )
                            .await;
                            debug!("WebSocket连接处理完成");
                        })
                    }
                }),
            )
            // 添加OPTIONS路由处理预检请求
            .route(
                "/ws",
                axum::routing::options(|| async {
                    axum::response::Response::builder()
                        .header("Access-Control-Allow-Origin", "*")
                        .header("Access-Control-Allow-Methods", "GET, POST, OPTIONS")
                        .header("Access-Control-Allow-Headers", "Content-Type, Authorization, Upgrade, Connection, Sec-WebSocket-Key, Sec-WebSocket-Version, Sec-WebSocket-Protocol")
                        .header("Access-Control-Allow-Credentials", "true")
                        .status(200)
                        .body(axum::body::Body::empty())
                        .unwrap()
                }),
            );

        let ws_listener = tokio::net::TcpListener::bind(ws_addr)
            .await
            .map_err(|e| {
                error!("绑定WebSocket地址失败: {} - {}", ws_addr, e);
                crate::Error::Network(anyhow::anyhow!(e))
            })?;
        
        axum::serve(ws_listener, ws_app)
            .await
            .map_err(|e| {
                error!("WebSocket服务器运行错误: {}", e);
                crate::Error::Network(anyhow::anyhow!(e))
            })?;
        Ok(())
    }

    /// 启动服务器（兼容旧接口）
    pub async fn start(&self, addr: &str) -> Result<()> {
        // 默认情况下，HTTP和WebSocket使用相同地址
        self.start_http_server(addr).await?;
        self.start_ws_server(addr).await?;
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
        let user_manager = self.user_manager.read().await;
        user_manager.handle_login(&params).await
    }
}

/// 处理认证回调
async fn handle_auth_callback(
    query: AuthCallbackQuery,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Html<String> {
    // 使用迭代器来简化参数转换
    let param_mappings = [
        ("openid.ns", &query.openid_ns),
        ("openid.mode", &query.openid_mode),
        ("openid.op_endpoint", &query.openid_op_endpoint),
        ("openid.claimed_id", &query.openid_claimed_id),
        ("openid.identity", &query.openid_identity),
        ("openid.return_to", &query.openid_return_to),
        ("openid.response_nonce", &query.openid_response_nonce),
        ("openid.assoc_handle", &query.openid_assoc_handle),
        ("openid.signed", &query.openid_signed),
        ("openid.sig", &query.openid_sig),
    ];

    let params: HashMap<String, String> = param_mappings
        .iter()
        .filter_map(|(key, value)| value.as_ref().map(|v| (key.to_string(), v.clone())))
        .collect();

    let user_manager_guard = user_manager.read().await;
    match user_manager_guard.handle_login(&params).await {
        Ok((session_id, user)) => {
            // 获取重定向地址，优先使用callback_url参数
            let redirect_url = if let Some(callback_url) = &query.callback_url {
                callback_url.clone()
            } else {
                // 默认重定向到首页
                "/index.html".to_string()
            };

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
    <p>正在跳转...</p>
    <script>
        // 保存session_id到localStorage
        localStorage.setItem('fishpi_session_id', '{}');
        // 跳转到指定页面，并传递session_id参数
        const redirectUrl = '{}';
        const separator = redirectUrl.includes('?') ? '&' : '?';
        window.location.href = redirectUrl + separator + 'session_id={}';
    </script>
</body>
</html>
"#,
                display_name, session_id, redirect_url, session_id
            );
            Html(html)
        }
        Err(e) => {
            // 登录失败时，也使用callback_url参数决定重定向地址
            let redirect_url = query.callback_url.unwrap_or_else(|| "/index.html".to_string());

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
    <p><a href="{}">返回</a></p>
</body>
</html>
"#,
                e, redirect_url
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

    let user_manager_guard = user_manager.read().await;
    match user_manager_guard.get_user_by_session(&session_id).await {
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
    query: LoginQuery,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Json<serde_json::Value> {
    let user_manager_guard = user_manager.read().await;
    let config = crate::config::Config::get();

    // 构建return_to URL，如果提供了callback_url则编码到参数中
    let base_return_to = format!("{}/auth/callback", config.auth.domain);
    let return_to = if let Some(callback_url) = query.callback_url {
        let separator = if base_return_to.contains('?') { "&" } else { "?" };
        format!("{}{}callback_url={}", base_return_to, separator, urlencoding::encode(&callback_url))
    } else {
        base_return_to
    };
    
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

/// 处理房间状态查询
async fn handle_rooms_status(
    rooms: Arc<DashMap<String, Arc<Room>>>,
) -> Json<serde_json::Value> {
    let mut room_statuses = Vec::new();
    
    for entry in rooms.iter() {
        let room_id = entry.key();
        let room = entry.value();
        
        // 检查房间是否应该被删除
        let should_be_deleted = room.should_be_deleted().await;
        
        // 只返回不应该被删除的房间
        if !should_be_deleted {
            let (player_count, idle_seconds, is_game_over, is_empty) = room.get_status().await;
            
            room_statuses.push(serde_json::json!({
                "room_id": room_id,
                "player_count": player_count,
                "idle_seconds": idle_seconds,
                "is_game_over": is_game_over,
                "is_empty": is_empty,
                "should_be_deleted": false
            }));
        }
    }
    
    Json(serde_json::json!({
        "success": true,
        "rooms": room_statuses,
        "total_rooms": room_statuses.len()
    }))
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

/// 提供admin.html文件
async fn serve_admin() -> Html<String> {
    let admin_path = Path::new("rooms.html");
    match fs::read_to_string(admin_path) {
        Ok(content) => Html(content),
        Err(e) => {
            error!("读取admin.html失败: {}", e);
            Html(format!("<h1>404 Not Found</h1><p>rooms.html文件</p>"))
        }
    }
}

/// 处理WebSocket连接
async fn handle_room_connection(
    socket: WebSocket,
    room_id: String,
    session_id: Option<Uuid>,
    rooms: Arc<DashMap<String, Arc<Room>>>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
    connection_manager: Arc<ConnectionManager>,
) {
    debug!(
        "开始处理WebSocket连接，房间ID: {}, 会话ID: {:?}",
        room_id, session_id
    );
    let (mut ws_sender, mut ws_receiver) = socket.split();

    // 验证用户会话（必需）
    let user = if let Some(session_id) = session_id {
        let user_manager_guard = user_manager.read().await;
        match user_manager_guard.get_user_by_session(&session_id).await {
            Ok(user) => {
                let display_name: String = user.nickname.as_ref().unwrap_or(&user.username).clone();
                debug!("用户已登录: {} ({})", display_name, user.username);
                Some(user.clone())
            }
            Err(e) => {
                error!("会话验证失败: {}", e);
                // 发送错误消息并关闭连接
                let error_msg = GameMessage {
                    type_: "error".to_string(),
                    data: serde_json::json!({
                        "code": "AuthError",
                        "message": "请先登录"
                    }),
                };
                if let Ok(text) = serde_json::to_string(&error_msg) {
                    let _ = ws_sender.send(Message::Text(text)).await;
                }
                return; // 关闭连接
            }
        }
    } else {
        // 没有会话ID，发送错误消息并关闭连接
        let error_msg = GameMessage {
            type_: "error".to_string(),
            data: serde_json::json!({
                "code": "AuthRequired",
                "message": "需要登录才能进入游戏"
            }),
        };
        if let Ok(text) = serde_json::to_string(&error_msg) {
            let _ = ws_sender.send(Message::Text(text)).await;
        }
        return; // 关闭连接
    };

    // 获取房间（必须已存在）
    let room = if let Some(room_entry) = rooms.get(&room_id) {
        debug!("连接到已存在的房间: {}", room_id);
        room_entry.value().clone()
    } else {
        // 房间不存在，发送错误消息并关闭连接
        let error_msg = GameMessage {
            type_: "error".to_string(),
            data: serde_json::json!({
                "code": "RoomNotFound",
                "message": format!("房间 {} 不存在，请先创建房间", room_id)
            }),
        };
        if let Ok(text) = serde_json::to_string(&error_msg) {
            let _ = ws_sender.send(Message::Text(text)).await;
        }
        return; // 关闭连接
    };

    // 检查房间是否已被删除
    if room.is_deleted().await {
        let error_msg = GameMessage {
            type_: "error".to_string(),
            data: serde_json::json!({
                "message": format!("房间 {} 不存在", room_id)
            }),
        };
        if let Ok(text) = serde_json::to_string(&error_msg) {
            let _ = ws_sender.send(Message::Text(text)).await;
        }
        return; // 关闭连接
    }

    // 更新房间活动时间
    room.update_activity().await;

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
        debug!("准备发送用户信息: {:?}", user);
        let user_info_msg = GameMessage {
            type_: "user_info".to_string(),
            data: serde_json::json!({
                "user_id": user.id,
                "username": user.username,
                "nickname": user.nickname,
                "avatar": user.avatar
            }),
        };
        debug!("用户信息消息: {:?}", user_info_msg);
        if let Ok(text) = serde_json::to_string(&user_info_msg) {
            debug!("用户信息序列化成功: {}", text);
            match ws_sender.lock().await.send(Message::Text(text)).await {
                Ok(_) => debug!("用户信息发送成功"),
                Err(e) => error!("发送用户信息失败: {}", e),
            }
        } else {
            error!("用户信息序列化失败");
        }
    } else {
        error!("用户信息为空，无法发送user_info消息");
    }

    // 处理WebSocket消息
    while let Some(msg) = ws_receiver.next().await {
        // 更新房间活动时间
        room.update_activity().await;
        
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

                            // 注册玩家连接
                            if let Some(user) = &user {
                                let player_id = user.id.clone();
                                connection_manager.register_connection(
                                    player_id.clone(),
                                    room_id.clone(),
                                    player_tx.clone(),
                                ).await;
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
                            // 处理其他消息
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
                                "code": "ParseError",
                                "message": "消息格式错误"
                            }),
                        };
                        if let Ok(text) = serde_json::to_string(&error) {
                            if let Err(e) = ws_sender.lock().await.send(Message::Text(text)).await {
                                error!("发送错误消息失败: {}", e);
                            }
                        }
                    }
                }
            }
            Ok(Message::Close(_)) => {
                debug!("收到关闭消息");
                break;
            }
            Ok(Message::Ping(data)) => {
                debug!("收到ping消息");
                if let Err(e) = ws_sender.lock().await.send(Message::Pong(data)).await {
                    error!("发送pong消息失败: {}", e);
                }
            }
            Ok(Message::Pong(_)) => {
                debug!("收到pong消息");
            }
            Ok(Message::Binary(_)) => {
                debug!("收到二进制消息，忽略");
            }
            Err(e) => {
                error!("WebSocket错误: {}", e);
                break;
            }
        }
    }

    // 连接关闭时，移除玩家连接记录
    if let Some(user) = &user {
        connection_manager.remove_connection(&user.id).await;
    }

    debug!("WebSocket连接关闭");
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

/// 处理创建房间请求
async fn handle_create_room(
    query: CreateRoomQuery,
    rooms: Arc<DashMap<String, Arc<Room>>>,
    word_bank: Arc<WordBank>,
    storage: Arc<Storage>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Json<CreateRoomResponse> {
    // 验证用户会话
    let session_id = match Uuid::parse_str(&query.session_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(CreateRoomResponse {
                success: false,
                room_id: None,
                message: Some("无效的会话ID格式".to_string()),
            });
        }
    };

    let user_manager_guard = user_manager.read().await;
    let user = match user_manager_guard.get_user_by_session(&session_id).await {
        Ok(user) => user,
        Err(e) => {
            return Json(CreateRoomResponse {
                success: false,
                room_id: None,
                message: Some(format!("会话验证失败: {}", e)),
            });
        }
    };

    // 生成房间ID
    let room_id = if let Some(custom_id) = query.room_id {
        // 验证自定义房间ID
        if custom_id.is_empty() {
            return Json(CreateRoomResponse {
                success: false,
                room_id: None,
                message: Some("房间ID不能为空".to_string()),
            });
        }
        
        // 限制房间ID长度（1-20个字符）
        if custom_id.len() > 20 {
            return Json(CreateRoomResponse {
                success: false,
                room_id: None,
                message: Some("房间ID长度不能超过20个字符".to_string()),
            });
        }
        
        // 检查房间ID是否包含非法字符（只允许字母、数字、下划线、连字符）
        if !custom_id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
            return Json(CreateRoomResponse {
                success: false,
                room_id: None,
                message: Some("房间ID只能包含字母、数字、下划线和连字符".to_string()),
            });
        }
        
        custom_id
    } else {
        generate_random_room_id()
    };
    
    // 检查房间ID是否已存在（理论上不应该，但为了安全）
    if rooms.contains_key(&room_id) {
        return Json(CreateRoomResponse {
            success: false,
            room_id: None,
            message: Some("房间ID冲突，请重试".to_string()),
        });
    }

    // 创建新房间
    let config = crate::config::Config::get();
    let mut new_room = Room::new(
        room_id.clone(),
        config.game.min_players,
        config.game.max_players,
        word_bank.clone(),
        storage.clone(),
        user.id.clone(), // 使用创建者的用户ID作为房主
    );

    // 设置房间删除回调
    let rooms_clone = rooms.clone();
    new_room.set_delete_callback(Box::new(move |id: String| {
        let rooms = rooms_clone.clone();
        tokio::spawn(async move {
            debug!("执行房间删除回调，删除房间: {}", id);
            let removed = rooms.remove(&id);
            if removed.is_some() {
                debug!("房间 {} 已从房间映射中删除", id);
            } else {
                error!("房间 {} 删除失败，房间不存在", id);
            }
        });
    }));

    // 设置跨房间玩家踢出回调
    let rooms_clone_for_kick = rooms.clone();
    new_room.set_player_kick_callback(Box::new(move |player_id: String, other_room_id: String| {
        let rooms = rooms_clone_for_kick.clone();
        tokio::spawn(async move {
            debug!("执行跨房间玩家踢出回调，玩家: {}, 从房间: {}", player_id, other_room_id);
            if let Some(room_entry) = rooms.get(&other_room_id) {
                let room = room_entry.value();
                if let Err(e) = room.kick_player_from_other_room(player_id).await {
                    error!("从房间 {} 踢出玩家失败: {}", other_room_id, e);
                }
            } else {
                debug!("房间 {} 不存在，无需踢出玩家", other_room_id);
            }
        });
    }));

    // 将房间包装在Arc中
    let room_arc = Arc::new(new_room);
    
    // 启动房间的生命周期管理
    Arc::clone(&room_arc).start_lifecycle_management();

    // 将房间插入到全局房间映射中
    rooms.insert(room_id.clone(), Arc::clone(&room_arc));

    debug!("用户 {} 创建了房间: {}", user.username, room_id);

    Json(CreateRoomResponse {
        success: true,
        room_id: Some(room_id),
        message: None,
    })
}

/// 处理管理员查看房间列表请求
async fn handle_admin_rooms(
    query: AdminQuery,
    rooms: Arc<DashMap<String, Arc<Room>>>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Json<serde_json::Value> {
    // 验证用户会话
    let session_id = match Uuid::parse_str(&query.session_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "message": "无效的会话ID格式"
            }));
        }
    };

    let user_manager_guard = user_manager.read().await;
    let user = match user_manager_guard.get_user_by_session(&session_id).await {
        Ok(user) => user,
        Err(e) => {
            return Json(serde_json::json!({
                "success": false,
                "message": format!("会话验证失败: {}", e)
            }));
        }
    };

    // 检查是否为管理员
    let config = crate::config::Config::get();
    if !config.is_admin(&user.username) {
        return Json(serde_json::json!({
            "success": false,
            "message": "权限不足，需要管理员权限"
        }));
    }

    // 获取所有房间的详细信息
    let mut room_details = Vec::new();
    
    for entry in rooms.iter() {
        let room_id = entry.key();
        let room = entry.value();
        
        let (player_count, idle_seconds, is_game_over, is_empty) = room.get_status().await;
        let host = room.get_host().await;
        let is_deleted = room.is_deleted().await;
        
        room_details.push(serde_json::json!({
            "room_id": room_id,
            "player_count": player_count,
            "idle_seconds": idle_seconds,
            "is_game_over": is_game_over,
            "is_empty": is_empty,
            "is_deleted": is_deleted,
            "host": host,
            "should_be_deleted": room.should_be_deleted().await
        }));
    }

    Json(serde_json::json!({
        "success": true,
        "rooms": room_details,
        "total_rooms": room_details.len()
    }))
}

/// 处理管理员删除房间请求
async fn handle_admin_delete_room(
    room_id: String,
    query: AdminQuery,
    rooms: Arc<DashMap<String, Arc<Room>>>,
    user_manager: Arc<tokio::sync::RwLock<UserManager>>,
) -> Json<serde_json::Value> {
    // 验证用户会话
    let session_id = match Uuid::parse_str(&query.session_id) {
        Ok(id) => id,
        Err(_) => {
            return Json(serde_json::json!({
                "success": false,
                "message": "无效的会话ID格式"
            }));
        }
    };

    let user_manager_guard = user_manager.read().await;
    let user = match user_manager_guard.get_user_by_session(&session_id).await {
        Ok(user) => user,
        Err(e) => {
            return Json(serde_json::json!({
                "success": false,
                "message": format!("会话验证失败: {}", e)
            }));
        }
    };

    // 检查是否为管理员
    let config = crate::config::Config::get();
    if !config.is_admin(&user.username) {
        return Json(serde_json::json!({
            "success": false,
            "message": "权限不足，需要管理员权限"
        }));
    }

    // 检查房间是否存在
    let room = match rooms.get(&room_id) {
        Some(room_entry) => room_entry.value().clone(),
        None => {
            return Json(serde_json::json!({
                "success": false,
                "message": "房间不存在"
            }));
        }
    };

    // 强制删除房间
    room.delete().await;

    Json(serde_json::json!({
        "success": true,
        "message": format!("房间 {} 已被管理员 {} 强制删除", room_id, user.username)
    }))
}


