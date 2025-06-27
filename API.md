# 谁是卧底游戏 - API 接口文档

## 概述

这是一个基于WebSocket的实时多人游戏API，支持摸鱼派用户登录认证。游戏采用房间制，支持断线重连和状态持久化。

## 服务器信息

- **协议**: WebSocket + HTTP
- **认证方式**: 摸鱼派OpenID认证
- **数据存储**: Redis
- **架构**: HTTP和WebSocket服务器分离
  - **HTTP服务器**: 处理认证回调、会话验证、静态资源等
  - **WebSocket服务器**: 处理游戏实时通信
- **默认端口**: 
  - HTTP服务器: 8080
  - WebSocket服务器: 8990

## HTTP 接口

### 1. 用户认证

#### 认证流程概述

摸鱼派OpenID认证采用标准的OAuth2.0流程，具体步骤如下：

1. **前端获取登录URL** → 调用 `/auth/login` 接口
2. **跳转登录页面** → 用户跳转到摸鱼派登录页面
3. **用户登录** → 用户在摸鱼派完成登录
4. **自动回调** → 摸鱼派重定向到 `/auth/callback`
5. **服务器处理** → 服务器验证并创建session_id
6. **自动跳转** → 服务器返回页面，自动跳转到游戏

#### 1.1 获取登录URL
**接口**: `GET /auth/login`

**描述**: 获取摸鱼派登录URL

**查询参数**:
- `callback_url` (可选): 登录成功后的回调地址，如果不传则使用默认地址

**示例请求**:
```bash
# 使用默认回调地址
GET /auth/login

# 指定自定义回调地址
GET /auth/login?callback_url=http://localhost:3000/game
GET /auth/login?callback_url=https://your-app.com/dashboard
```

**响应格式**:
```json
{
    "success": true,
    "login_url": "https://fishpi.cn/openid/login?openid.ns=...&openid.return_to=..."
}
```

**说明**:
- `login_url`: 完整的摸鱼派登录URL，包含所有必要的OpenID参数
- 如果提供了 `callback_url`，它会被编码到 `return_to` 参数中
- 前端需要将用户重定向到此URL进行登录
- 登录URL包含回调地址，用户登录后会自动重定向回您的服务器

#### 1.2 认证回调
**接口**: `GET /auth/callback`

**描述**: 处理摸鱼派登录回调（服务器端自动处理）

**说明**: 
- 此接口由服务器自动处理，前端无需干预
- 摸鱼派认证完成后会自动重定向到此接口
- 服务器验证OpenID参数，创建session_id
- 返回HTML页面，自动保存session_id并跳转到指定页面

**OpenID参数**:
- `openid.ns`: OpenID命名空间 (固定值: `http://specs.openid.net/auth/2.0`)
- `openid.mode`: 认证模式 (固定值: `id_res`)
- `openid.op_endpoint`: 摸鱼派OpenID端点
- `openid.claimed_id`: 声明的用户ID
- `openid.identity`: 用户身份标识
- `openid.return_to`: 回调URL
- `openid.response_nonce`: 响应随机数
- `openid.assoc_handle`: 关联句柄
- `openid.signed`: 签名字段列表
- `openid.sig`: 签名值

**自定义参数**:
- `callback_url`: 登录成功后的重定向地址（从return_to参数中解析）

**服务器处理流程**:
1. 接收OpenID回调参数
2. 验证OpenID签名
3. 检查nonce防止重放攻击
4. 获取用户信息
5. 创建或更新用户会话
6. 生成session_id
7. 解析callback_url参数
8. 返回HTML页面并重定向到指定地址

**响应HTML页面**:
```html
<!DOCTYPE html>
<html>
<head>
    <meta charset="UTF-8">
    <title>登录成功</title>
</head>
<body>
    <h1>登录成功！</h1>
    <p>欢迎，用户昵称！</p>
    <p>正在跳转...</p>
    <script>
        // 保存session_id到localStorage
        localStorage.setItem('fishpi_session_id', '550e8400-e29b-41d4-a716-446655440000');
        // 跳转到指定页面，并传递session_id参数
        const redirectUrl = 'http://localhost:3000/game';
        const separator = redirectUrl.includes('?') ? '&' : '?';
        window.location.href = redirectUrl + separator + 'session_id=550e8400-e29b-41d4-a716-446655440000';
    </script>
</body>
</html>
```

