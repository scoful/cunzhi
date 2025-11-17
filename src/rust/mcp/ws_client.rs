use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::mcp::types::{PopupRequest, RegisterMessage, RegisterAck, RegisterError, PopupRequestMessage, PopupResponseMessage};
use crate::log_important;
use crate::constants::network::{
    WEBSOCKET_PING_INTERVAL_SECS,
    WEBSOCKET_PONG_TIMEOUT_SECS,
};

/// WebSocket客户端配置
pub struct WsClientConfig {
    pub host: String,
    pub port: u16,
    pub client_id: String,
}

impl WsClientConfig {
    /// 从环境变量加载配置
    /// 如果未配置,返回None(不启动WebSocket客户端)
    pub fn from_env() -> Option<Self> {
        // 检查是否配置了连接端口(默认9000)
        let port = std::env::var("CUNZHI_WS_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9000);

        let host = std::env::var("CUNZHI_WS_HOST")
            .unwrap_or_else(|_| "localhost".to_string());

        // 生成CLIENT_ID: 优先使用环境变量,否则自动生成
        let client_id = std::env::var("CUNZHI_CLIENT_ID")
            .unwrap_or_else(|_| Self::generate_client_id());

        Some(Self {
            host,
            port,
            client_id,
        })
    }

    /// 自动生成CLIENT_ID: 主机名-进程ID
    fn generate_client_id() -> String {
        let hostname = hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "unknown".to_string());
        
        let pid = std::process::id();
        format!("{}-{}", hostname, pid)
    }
}

/// WebSocket客户端连接状态
#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

/// WebSocket客户端
pub struct WsClient {
    config: WsClientConfig,
    connection: Arc<Mutex<Option<futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
        Message,
    >>>>,
    status: Arc<Mutex<ConnectionStatus>>,
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
    last_pong_time: Arc<Mutex<Instant>>,
}

