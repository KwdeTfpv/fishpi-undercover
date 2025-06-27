# 谁是卧底游戏 - 后端服务

## 概述

这是一个基于Rust和Axum框架开发的"谁是卧底"游戏后端服务。支持摸鱼派账号登录，提供WebSocket实时游戏功能。

## 新特性：HTTP和WebSocket分离配置

### 配置说明

现在支持将HTTP服务器和WebSocket服务器分别配置在不同的端口上：

- **HTTP服务器**：处理认证回调、会话验证、登录等HTTP请求
- **WebSocket服务器**：处理游戏实时通信

### 配置文件

在 `config.toml` 中配置：

```toml
[server]
host = "0.0.0.0"
port = 8989
workers = 4
# HTTP服务器端口（用于认证回调等），如果为None则使用port
http_port = 8989
# WebSocket服务器端口，如果为None则使用port
ws_port = 8990
```

### 使用场景

1. **开发环境**：HTTP和WebSocket使用相同端口
   ```toml
   http_port = 8989
   ws_port = 8989
   ```

2. **生产环境**：HTTP使用HTTPS端口，WebSocket使用WS端口
   ```toml
   http_port = 443  # HTTPS端口
   ws_port = 8990   # WebSocket端口
   ```

3. **反向代理**：HTTP和WebSocket都通过Nginx代理
   ```toml
   http_port = 8080  # 内部HTTP端口
   ws_port = 8990    # 内部WebSocket端口
   ```

### 前端配置

前端会自动从 `/config/websocket` API获取WebSocket服务器配置，无需手动配置。

## 启动方式

### 1. 分离模式（推荐）

```bash
cargo run
```

程序会自动启动两个服务器：
- HTTP服务器：处理认证和静态文件
- WebSocket服务器：处理游戏通信

### 2. 兼容模式

如果配置文件中没有设置 `http_port` 和 `ws_port`，程序会使用传统的单服务器模式。

## 网络架构

```
客户端
├── HTTP请求 (认证、回调等)
│   └── HTTP服务器 (端口: http_port)
└── WebSocket连接 (游戏通信)
    └── WebSocket服务器 (端口: ws_port)
```

## 认证流程

1. 用户访问前端页面
2. 点击登录，前端请求 `/auth/login`
3. 后端返回摸鱼派登录URL
4. 用户跳转到摸鱼派进行认证
5. 摸鱼派回调到 `/auth/callback`（HTTP服务器）
6. 后端验证并创建会话
7. 用户跳转回游戏页面
8. 前端连接WebSocket服务器进行游戏

## 配置示例

### 开发环境配置

```toml
[server]
host = "127.0.0.1"
port = 8080
http_port = 8080
ws_port = 8080
```

### 生产环境配置

```toml
[server]
host = "0.0.0.0"
port = 8989
http_port = 443  # HTTPS端口
ws_port = 8990   # WebSocket端口

[auth]
domain = "https://your-domain.com"
```

### Nginx反向代理配置

```nginx
# HTTP服务器（认证回调）
server {
    listen 443 ssl;
    server_name your-domain.com;
    
    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
    }
}

# WebSocket服务器（直连）
server {
    listen 8990;
    server_name your-domain.com;
    
    location /ws {
        proxy_pass http://127.0.0.1:8990;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
    }
}
```

## 注意事项

1. **CORS配置**：确保前端域名在认证回调的允许列表中
2. **防火墙**：确保WebSocket端口对外开放
3. **SSL证书**：生产环境建议为WebSocket也配置SSL证书
4. **负载均衡**：WebSocket连接不支持负载均衡，需要保持会话粘性

## 故障排除

### WebSocket连接失败

1. 检查WebSocket端口是否开放
2. 检查防火墙设置
3. 查看后端日志确认WebSocket服务器启动成功

### 认证回调失败

1. 检查HTTP服务器端口配置
2. 确认域名配置正确
3. 检查SSL证书（如果使用HTTPS）


## 项目简介
本项目为"谁是卧底"游戏的后端服务，基于 Rust 语言开发，支持多种前端集成方式，适合自部署和二次开发。

## 主要功能
- 游戏房间管理
- 玩家身份分配与回合控制
- 游戏流程自动推进
- API 文档详尽（见 API.md）

## 快速开始
1. 克隆仓库：
   ```bash
   git clone <your-repo-url>
   ```
2. 进入项目目录，安装 Rust 环境。
3. 复制配置文件模板并根据实际情况修改：
   ```bash
   cp config.example.toml config.toml
   ```
4. 编译并运行：
   ```bash
   cargo run --release --bin fishpi-undercover
   ```

## 配置说明
- `config.toml`：主配置文件，包含端口、数据库等信息。
- 示例配置文件已提供（*.example.*），请勿上传真实配置。

## 注意事项
- 前端可通过 API 进行集成，详见 `API.md`。

## 目录结构
- `src/`：核心后端代码
- `data/`：数据文件
- `docs/`：文档与流程图
- `index.html`：示例前端页面

---
如有问题欢迎提 Issue 或 PR！

# 摸鱼派卧底游戏 - 房间管理系统

## 新的房间删除和心跳功能设计

### 设计原则

1. **不使用Clone**: 避免不必要的房间克隆操作
2. **自动生命周期管理**: 房间自动管理自己的生命周期
3. **回调机制**: 使用回调函数通知网络层房间删除事件
4. **活动跟踪**: 跟踪房间活动时间，自动清理空闲房间

### 核心组件

#### Room 结构
```rust
pub struct Room {
    id: String,
    state: Arc<RwLock<GameState>>,
    players: Arc<DashMap<PlayerId, Player>>,
    // ... 其他字段
    last_activity: Arc<Mutex<chrono::DateTime<Utc>>>,
    delete_callback: Option<Arc<RoomDeleteCallback>>,
    heartbeat_interval: Duration,
    max_idle_time: Duration,
}
```

#### 主要功能

1. **活动跟踪**
   - `update_activity()`: 更新最后活动时间
   - `should_be_deleted()`: 检查房间是否应该被删除

2. **生命周期管理**
   - `start_lifecycle_management()`: 启动心跳和超时检查
   - `delete()`: 执行房间删除操作

3. **回调机制**
   - `set_delete_callback()`: 设置删除回调函数
   - 当房间需要删除时，自动调用回调通知网络层

### 使用示例

```rust
// 创建房间
let mut room = Room::new(room_id, min_players, max_players, word_bank, storage, host_id);

// 设置删除回调
let rooms_clone = rooms.clone();
room.set_delete_callback(Box::new(move |room_id: String| {
    let rooms = rooms_clone.clone();
    tokio::spawn(async move {
        info!("删除房间: {}", room_id);
        rooms.remove(&room_id);
    });
}));

// 启动生命周期管理
room.start_lifecycle_management().await;

// 在WebSocket连接中更新活动时间
room.update_activity().await;
```

### 优势

1. **内存效率**: 避免不必要的克隆，减少内存使用
2. **自动管理**: 房间自动管理自己的生命周期，无需外部干预
3. **响应式**: 基于活动时间的自动清理机制
4. **可扩展**: 回调机制允许灵活的自定义删除逻辑

### 配置参数

- `heartbeat_interval`: 心跳检查间隔（默认从配置文件读取）
- `max_idle_time`: 最大空闲时间（默认5分钟）
- 房间为空或游戏结束后，超过空闲时间自动删除 