**重定向逻辑**:
- 如果提供了 `callback_url` 参数，重定向到该地址
- 如果没有提供，重定向到默认的 `/index.html`
- session_id 会作为 URL 参数传递到重定向地址

#### 1.3 验证会话
**接口**: `GET /auth/validate`

**描述**: 验证用户会话是否有效

**参数**:
- `session_id`: 会话ID (UUID格式，必需)

**请求示例**:
```javascript
// 验证会话
function validateSession(sessionId) {
  return fetch(`/auth/validate?session_id=${sessionId}`)
    .then(response => response.json())
    .then(data => {
      if (data.success) {
        console.log('会话有效，用户信息:', data.user);
        return data.user;
      } else {
        console.error('会话无效:', data.message);
        // 清除无效的session_id
        localStorage.removeItem('fishpi_session_id');
        return null;
      }
    });
}

// 使用示例
const sessionId = localStorage.getItem('fishpi_session_id');
if (sessionId) {
  validateSession(sessionId).then(user => {
    if (user) {
      // 会话有效，可以连接WebSocket
      connectWebSocket(sessionId);
    } else {
      // 会话无效，需要重新登录
      showLoginButton();
    }
  });
}
```

**成功响应**:
```json
{
    "success": true,
    "user": {
        "id": "123456",
        "username": "用户名",
        "nickname": "昵称",
        "avatar": "头像URL"
    },
    "message": null
}
```

**失败响应**:
```json
{
    "success": false,
    "user": null,
    "message": "会话验证失败: 具体错误信息"
}
```

**说明**:
- `session_id`: UUID格式的会话标识符
- `user.id`: 摸鱼派用户ID，为数字字符串格式（如："123456"）
- `user.username`: 摸鱼派用户名
- `user.nickname`: 用户昵称（可选）
- `user.avatar`: 用户头像URL（可选）

#### 完整的前端认证示例

```javascript
class AuthManager {
  constructor() {
    this.sessionId = localStorage.getItem('fishpi_session_id');
  }

  // 检查是否已登录
  async checkLoginStatus() {
    if (!this.sessionId) {
      return false;
    }

    try {
      const response = await fetch(`/auth/validate?session_id=${this.sessionId}`);
      const data = await response.json();
      
      if (data.success) {
        this.user = data.user;
        return true;
      } else {
        // 清除无效的session_id
        this.clearSession();
        return false;
      }
    } catch (error) {
      console.error('验证会话失败:', error);
      this.clearSession();
      return false;
    }
  }

  // 开始登录流程
  async startLogin() {
    try {
      const response = await fetch('/auth/login');
      const data = await response.json();
      
      if (data.success) {
        // 跳转到摸鱼派登录页面
        window.location.href = data.login_url;
      } else {
        throw new Error(data.error);
      }
    } catch (error) {
      console.error('获取登录URL失败:', error);
      throw error;
    }
  }

  // 清除会话
  clearSession() {
    this.sessionId = null;
    this.user = null;
    localStorage.removeItem('fishpi_session_id');
  }

  // 获取用户信息
  getUser() {
    return this.user;
  }

  // 获取session_id
  getSessionId() {
    return this.sessionId;
  }
}

// 使用示例
const auth = new AuthManager();

// 页面加载时检查登录状态
async function initApp() {
  const isLoggedIn = await auth.checkLoginStatus();
  
  if (isLoggedIn) {
    // 已登录，显示游戏界面
    showGameInterface();
    // 连接WebSocket
    connectWebSocket(auth.getSessionId());
  } else {
    // 未登录，显示登录按钮
    showLoginButton();
  }
}

// 显示登录按钮
function showLoginButton() {
  const loginBtn = document.createElement('button');
  loginBtn.textContent = '使用摸鱼派登录';
  loginBtn.onclick = () => auth.startLogin();
  document.body.appendChild(loginBtn);
}

// 显示游戏界面
function showGameInterface() {
  const user = auth.getUser();
  console.log('欢迎，', user.nickname || user.username);
  // 显示游戏相关UI
}

// 连接WebSocket
function connectWebSocket(sessionId) {
  const ws = new WebSocket(`ws://your-domain.com:8080/ws?room_id=room123&session_id=${sessionId}`);
  // WebSocket事件处理...
}

