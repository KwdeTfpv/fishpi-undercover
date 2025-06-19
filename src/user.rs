use crate::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json;
use std::collections::HashMap;
use tracing::debug;
use url::Url;
use uuid::Uuid;

/// 用户信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub username: String,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub created_at: DateTime<Utc>,
    pub last_login: DateTime<Utc>,
}

/// 用户会话
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserSession {
    pub session_id: Uuid,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

/// 摸鱼派用户信息响应
#[derive(Debug, Deserialize)]
#[allow(dead_code)]
struct FishpiUserInfoResponse {
    pub msg: String,
    pub code: i32,
    pub data: FishpiUserInfoData,
}

#[derive(Debug, Deserialize)]
struct FishpiUserInfoData {
    #[serde(rename = "userAvatarURL")]
    pub user_avatar_url: Option<String>,
    #[serde(rename = "userNickname")]
    pub user_nickname: Option<String>,
    #[serde(rename = "userName")]
    pub user_name: String,
}

/// 用户管理器
pub struct UserManager {
    sessions: HashMap<Uuid, UserSession>,
    users: HashMap<String, User>,
    fishpi_base_url: String,
}

impl UserManager {
    /// 创建新的用户管理器
    pub fn new() -> Self {
        UserManager {
            sessions: HashMap::new(),
            users: HashMap::new(),
            fishpi_base_url: "https://fishpi.cn".to_string(),
        }
    }

    /// 生成摸鱼派登录URL
    pub fn generate_login_url(&self, return_url: &str, realm: &str) -> Result<String> {
        // 验证参数格式
        if !return_url.starts_with("https://") {
            return Err(crate::Error::Auth("return_to必须是HTTPS".to_string()));
        }

        if !realm.starts_with("https://") {
            return Err(crate::Error::Auth("realm必须是HTTPS".to_string()));
        }

        // 验证realm是return_to的前缀
        if !return_url.starts_with(realm) {
            return Err(crate::Error::Auth("realm必须是return_to的前缀".to_string()));
        }

        let mut url = Url::parse(&format!("{}/openid/login", self.fishpi_base_url))
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        // 使用url crate的query_pairs_mut来确保正确的URL编码
        let mut query_pairs = url.query_pairs_mut();

        // 按照文档要求的顺序添加参数，确保正确编码
        query_pairs.append_pair("openid.ns", "http://specs.openid.net/auth/2.0");
        query_pairs.append_pair("openid.mode", "checkid_setup");
        query_pairs.append_pair("openid.return_to", return_url);
        query_pairs.append_pair("openid.realm", realm);
        query_pairs.append_pair(
            "openid.claimed_id",
            "http://specs.openid.net/auth/2.0/identifier_select",
        );
        query_pairs.append_pair(
            "openid.identity",
            "http://specs.openid.net/auth/2.0/identifier_select",
        );

        // 释放query_pairs以完成URL构建
        drop(query_pairs);

        debug!("生成的登录URL: {}", url);

        Ok(url.to_string())
    }

    /// 验证摸鱼派OpenID响应
    pub async fn verify_openid_response(&self, params: &HashMap<String, String>) -> Result<String> {
        // 检查是否是成功的响应
        if let Some(mode) = params.get("openid.mode") {
            if mode == "id_res" {
                // 检查response_nonce的有效期
                if let Some(response_nonce) = params.get("openid.response_nonce") {
                    if !self.is_response_nonce_valid(response_nonce)? {
                        return Err(crate::Error::Auth("response_nonce已过期或无效".to_string()));
                    }
                } else {
                    return Err(crate::Error::Auth("缺少response_nonce参数".to_string()));
                }

                // 进行签名校验
                self.verify_signature(params).await?;

                // 提取用户ID
                if let Some(claimed_id) = params.get("openid.claimed_id") {
                    // /openid/id/123456
                    if let Some(user_id) = claimed_id.split('/').last() {
                        return Ok(user_id.to_string());
                    } else {
                        return Err(crate::Error::Auth(
                            "无法从claimed_id中提取用户ID".to_string(),
                        ));
                    }
                } else {
                    return Err(crate::Error::Auth("缺少openid.claimed_id参数".to_string()));
                }
            } else {
                return Err(crate::Error::Auth("无效的OpenID模式".to_string()));
            }
        } else {
            return Err(crate::Error::Auth("缺少openid.mode参数".to_string()));
        }
    }

    /// 检查response_nonce是否有效
    fn is_response_nonce_valid(&self, response_nonce: &str) -> Result<bool> {
        // response_nonce格式: 2025-06-19T03:52:20Z8241ed4a70
        if let Some(timestamp_str) = response_nonce.split('Z').next() {
            if let Ok(timestamp) = DateTime::parse_from_rfc3339(&format!("{}Z", timestamp_str)) {
                let now = Utc::now();
                let diff = now.signed_duration_since(timestamp.naive_utc().and_utc());

                // 检查是否在5分钟内
                if diff.num_minutes() <= 5 {
                    debug!("response_nonce时间有效，距离现在{}分钟", diff.num_minutes());
                    return Ok(true);
                } else {
                    debug!("response_nonce已过期，距离现在{}分钟", diff.num_minutes());
                    return Ok(false);
                }
            }
        }

        debug!("无法解析response_nonce时间戳: {}", response_nonce);
        Ok(false)
    }

    /// 验证OpenID签名
    async fn verify_signature(&self, params: &HashMap<String, String>) -> Result<()> {
        // 构建验证请求参数
        let mut verify_params = HashMap::new();

        // 获取openid.signed参数，确定哪些参数被签名了
        let signed_params = if let Some(signed_str) = params.get("openid.signed") {
            signed_str.split(',').collect::<Vec<&str>>()
        } else {
            return Err(crate::Error::Auth("缺少openid.signed参数".to_string()));
        };

        // 添加所有必需的参数
        verify_params.insert(
            "openid.ns".to_string(),
            "http://specs.openid.net/auth/2.0".to_string(),
        );
        verify_params.insert(
            "openid.mode".to_string(),
            "check_authentication".to_string(),
        );

        // 添加所有被签名的参数
        for (key, value) in params {
            if key == "openid.signed" || key == "openid.sig" {
                verify_params.insert(key.clone(), value.clone());
            } else if key == "openid.mode" {
                // 跳过，已经设置为check_authentication
                continue;
            } else {
                // 检查这个参数是否在签名列表中
                let param_name = key.strip_prefix("openid.").unwrap_or(key);
                if signed_params.contains(&param_name) {
                    verify_params.insert(key.clone(), value.clone());
                }
            }
        }

        // 发送验证请求到摸鱼派
        let verify_url = format!("{}/openid/verify", self.fishpi_base_url);

        // 构建请求体，使用JSON格式
        let query_string = serde_json::to_string(&verify_params)
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        // 创建HTTP客户端
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        // 发送POST请求
        let response = client
            .post(&verify_url)
            .header("Content-Type", "application/json")
            .body(query_string)
            .send()
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        if !response.status().is_success() {
            return Err(crate::Error::Auth(format!(
                "验证请求失败，状态码: {}",
                response.status()
            )));
        }

        let response_text = response
            .text()
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        // 解析响应
        let lines: Vec<&str> = response_text.lines().collect();
        let mut is_valid = false;

        for line in lines {
            if line.starts_with("is_valid:") {
                let valid_str = line.split(':').nth(1).unwrap_or("false").trim();
                is_valid = valid_str == "true";
                break;
            }
        }

        if is_valid {
            Ok(())
        } else {
            Err(crate::Error::Auth("签名验证失败".to_string()))
        }
    }

    /// 获取用户信息
    pub async fn get_user_info(&mut self, user_id: &str) -> Result<User> {
        // 检查缓存
        if let Some(user) = self.users.get(user_id) {
            return Ok(user.clone());
        }

        // 从摸鱼派API获取用户信息
        let url = format!(
            "{}/api/user/getInfoById?userId={}",
            self.fishpi_base_url, user_id
        );

        // 创建新的HTTP客户端
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36")
            .build()
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        let response = client
            .get(&url)
            .send()
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        let status = response.status();
        let response_text = response
            .text()
            .await
            .map_err(|e| crate::Error::Network(anyhow::anyhow!(e)))?;

        if status.is_success() {
            let fishpi_user: FishpiUserInfoResponse = serde_json::from_str(&response_text)
                .map_err(|e| {
                    println!("JSON解析错误: {}", e);
                    crate::Error::Network(anyhow::anyhow!(e))
                })?;

            let user = User {
                id: user_id.to_string(),
                username: fishpi_user.data.user_name,
                nickname: fishpi_user.data.user_nickname,
                avatar: fishpi_user.data.user_avatar_url,
                created_at: Utc::now(),
                last_login: Utc::now(),
            };

            // 缓存用户信息
            self.users.insert(user.id.clone(), user.clone());

            Ok(user)
        } else {
            Err(crate::Error::Auth("获取用户信息失败".to_string()))
        }
    }

    /// 创建用户会话
    pub fn create_session(&mut self, user_id: &str) -> Result<Uuid> {
        let session_id = Uuid::new_v4();
        let now = Utc::now();
        let expires_at = now + chrono::Duration::days(30); // 30天过期

        let session = UserSession {
            session_id,
            user_id: user_id.to_string(),
            created_at: now,
            expires_at,
        };

        self.sessions.insert(session_id, session);

        debug!("创建用户会话: {} -> {}", session_id, user_id);

        Ok(session_id)
    }

    /// 验证会话
    pub fn validate_session(&self, session_id: &Uuid) -> Result<&UserSession> {
        if let Some(session) = self.sessions.get(session_id) {
            if session.expires_at > Utc::now() {
                return Ok(session);
            } else {
                // 会话已过期，应该清理
                return Err(crate::Error::Auth("会话已过期".to_string()));
            }
        }

        Err(crate::Error::Auth("无效的会话".to_string()))
    }

    /// 获取会话对应的用户
    pub fn get_user_by_session(&mut self, session_id: &Uuid) -> Result<&User> {
        // 先验证会话并获取用户ID
        let user_id = {
            let session = self.validate_session(session_id)?;
            session.user_id.clone()
        };

        // 验证成功，延长会话有效期
        self.extend_session(session_id)?;

        self.users
            .get(&user_id)
            .ok_or_else(|| crate::Error::Auth("用户不存在".to_string()))
    }

    /// 删除会话
    pub fn remove_session(&mut self, session_id: &Uuid) {
        self.sessions.remove(session_id);
        debug!("删除用户会话: {}", session_id);
    }

    /// 清理过期会话
    pub fn cleanup_expired_sessions(&mut self) {
        let now = Utc::now();
        self.sessions.retain(|_, session| session.expires_at > now);
        debug!("清理过期会话完成");
    }

    /// 处理登录流程
    pub async fn handle_login(
        &mut self,
        openid_params: &HashMap<String, String>,
    ) -> Result<(Uuid, User)> {
        // 验证OpenID响应
        let user_id = self.verify_openid_response(openid_params).await?;

        // 获取用户信息
        let user = self.get_user_info(&user_id).await?;

        // 创建会话
        let session_id = self.create_session(&user_id)?;

        Ok((session_id, user))
    }

    /// 延长会话有效期
    pub fn extend_session(&mut self, session_id: &Uuid) -> Result<()> {
        if let Some(session) = self.sessions.get_mut(session_id) {
            session.expires_at = Utc::now() + chrono::Duration::days(30); // 延长30天
            debug!("延长会话有效期: {}", session_id);
            Ok(())
        } else {
            Err(crate::Error::Auth("会话不存在".to_string()))
        }
    }
}

impl Default for UserManager {
    fn default() -> Self {
        Self::new()
    }
}