impl WsClient {
    pub fn new(config: WsClientConfig) -> Self {
        Self {
            config,
            connection: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(ConnectionStatus::Disconnected)),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
            last_pong_time: Arc::new(Mutex::new(Instant::now())),
        }
    }

    /// 获取最大重试次数
    ///
    /// 优先级:
    /// 1. 环境变量 CUNZHI_WS_MAX_RETRIES (可以是数字或"infinite")
    /// 2. 默认值: Linux无限重试, Windows/macOS重试3次
    fn get_max_retries() -> u32 {
        // 1. 优先使用环境变量
        if let Ok(retries) = std::env::var("CUNZHI_WS_MAX_RETRIES") {
            if retries == "infinite" || retries == "∞" {
                return u32::MAX;
            }
            if let Ok(n) = retries.parse() {
                return n;
            }
        }

        // 2. 默认: Linux无限重试,其他平台3次
        #[cfg(target_os = "linux")]
        return u32::MAX;

        #[cfg(not(target_os = "linux"))]
        return 3;
    }

    /// 启动WebSocket客户端(带重试机制)
    ///
    /// 重试策略:
    /// - Linux: 无限重试 (连接失败或断开都会重试)
    /// - Windows/macOS: 重试3次后fallback到本地模式
    /// - 环境变量 CUNZHI_WS_MAX_RETRIES 可以覆盖默认值
    pub async fn start_with_retry(self: Arc<Self>) {
        let mut retry_count = 0;
        let max_retries = Self::get_max_retries();
        let retry_interval = Duration::from_secs(5);

        loop {
            match self.clone().start().await {
                Ok(_) => {
                    // 连接成功后断开,继续重试
                    retry_count += 1;

                    if retry_count >= max_retries {
                        log_important!(warn,
                            "WebSocket客户端断开,已达最大重试次数({}次),将使用本地模式",
                            max_retries
                        );
                        break;
                    }

                    let retry_display = if max_retries == u32::MAX {
                        format!("{}/∞", retry_count)
                    } else {
                        format!("{}/{}", retry_count, max_retries)
                    };

                    log_important!(warn,
                        "WebSocket客户端断开({}),{}秒后重连",
                        retry_display, retry_interval.as_secs()
                    );

                    tokio::time::sleep(retry_interval).await;
                }
                Err(e) => {
                    // 连接失败,继续重试
                    retry_count += 1;

                    if retry_count >= max_retries {
                        log_important!(warn,
                            "WebSocket客户端连接失败,已达最大重试次数({}次),将使用本地模式: {}",
                            max_retries, e
                        );
                        break;
                    }

                    let retry_display = if max_retries == u32::MAX {
                        format!("{}/∞", retry_count)
                    } else {
                        format!("{}/{}", retry_count, max_retries)
                    };

                    log_important!(warn,
                        "WebSocket客户端连接失败({}): {}, {}秒后重试",
                        retry_display, e, retry_interval.as_secs()
                    );

                    tokio::time::sleep(retry_interval).await;
                }
            }
        }
    }

    /// 启动WebSocket客户端(连接并注册)
    async fn start(self: Arc<Self>) -> Result<()> {
        // 设置状态为连接中
        {
            let mut status = self.status.lock().await;
            *status = ConnectionStatus::Connecting;
        }

        let url = format!("ws://{}:{}", self.config.host, self.config.port);
        log_important!(info, "WebSocket客户端连接: {}", url);

        // 连接到服务器
        let (ws_stream, _) = match connect_async(&url).await {
            Ok(result) => result,
            Err(e) => {
                let error_msg = format!("连接失败: {}", e);
                log_important!(warn, "{}", error_msg);
                let mut status = self.status.lock().await;
                *status = ConnectionStatus::Error(error_msg);
                return Err(e.into());
            }
        };

        let (mut write, mut read) = ws_stream.split();

        // 发送注册消息
        let register_msg = RegisterMessage {
            msg_type: "register".to_string(),
            client_id: self.config.client_id.clone(),
        };
        let register_json = serde_json::to_string(&register_msg)?;
        write.send(Message::Text(register_json)).await?;

        log_important!(info, "已发送注册消息: {}", self.config.client_id);

        // 保存写入端
        {
            let mut conn = self.connection.lock().await;
            *conn = Some(write);
        }

        // 启动心跳任务
        let client_for_heartbeat = self.clone();
        tokio::spawn(async move {
            client_for_heartbeat.heartbeat_task().await;
        });

        // 处理消息
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_message(&text).await {
                        log_important!(warn, "处理消息失败: {}", e);
                    }
                }
                Ok(Message::Ping(_)) => {
                    // 收到服务端ping,更新心跳时间
                    // WebSocket库会自动回复Pong
                    log_important!(info, "收到服务端ping");
                    let mut last_pong = self.last_pong_time.lock().await;
                    *last_pong = Instant::now();
                }
                Ok(Message::Close(_)) => {
                    log_important!(info, "服务器断开连接");
                    break;
                }
                Err(e) => {
                    log_important!(warn, "接收消息失败: {}", e);
                    break;
                }
                _ => {}
            }
        }

        // 连接断开,更新状态
        {
            let mut status = self.status.lock().await;
            *status = ConnectionStatus::Disconnected;
        }
        {
            let mut conn = self.connection.lock().await;
            *conn = None;
        }

        log_important!(info, "WebSocket客户端已断开");
        Ok(())
    }

    /// 处理服务器消息
    async fn handle_message(&self, text: &str) -> Result<()> {
        let message: serde_json::Value = serde_json::from_str(text)?;

        if let Some(msg_type) = message.get("type").and_then(|v| v.as_str()) {
            match msg_type {
                "register_ack" => {
                    let ack: RegisterAck = serde_json::from_str(text)?;
                    log_important!(info, "注册成功: {}", ack.message);
                    let mut status = self.status.lock().await;
                    *status = ConnectionStatus::Connected;
                }
                "register_error" => {
                    let error: RegisterError = serde_json::from_str(text)?;
                    log_important!(error, "注册失败: {}", error.error);
                    let mut status = self.status.lock().await;
                    *status = ConnectionStatus::Error(error.error.clone());
                }
                "popup_request" => {
                    // 处理弹窗请求(寸止作为服务器时使用,当前架构不需要)
                    self.handle_popup_request(&message).await?;
                }
                "popup_response" => {
                    // 处理弹窗响应(寸止作为客户端时使用)
                    self.handle_popup_response(&message).await?;
                }
                _ => {
                    log_important!(warn, "未知消息类型: {}", msg_type);
                }
            }
        }

        Ok(())
    }

    /// 处理弹窗请求(寸止作为服务器时使用,当前架构不需要)
    async fn handle_popup_request(&self, message: &serde_json::Value) -> Result<()> {
        let request: PopupRequestMessage = serde_json::from_value(message.clone())?;

        log_important!(info, "收到弹窗请求: {}", request.request_id);

        // 调用本地"等一下"处理弹窗
        let popup_request = PopupRequest {
            id: request.request_id.clone(),
            message: request.message,
            predefined_options: request.predefined_options,
            is_markdown: request.is_markdown,
        };

        // 调用本地弹窗处理函数
        let response = crate::mcp::handlers::popup::create_local_popup(&popup_request)?;

        // 发送响应回服务器
        let response_msg = PopupResponseMessage {
            msg_type: "popup_response".to_string(),
            request_id: request.request_id,
            response,
        };

        self.send_message(&response_msg).await?;

        Ok(())
    }

    /// 处理弹窗响应(寸止作为客户端时使用)
    async fn handle_popup_response(&self, message: &serde_json::Value) -> Result<()> {
        let response: PopupResponseMessage = serde_json::from_value(message.clone())?;

        log_important!(info, "收到弹窗响应: {}", response.request_id);

        // 查找pending request并发送响应
        let mut pending = self.pending_requests.lock().await;
        if let Some(tx) = pending.remove(&response.request_id) {
            let _ = tx.send(response.response);
        } else {
            log_important!(warn, "未找到对应的pending request: {}", response.request_id);
        }

        Ok(())
    }

    /// 发送消息到服务器
    async fn send_message<T: serde::Serialize>(&self, message: &T) -> Result<()> {
        let json = serde_json::to_string(message)?;
        let mut conn = self.connection.lock().await;
        if let Some(write) = conn.as_mut() {
            write.send(Message::Text(json)).await?;
            Ok(())
        } else {
            anyhow::bail!("连接未建立")
        }
    }

    /// 检查是否已连接
    pub async fn is_connected(&self) -> bool {
        let status = self.status.lock().await;
        *status == ConnectionStatus::Connected
    }

    /// 发送弹窗请求并等待响应
    pub async fn send_popup_request(&self, request: &PopupRequest) -> Result<String> {
        // 检查连接状态
        if !self.is_connected().await {
            anyhow::bail!("WebSocket未连接");
        }

        // 生成请求ID
        let request_id = uuid::Uuid::new_v4().to_string();

        // 创建oneshot channel等待响应
        let (tx, rx) = oneshot::channel();
        {
            let mut pending = self.pending_requests.lock().await;
            pending.insert(request_id.clone(), tx);
        }

        // 构造弹窗请求消息
        let popup_msg = PopupRequestMessage {
            msg_type: "popup_request".to_string(),
            request_id: request_id.clone(),
            message: request.message.clone(),
            predefined_options: request.predefined_options.clone(),
            is_markdown: request.is_markdown,
        };

        let msg_json = serde_json::to_string(&popup_msg)?;
        let message = Message::Text(msg_json);

        // 发送请求
        {
            let mut conn = self.connection.lock().await;
            if let Some(write) = conn.as_mut() {
                write.send(message).await?;
                log_important!(info, "已发送弹窗请求: {}", request_id);
            } else {
                anyhow::bail!("WebSocket连接已断开");
            }
        }

        // 等待响应(30秒超时)
        match tokio::time::timeout(Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => {
                log_important!(info, "收到弹窗响应: {}", request_id);
                Ok(response)
            }
            Ok(Err(_)) => {
                anyhow::bail!("响应channel已关闭");
            }
            Err(_) => {
                // 超时,清理pending request
                let mut pending = self.pending_requests.lock().await;
                pending.remove(&request_id);
                anyhow::bail!("弹窗请求超时");
            }
        }
    }

    /// 心跳任务 - 定期检查pong超时
    async fn heartbeat_task(&self) {
        let ping_interval = Duration::from_secs(WEBSOCKET_PING_INTERVAL_SECS);
        let pong_timeout = Duration::from_secs(WEBSOCKET_PONG_TIMEOUT_SECS);

        loop {
            tokio::time::sleep(ping_interval).await;

            // 检查连接状态
            let is_connected = self.is_connected().await;
            if !is_connected {
                break;
            }

            // 检查pong超时
            {
                let last_pong = self.last_pong_time.lock().await;
                if last_pong.elapsed() > pong_timeout {
                    log_important!(warn, "服务器pong超时,断开连接");
                    let mut status = self.status.lock().await;
                    *status = ConnectionStatus::Error("pong超时".to_string());
                    break;
                }
            }
        }
    }
}