// 初始化应用
initApp();
```

### 2. 静态资源

#### 2.1 游戏页面
**接口**: `GET /` 或 `GET /index.html`

**描述**: 提供游戏前端页面

**响应**: HTML页面

### 3. 房间状态

#### 3.1 获取房间状态
**接口**: `GET /rooms/status`

**描述**: 获取所有房间的状态信息

**请求示例**:
```javascript
// 获取房间状态
fetch('/rooms/status')
  .then(response => response.json())
  .then(data => {
    if (data.success) {
      console.log('房间状态:', data.rooms);
      console.log('总房间数:', data.total_rooms);
    }
  });
```

**成功响应**:
```json
{
    "success": true,
    "rooms": [
        {
            "room_id": "ABC123",
            "player_count": 4,
            "idle_seconds": 120,
            "is_game_over": false,
            "is_empty": false,
            "should_be_deleted": false
        }
    ],
    "total_rooms": 1
}
```

**说明**:
- `room_id`: 房间ID
- `player_count`: 当前玩家数量
- `idle_seconds`: 房间空闲时间（秒）
- `is_game_over`: 游戏是否已结束
- `is_empty`: 房间是否为空
- `should_be_deleted`: 房间是否应该被删除

## WebSocket 接口

### 连接建立

**连接地址**: `ws://{host}:{port}/ws` 或 `wss://{host}:{port}/ws`

**查询参数**:
- `room_id`: 房间ID (可选，不提供则自动生成)
- `session_id`: 用户会话ID (必需)

**连接示例**:
```
ws://your-domain.com:8990/ws?room_id=room123&session_id=550e8400-e29b-41d4-a716-446655440000
wss://your-domain.com:443/ws?room_id=room123&session_id=550e8400-e29b-41d4-a716-446655440000
```

**说明**:
- 如果未提供`room_id`，服务器会自动生成一个6位随机字母的房间ID
- `session_id`必须通过摸鱼派认证获得，格式为UUID
- 连接建立后，服务器会发送用户信息和房间列表

### 消息格式

所有WebSocket消息都使用JSON格式：

```json
{
    "type": "消息类型",
    "data": {
        // 消息数据
    }
}
```

### 客户端发送消息

#### 1. 加入游戏
**消息类型**: `join`

**数据格式**:
```json
{
    "type": "join",
    "data": {
        "player_name": "玩家名称",
        "player_id": "123456"
    }
}
```

**说明**: 
- `player_name`: 使用摸鱼派用户的昵称或用户名
- `player_id`: 使用摸鱼派用户ID（数字字符串格式，如："123456"）
- 服务器会自动处理新玩家加入或断线重连

#### 2. 准备游戏
**消息类型**: `ready`

**数据格式**:
```json
{
    "type": "ready",
    "data": {
        "player_id": "123456"
    }
}
```

#### 3. 描述词语
**消息类型**: `describe`

**数据格式**:
```json
{
    "type": "describe",
    "data": {
        "player_id": "123456",
        "content": "描述内容"
    }
}
```

**限制**: 
- 只能在描述阶段发送
- 每人60秒时间限制
- 内容不能包含敏感词

#### 4. 投票
**消息类型**: `vote`

**数据格式**:
```json
{
    "type": "vote",
    "data": {
        "player_id": "123456",
        "target_id": "789012"
    }
}
```

**限制**:
- 只能在投票阶段发送
- 每人60秒时间限制
- 只能投给存活玩家
- 可以更改投票（重新投票给不同玩家）
- 投票阶段结束后才处理投票结果

