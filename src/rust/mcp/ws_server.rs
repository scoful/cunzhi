use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::mcp::types::PopupRequest;
use crate::log_important;
use std::time::{Duration, Instant};
use serde_json::json;
use crate::constants::network::{
    WEBSOCKET_PING_INTERVAL_SECS,
    WEBSOCKET_PONG_TIMEOUT_SECS,
};

/// WebSocket服务器配置
pub struct WsServerConfig {
    pub host: String,
    pub port: u16,
    pub api_key: Option<String>, // API密钥，用于客户端认证
}

impl WsServerConfig {
    /// 从环境变量加载配置
    /// 只有配置了API Key才返回配置,否则返回None(不启动WebSocket服务器)
    pub fn from_env() -> Option<Self> {
        // 检查是否配置了API Key
        let api_key = std::env::var("CUNZHI_WS_API_KEY").ok();

        if api_key.is_none() {
            log_important!(info, "未配置CUNZHI_WS_API_KEY,跳过WebSocket服务器启动");
            return None;
        }

        Some(Self {
            host: std::env::var("CUNZHI_WS_HOST")
                .unwrap_or_else(|_| "0.0.0.0".to_string()),
            port: std::env::var("CUNZHI_WS_PORT")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or(9000),
            api_key,
        })
    }
}

/// 客户端认证状态
#[derive(Debug, Clone)]
enum AuthStatus {
    Unauthenticated, // 未认证
    Authenticated,   // 已认证
}

/// WebSocket客户端连接
struct WsClient {
    sender: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
    auth_status: AuthStatus,
    connect_time: Instant, // 连接时间，用于认证超时
    last_pong_time: Instant, // 最后收到pong的时间，用于检测僵尸连接
}

/// WebSocket服务器状态
pub struct WsServer {
    clients: Arc<Mutex<HashMap<String, WsClient>>>,
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    config: WsServerConfig,
}