**说明**:
- 首次投票会触发 `VoteAdded` 事件
- 更改投票会触发 `VoteChanged` 事件
- 投票阶段不会因为所有玩家都投票而立即结束，需要等待倒计时结束

#### 5. 聊天消息
**消息类型**: `chat`

**数据格式**:
```json
{
    "type": "chat",
    "data": {
        "player_id": "123456",
        "content": "聊天内容"
    }
}
```

#### 6. 离开游戏
**消息类型**: `leave`

**数据格式**:
```json
{
    "type": "leave",
    "data": {
        "player_id": "123456"
    }
}
```

### 服务器推送消息

#### 1. 用户信息
**消息类型**: `user_info`

**数据格式**:
```json
{
    "type": "user_info",
    "data": {
        "user_id": "123456",
        "username": "用户名",
        "nickname": "昵称",
        "avatar": "头像URL"
    }
}
```

**说明**: 连接建立后立即发送
- `user_id`: 摸鱼派用户ID，数字字符串格式
- `username`: 摸鱼派用户名
- `nickname`: 用户昵称（可选）
- `avatar`: 用户头像URL（可选）

#### 2. 房间列表
**消息类型**: `room_list`

**数据格式**:
```json
{
    "type": "room_list",
    "data": {
        "rooms": [
            {
                "id": "房间ID",
                "player_count": 当前玩家数
            }
        ]
    }
}
```

#### 3. 状态更新
**消息类型**: `state_update`

**数据格式**:
```json
{
    "type": "state_update",
    "data": {
        "state": "Lobby|RoleAssignment|DescribePhase|VotePhase|ResultPhase|GameOver",
        "message": "状态说明文字",
        "players": [
            {
                "id": "123456",
                "name": "玩家名称",
                "is_alive": true,
                "role": "civilian|undercover",
                "word": "词语",
                "is_ready": true
            }
        ],
        "current_player": "123456",
        "descriptions": {
            "123456": "描述内容"
        },
        "votes": {
            "123456": "789012"
        },
        "eliminated": "123456",
        "winner": "civilian|undercover"
    }
}
```

**状态说明**:
- `Lobby`: 等待玩家加入，可以准备/取消准备
- `RoleAssignment`: 分配角色和词语（自动进行）
- `DescribePhase`: 描述阶段，轮流描述词语
- `VotePhase`: 投票阶段，同时投票
- `ResultPhase`: 显示投票结果
- `GameOver`: 游戏结束，显示获胜方

**字段说明**:
- `players[].id`: 摸鱼派用户ID，数字字符串格式
- `current_player`: 当前玩家ID，数字字符串格式
- `descriptions`: 玩家ID到描述内容的映射
- `votes`: 投票者ID到被投票者ID的映射
- `eliminated`: 被淘汰玩家ID，数字字符串格式

#### 4. 通知消息
**消息类型**: `notification`

**数据格式**:
```json
{
    "type": "notification",
    "data": {
        "message": "通知内容"
    }
}
```

#### 5. 描述广播
**消息类型**: `description`

**数据格式**:
```json
{
    "type": "description",
    "data": {
        "player_id": "123456",
        "content": "描述内容"
    }
}
```

#### 6. 投票广播
**消息类型**: `vote`

**数据格式**:
```json
{
    "type": "vote",
    "data": {
        "voter_id": "123456",
        "target_id": "789012"
    }
}
```

#### 7. 错误消息
**消息类型**: `error`

**数据格式**:
```json
{
    "type": "error",
    "data": {
        "code": "错误代码",
        "message": "错误信息"
    }
}
```

## 错误代码

| 错误代码 | 说明 |
|---------|------|
| `AuthError` | 认证失败 |
| `AuthRequired` | 需要登录才能进入游戏 |
| `RoomFull` | 房间已满 |
| `GameStarted` | 游戏已开始 |
| `InvalidState` | 无效的游戏状态 |
| `InvalidAction` | 无效的操作 |
| `PlayerNotFound` | 玩家不存在 |
| `NotYourTurn` | 还没轮到您 |
| `AlreadyVoted` | 已经投过票 |
| `InvalidVote` | 无效的投票 |
| `Timeout` | 操作超时 |
| `InternalError` | 内部错误 |
| `RateLimitExceeded` | 操作频率超限 |
| `InvalidMessageFormat` | 消息格式无效 |
| `WordBankError` | 词语库错误 |
| `ConfigError` | 配置错误 |

## 游戏配置

### 玩家配置
- **最少玩家**: 4人
- **最多玩家**: 12人
- **卧底数量**: 总玩家数 × 30%（向上取整）
- **平民数量**: 剩余玩家

### 时间限制
- **描述阶段**: 每人60秒
- **投票阶段**: 每人60秒
- **结果阶段**: 5秒
- **回合间隔**: 5秒

### 胜利条件
- **平民胜利**: 所有卧底被淘汰
- **卧底胜利**: 卧底数量 ≥ 平民数量

## 安全限制

| 限制类型 | 限制值 | 说明 |
|---------|--------|------|
| 消息频率 | 每秒100条 | 超过限制将被临时禁言 |
| 连接数 | 每IP最多3个 | 超过限制将被拒绝连接 |
| 消息大小 | 16KB | 超过限制将被拒绝 |
| 会话超时 | 5分钟 | 空闲超时自动断开 |

## 技术特性

### 1. 认证系统
- 基于摸鱼派OpenID认证
- 支持会话管理和验证
- 自动获取用户信息

### 2. 房间管理
- 动态房间创建
- 房间状态持久化
- 支持断线重连

### 3. 游戏状态
- 实时状态同步
- 自动状态转换
- 超时处理机制

### 4. 数据存储
- Redis持久化
- 游戏历史记录
- 用户会话存储

## 部署说明

### 架构说明

系统支持两种部署模式：

#### 1. 分离模式（推荐）
- HTTP服务器和WebSocket服务器运行在不同端口
- 适合生产环境，可以独立扩展和负载均衡
- 支持不同的域名配置

#### 2. 兼容模式
- HTTP和WebSocket服务器运行在同一端口
- 适合开发环境或简单部署
- 使用传统单服务器架构

### 环境要求
- Rust 1.70+
- Redis 6.0+
- 摸鱼派开发者账号

### 配置示例

#### 开发环境配置
```toml
[server]
host = "127.0.0.1"
port = 8080
http_port = 8080
ws_port = 8080
```

#### 生产环境配置
```toml
[server]
host = "0.0.0.0"
port = 8989
http_port = 443  # HTTPS端口
ws_port = 8990   # WebSocket端口

[auth]
domain = "https://your-domain.com"
```

### 前端连接示例

```javascript
// 连接WebSocket服务器
function connectWebSocket(sessionId, roomId = null) {
  const wsHost = 'ws.your-domain.com';  // WebSocket服务器地址
  const wsPort = 8990;                  // WebSocket服务器端口
  
  let wsUrl = `ws://${wsHost}:${wsPort}/ws?session_id=${sessionId}`;
  if (roomId) {
    wsUrl += `&room_id=${roomId}`;
  }
  
  const ws = new WebSocket(wsUrl);
  // WebSocket事件处理...
}
```

## 注意事项

1. **认证必需**: 所有WebSocket连接都需要有效的session_id
2. **断线重连**: 支持断线重连，重连后可以继续游戏
3. **状态同步**: 每次状态变更都会推送完整的房间信息
4. **错误处理**: 所有操作都可能失败，需要做好错误处理
5. **安全考虑**: 建议在生产环境中使用HTTPS/WSS 

## 使用示例

### 前端集成示例

#### 1. 基本登录流程（使用默认回调）

```javascript
class AuthManager {
  constructor() {
    this.sessionId = localStorage.getItem('fishpi_session_id');
  }

  // 检查是否已登录
  async checkLoginStatus() {
    if (!this.sessionId) {
      return false;
    }

    try {
      const response = await fetch(`/auth/validate?session_id=${this.sessionId}`);
      const data = await response.json();
      
      if (data.success) {
        this.user = data.user;
        return true;
      } else {
        // 清除无效的session_id
        this.clearSession();
        return false;
      }
    } catch (error) {
      console.error('验证会话失败:', error);
      this.clearSession();
      return false;
    }
  }