impl WsServer {
    pub fn new(config: WsServerConfig) -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            config,
        }
    }

    /// 启动WebSocket服务器
    pub async fn start(self: Arc<Self>) -> Result<()> {
        let addr = format!("{}:{}", self.config.host, self.config.port);
        let listener = TcpListener::bind(&addr).await?;
        log_important!(info, "WebSocket服务器启动: {}", addr);

        // 启动心跳任务
        let server_for_heartbeat = self.clone();
        tokio::spawn(async move {
            server_for_heartbeat.heartbeat_task().await;
        });

        loop {
            match listener.accept().await {
                Ok((stream, peer_addr)) => {
                    log_important!(info, "新客户端连接: {}", peer_addr);
                    let server = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = server.handle_client(stream, peer_addr.to_string()).await {
                            log_important!(warn, "客户端处理失败: {}", e);
                        }
                    });
                }
                Err(e) => {
                    log_important!(warn, "接受连接失败: {}", e);
                }
            }
        }
    }

    /// 处理单个客户端连接
    async fn handle_client(&self, stream: TcpStream, client_id: String) -> Result<()> {
        let ws_stream = accept_async(stream).await?;
        let (write, mut read) = ws_stream.split();

        // 注册客户端（初始状态为未认证）
        {
            let mut clients = self.clients.lock().await;
            let now = Instant::now();
            clients.insert(client_id.clone(), WsClient {
                sender: write,
                auth_status: AuthStatus::Unauthenticated,
                connect_time: now,
                last_pong_time: now,
            });
        }

        log_important!(info, "客户端已注册: {}", client_id);

        // 处理消息
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_message(&client_id, &text).await {
                        log_important!(warn, "处理消息失败: {}", e);
                    }
                }
                Ok(Message::Pong(_)) => {
                    // 收到pong响应，更新最后pong时间
                    let mut clients = self.clients.lock().await;
                    if let Some(client) = clients.get_mut(&client_id) {
                        client.last_pong_time = Instant::now();
                    }
                }
                Ok(Message::Close(_)) => {
                    log_important!(info, "客户端断开: {}", client_id);
                    break;
                }
                Err(e) => {
                    log_important!(warn, "接收消息失败: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // 移除客户端
        {
            let mut clients = self.clients.lock().await;
            clients.remove(&client_id);
        }

        Ok(())
    }

    /// 处理客户端消息
    async fn handle_message(&self, client_id: &str, text: &str) -> Result<()> {
        let message: serde_json::Value = serde_json::from_str(text)?;

        // 检查客户端认证状态
        let auth_status = {
            let clients = self.clients.lock().await;
            if let Some(client) = clients.get(client_id) {
                client.auth_status.clone()
            } else {
                return Ok(()); // 客户端不存在
            }
        };

        // 处理认证消息
        if let Some(msg_type) = message.get("type").and_then(|v| v.as_str()) {
            if msg_type == "auth" {
                return self.handle_auth_message(client_id, &message).await;
            }
        }

        // 强制要求API Key认证
        match auth_status {
            AuthStatus::Unauthenticated => {
                // 检查认证超时（10秒）
                let connect_time = {
                    let clients = self.clients.lock().await;
                    clients.get(client_id).map(|c| c.connect_time).unwrap_or_else(Instant::now)
                };

                if connect_time.elapsed() > Duration::from_secs(10) {
                    log_important!(warn, "客户端认证超时: {}", client_id);
                    self.send_error_and_disconnect(client_id, "认证超时，连接已断开").await?;
                    return Ok(());
                }

                // 未认证，拒绝处理其他消息
                self.send_error_message(client_id, "请先发送认证消息").await?;
                return Ok(());
            }
            AuthStatus::Authenticated => {
                // 已认证，继续处理消息
            }
        }

        // 处理弹窗响应
        if let Some(request_id) = message.get("request_id").and_then(|v| v.as_str()) {
            let mut pending = self.pending_requests.lock().await;
            if let Some(sender) = pending.remove(request_id) {
                let _ = sender.send(text.to_string());
            }
        }

        Ok(())
    }

    /// 发送弹窗请求到客户端
    pub async fn send_popup_request(&self, request: &PopupRequest) -> Result<String> {
        // 检查是否有已认证的在线客户端
        let client_id = {
            let clients = self.clients.lock().await;
            if clients.is_empty() {
                anyhow::bail!("没有在线的WebSocket客户端");
            }

            // 强制要求认证，只选择已认证的客户端
            let authenticated_client = clients.iter()
                .find(|(_, client)| matches!(client.auth_status, AuthStatus::Authenticated))
                .map(|(id, _)| id.clone());

            if let Some(id) = authenticated_client {
                id
            } else {
                anyhow::bail!("没有已认证的WebSocket客户端");
            }
        };

        // 创建响应通道
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request.id.clone(), tx);
        }

        // 发送请求
        let request_json = serde_json::to_string(&serde_json::json!({
            "type": "popup_request",
            "request_id": request.id,
            "message": request.message,
            "predefined_options": request.predefined_options,
            "is_markdown": request.is_markdown,
        }))?;

        {
            let mut clients = self.clients.lock().await;
            if let Some(client) = clients.get_mut(&client_id) {
                client.sender.send(Message::Text(request_json)).await?;
            } else {
                anyhow::bail!("客户端已断开");
            }
        }

        // 等待响应(永不超时,依赖断线检测)
        match rx.await {
            Ok(response) => Ok(response),
            Err(_) => anyhow::bail!("响应通道关闭"),
        }
    }

    /// 检查是否有在线客户端
    pub async fn has_clients(&self) -> bool {
        let clients = self.clients.lock().await;
        !clients.is_empty()
    }

    /// 处理认证消息
    async fn handle_auth_message(&self, client_id: &str, message: &serde_json::Value) -> Result<()> {
        let received_api_key = message.get("api_key").and_then(|v| v.as_str());

        // 强制要求API Key认证
        if let Some(expected_api_key) = &self.config.api_key {
            if let Some(key) = received_api_key {
                if key == expected_api_key {
                    // 认证成功
                    {
                        let mut clients = self.clients.lock().await;
                        if let Some(client) = clients.get_mut(client_id) {
                            client.auth_status = AuthStatus::Authenticated;
                        }
                    }

                    log_important!(info, "客户端认证成功: {}", client_id);
                    self.send_auth_response(client_id, true, "认证成功").await?;
                } else {
                    // 认证失败
                    log_important!(warn, "客户端认证失败，API Key不匹配: {}", client_id);
                    self.send_error_and_disconnect(client_id, "API Key验证失败").await?;
                }
            } else {
                // 缺少API Key
                log_important!(warn, "客户端认证失败，缺少API Key: {}", client_id);
                self.send_error_and_disconnect(client_id, "缺少API Key").await?;
            }
        } else {
            // 服务端未配置API Key，拒绝连接
            log_important!(warn, "服务端未配置API Key，拒绝客户端连接: {}", client_id);
            self.send_error_and_disconnect(client_id, "服务端未配置API Key，请设置CUNZHI_WS_API_KEY环境变量").await?;
        }

        Ok(())
    }

    /// 发送认证响应
    async fn send_auth_response(&self, client_id: &str, success: bool, message: &str) -> Result<()> {
        let response = json!({
            "type": "auth_response",
            "success": success,
            "message": message
        });

        self.send_message_to_client(client_id, &response.to_string()).await
    }

    /// 发送错误消息
    async fn send_error_message(&self, client_id: &str, error: &str) -> Result<()> {
        let response = json!({
            "type": "error",
            "message": error
        });

        self.send_message_to_client(client_id, &response.to_string()).await
    }

    /// 发送错误消息并断开连接
    async fn send_error_and_disconnect(&self, client_id: &str, error: &str) -> Result<()> {
        self.send_error_message(client_id, error).await?;

        // 移除客户端
        {
            let mut clients = self.clients.lock().await;
            clients.remove(client_id);
        }

        Ok(())
    }

    /// 发送消息到指定客户端
    async fn send_message_to_client(&self, client_id: &str, message: &str) -> Result<()> {
        let mut clients = self.clients.lock().await;
        if let Some(client) = clients.get_mut(client_id) {
            client.sender.send(Message::Text(message.to_string())).await?;
        }
        Ok(())
    }

    /// 心跳任务 - 定期发送ping并清理超时连接
    async fn heartbeat_task(&self) {
        let ping_interval = Duration::from_secs(WEBSOCKET_PING_INTERVAL_SECS);
        let pong_timeout = Duration::from_secs(WEBSOCKET_PONG_TIMEOUT_SECS);

        loop {
            tokio::time::sleep(ping_interval).await;

            let mut clients = self.clients.lock().await;
            let mut to_remove = Vec::new();

            for (client_id, client) in clients.iter_mut() {
                // 只对已认证的客户端发送ping
                if matches!(client.auth_status, AuthStatus::Authenticated) {
                    // 检查pong超时
                    if client.last_pong_time.elapsed() > pong_timeout {
                        log_important!(warn, "客户端pong超时，断开连接: {}", client_id);
                        to_remove.push(client_id.clone());
                        continue;
                    }

                    // 发送ping
                    if let Err(e) = client.sender.send(Message::Ping(vec![])).await {
                        log_important!(warn, "发送ping失败，标记断开: {} - {}", client_id, e);
                        to_remove.push(client_id.clone());
                    }
                }
            }

            // 移除超时或发送失败的客户端
            for client_id in to_remove {
                clients.remove(&client_id);
                log_important!(info, "已清理客户端: {}", client_id);
            }
        }
    }
}