  // 开始登录流程（使用默认回调）
  async startLogin() {
    try {
      const response = await fetch('/auth/login');
      const data = await response.json();
      
      if (data.success) {
        // 跳转到摸鱼派登录页面
        window.location.href = data.login_url;
      } else {
        throw new Error(data.error);
      }
    } catch (error) {
      console.error('获取登录URL失败:', error);
      throw error;
    }
  }

  // 开始登录流程（指定自定义回调）
  async startLoginWithCallback(callbackUrl) {
    try {
      const encodedCallback = encodeURIComponent(callbackUrl);
      const response = await fetch(`/auth/login?callback_url=${encodedCallback}`);
      const data = await response.json();
      
      if (data.success) {
        // 跳转到摸鱼派登录页面
        window.location.href = data.login_url;
      } else {
        throw new Error(data.error);
      }
    } catch (error) {
      console.error('获取登录URL失败:', error);
      throw error;
    }
  }

  // 清除会话
  clearSession() {
    this.sessionId = null;
    this.user = null;
    localStorage.removeItem('fishpi_session_id');
  }

  // 获取用户信息
  getUser() {
    return this.user;
  }

  // 获取session_id
  getSessionId() {
    return this.sessionId;
  }
}

// 使用示例
const auth = new AuthManager();

// 页面加载时检查登录状态
async function initApp() {
  const isLoggedIn = await auth.checkLoginStatus();
  
  if (isLoggedIn) {
    // 已登录，显示游戏界面
    showGameInterface();
    // 连接WebSocket
    connectWebSocket(auth.getSessionId());
  } else {
    // 未登录，显示登录按钮
    showLoginButton();
  }
}

// 显示登录按钮
function showLoginButton() {
  const loginButton = document.createElement('button');
  loginButton.textContent = '登录';
  loginButton.onclick = () => {
    // 使用默认回调地址
    auth.startLogin();
  };
  document.body.appendChild(loginButton);
}

// 显示登录按钮（指定回调地址）
function showLoginButtonWithCallback() {
  const loginButton = document.createElement('button');
  loginButton.textContent = '登录';
  loginButton.onclick = () => {
    // 指定登录成功后的回调地址
    auth.startLoginWithCallback('http://localhost:3000/game');
  };
  document.body.appendChild(loginButton);
}
```

#### 2. 处理登录回调

```javascript
// 页面加载时检查URL参数
function handleLoginCallback() {
  const urlParams = new URLSearchParams(window.location.search);
  const sessionId = urlParams.get('session_id');
  
  if (sessionId) {
    // 保存session_id到localStorage
    localStorage.setItem('fishpi_session_id', sessionId);
    
    // 清除URL参数
    const newUrl = window.location.pathname;
    window.history.replaceState({}, document.title, newUrl);
    
    // 初始化应用
    initApp();
  }
}

// 页面加载时执行
document.addEventListener('DOMContentLoaded', () => {
  handleLoginCallback();
  initApp();
});
```

### 不同场景的使用方式

#### 1. 本地开发环境

```javascript
// 本地开发时指定回调地址
auth.startLoginWithCallback('http://localhost:3000/game');
```

#### 2. 生产环境

```javascript
// 生产环境使用默认回调或指定生产地址
auth.startLoginWithCallback('https://your-app.com/dashboard');
```

#### 3. 多页面应用

```javascript
// 不同页面指定不同的回调地址
auth.startLoginWithCallback('https://your-app.com/profile');
auth.startLoginWithCallback('https://your-app.com/settings');
```

### 注意事项

1. **URL编码**: 前端在传递 `callback_url` 时需要进行 URL 编码
2. **域名验证**: 建议在生产环境中验证回调地址的域名
3. **HTTPS要求**: 生产环境的回调地址应该使用 HTTPS
4. **参数传递**: session_id 会作为 URL 参数传递到回调地址