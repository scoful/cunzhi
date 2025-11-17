use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{accept_async, tungstenite::Message};
use std::time::{Duration, Instant};
use serde_json::json;
use tauri::Emitter;

use crate::log_important;
use crate::constants::network::{
    WEBSOCKET_PING_INTERVAL_SECS,
    WEBSOCKET_PONG_TIMEOUT_SECS,
};

/// 发送WebSocket日志事件到前端
fn emit_ws_log(client_id: &str, log_type: &str, message: &str) {
    if let Some(app) = crate::lian_yi_xia::get_app_handle() {
        let _ = app.emit("ws_log", json!({
            "type": log_type,
            "server_name": client_id,
            "message": message
        }));
    }
}

/// 客户端连接信息
struct ClientConnection {
    #[allow(dead_code)]
    client_id: String,
    write: Arc<Mutex<futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >>>,
    last_pong_time: Arc<Mutex<Instant>>,
    connected_at: Instant,
}

/// WebSocket服务器状态
#[derive(Clone, Debug)]
pub enum ServerStatus {
    Running,
    Error(String),
}

/// WebSocket服务器
pub struct LianYiXiaWsServer {
    clients: Arc<Mutex<HashMap<String, ClientConnection>>>,
    port: u16,
    status: Arc<Mutex<ServerStatus>>,
    start_time: Arc<Mutex<Option<Instant>>>,
}

impl LianYiXiaWsServer {
    pub fn new() -> Self {
        // 从环境变量读取端口,默认9000
        let port = std::env::var("LIAN_YI_XIA_PORT")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(9000);

        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            port,
            status: Arc::new(Mutex::new(ServerStatus::Running)),
            start_time: Arc::new(Mutex::new(None)),
        }
    }

    /// 启动WebSocket服务器
    pub async fn start(self: Arc<Self>) -> Result<()> {
        let addr = format!("127.0.0.1:{}", self.port);

        // 尝试绑定端口
        let listener = match TcpListener::bind(&addr).await {
            Ok(l) => {
                // 启动成功,更新状态
                *self.status.lock().await = ServerStatus::Running;
                *self.start_time.lock().await = Some(Instant::now());
                log_important!(info, "WebSocket服务器启动: {}", addr);
                emit_ws_log("系统", "success", &format!("服务器启动: {}", addr));
                l
            }
            Err(e) => {
                // 启动失败,更新状态
                let error_msg = format!("端口{}绑定失败: {}", self.port, e);
                *self.status.lock().await = ServerStatus::Error(error_msg.clone());
                log_important!(error, "WebSocket服务器启动失败: {}", error_msg);
                emit_ws_log("系统", "error", &format!("服务器启动失败: {}", error_msg));
                return Err(e.into());
            }
        };

        // 启动心跳任务
        let server_for_heartbeat = self.clone();
        tokio::spawn(async move {
            server_for_heartbeat.heartbeat_task().await;
        });

        // 接受客户端连接
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    log_important!(info, "新连接: {}", addr);
                    let server = self.clone();
                    tokio::spawn(async move {
                        if let Err(e) = server.handle_client(stream).await {
                            log_important!(warn, "处理客户端失败: {}", e);
                        }
                    });
                }
                Err(e) => {
                    log_important!(error, "接受连接失败: {}", e);
                }
            }
        }
    }

    /// 处理单个客户端连接
    async fn handle_client(&self, stream: TcpStream) -> Result<()> {
        let ws_stream = accept_async(stream).await?;
        let (mut write, mut read) = ws_stream.split();

        // 等待注册消息
        let client_id = match read.next().await {
            Some(Ok(Message::Text(text))) => {
                match self.handle_register(&text).await {
                    Ok(id) => {
                        // 发送注册成功响应
                        let ack = json!({
                            "type": "register_ack",
                            "message": "注册成功"
                        });
                        write.send(Message::Text(ack.to_string())).await?;
                        log_important!(info, "客户端注册成功: {}", id);
                        emit_ws_log(&id, "success", "客户端已连接");
                        id
                    }
                    Err(e) => {
                        // 发送注册失败响应
                        let error = json!({
                            "type": "register_error",
                            "error": e.to_string()
                        });
                        write.send(Message::Text(error.to_string())).await?;
                        return Err(e);
                    }
                }
            }
            _ => {
                anyhow::bail!("未收到注册消息");
            }
        };

        // 保存客户端连接
        let write_arc = Arc::new(Mutex::new(write));
        let client_conn = ClientConnection {
            client_id: client_id.clone(),
            write: write_arc.clone(),
            last_pong_time: Arc::new(Mutex::new(Instant::now())),
            connected_at: Instant::now(),
        };

        // 检查是否有同CLIENT_ID的旧连接,如果有则清理
        {
            let mut clients = self.clients.lock().await;
            if let Some(old_conn) = clients.remove(&client_id) {
                log_important!(warn, "清理旧连接: {}", client_id);
                emit_ws_log(&client_id, "warning", "检测到重复连接,已清理旧连接");
                // 旧连接会在drop时自动关闭
                drop(old_conn);
            }
            clients.insert(client_id.clone(), client_conn);
        }

        // 处理消息
        while let Some(msg) = read.next().await {
            match msg {
                Ok(Message::Text(text)) => {
                    if let Err(e) = self.handle_message(&client_id, &text).await {
                        log_important!(warn, "[{}] 处理消息失败: {}", client_id, e);
                        emit_ws_log(&client_id, "error", &format!("处理消息失败: {}", e));
                    }
                }
                Ok(Message::Pong(_)) => {
                    // 更新pong时间
                    if let Some(client) = self.clients.lock().await.get(&client_id) {
                        *client.last_pong_time.lock().await = Instant::now();
                        log_important!(info, "[{}] 收到pong响应", client_id);
                        emit_ws_log(&client_id, "success", "💚 收到心跳响应");
                    }
                }
                Ok(Message::Close(_)) => {
                    log_important!(info, "[{}] 客户端断开连接", client_id);
                    emit_ws_log(&client_id, "info", "客户端已断开");
                    break;
                }
                Err(e) => {
                    log_important!(warn, "[{}] 连接错误: {}", client_id, e);
                    emit_ws_log(&client_id, "error", &format!("连接错误: {}", e));
                    break;
                }
                _ => {}
            }
        }

        // 清理客户端连接
        self.clients.lock().await.remove(&client_id);
        log_important!(info, "[{}] 客户端已移除", client_id);

        Ok(())
    }

    /// 处理注册消息
    async fn handle_register(&self, text: &str) -> Result<String> {
        let json: serde_json::Value = serde_json::from_str(text)?;

        let msg_type = json.get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("消息缺少type字段"))?;

        if msg_type != "register" {
            anyhow::bail!("期望register消息,收到: {}", msg_type);
        }

        let client_id = json.get("client_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("缺少client_id字段"))?
            .to_string();

        Ok(client_id)
    }

    /// 处理客户端消息
    async fn handle_message(&self, client_id: &str, text: &str) -> Result<()> {
        let json: serde_json::Value = serde_json::from_str(text)?;

        let msg_type = json.get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("消息缺少type字段"))?;

        match msg_type {
            "popup_request" => {
                // 弹窗请求(从寸止接收)
                log_important!(info, "[{}] 收到弹窗请求", client_id);
                emit_ws_log(client_id, "info", "← 收到弹窗请求");

                // 启动本地"等一下"处理请求
                let client_id_clone = client_id.to_string();
                let json_clone = json.clone();
                let clients = self.clients.clone();

                tokio::spawn(async move {
                    match super::deng_yi_xia_launcher::launch_deng_yi_xia(&json_clone, &client_id_clone).await {
                        Ok(response) => {
                            log_important!(info, "[{}] 等一下返回响应", client_id_clone);
                            emit_ws_log(&client_id_clone, "success", "✓ 用户已响应");

                            // 发送响应回寸止
                            let request_id = json_clone.get("request_id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown");

                            let response_msg = json!({
                                "type": "popup_response",
                                "request_id": request_id,
                                "response": response,
                            });

                            // 发送响应
                            if let Some(client) = clients.lock().await.get(&client_id_clone) {
                                if let Err(e) = client.write.lock().await.send(Message::Text(response_msg.to_string())).await {
                                    log_important!(error, "[{}] 发送响应失败: {}", client_id_clone, e);
                                    emit_ws_log(&client_id_clone, "error", "✗ 发送响应失败");
                                } else {
                                    log_important!(info, "[{}] 已发送响应", client_id_clone);
                                    emit_ws_log(&client_id_clone, "info", "→ 已发送响应");
                                }
                            }
                        }
                        Err(e) => {
                            log_important!(error, "[{}] 等一下失败: {}", client_id_clone, e);
                            emit_ws_log(&client_id_clone, "error", &format!("✗ 等一下失败: {}", e));
                        }
                    }
                });
            }
            _ => {
                log_important!(warn, "[{}] 未知消息类型: {}", client_id, msg_type);
            }
        }

        Ok(())
    }

    /// 获取服务器状态信息
    pub async fn get_status_info(&self) -> (String, String, String, usize) {
        let status = self.status.lock().await;
        let start_time = self.start_time.lock().await;
        let clients = self.clients.lock().await;

        let status_str = match &*status {
            ServerStatus::Running => "running".to_string(),
            ServerStatus::Error(e) => format!("error: {}", e),
        };

        let addr = format!("127.0.0.1:{}", self.port);

        let uptime = if let Some(start) = *start_time {
            let duration = start.elapsed();
            let total_secs = duration.as_secs();
            let hours = total_secs / 3600;
            let minutes = (total_secs % 3600) / 60;
            let seconds = total_secs % 60;

            if hours > 0 {
                format!("{}小时{}分钟", hours, minutes)
            } else if minutes > 0 {
                format!("{}分钟{}秒", minutes, seconds)
            } else {
                format!("{}秒", seconds)
            }
        } else {
            "未启动".to_string()
        };

        let client_count = clients.len();

        (status_str, addr, uptime, client_count)
    }

    /// 心跳任务
    async fn heartbeat_task(&self) {
        let ping_interval = Duration::from_secs(WEBSOCKET_PING_INTERVAL_SECS);
        let pong_timeout = Duration::from_secs(WEBSOCKET_PONG_TIMEOUT_SECS);

        log_important!(info, "心跳任务已启动,间隔{}秒", WEBSOCKET_PING_INTERVAL_SECS);

        loop {
            tokio::time::sleep(ping_interval).await;

            let mut clients_to_remove = Vec::new();
            let clients = self.clients.lock().await;

            log_important!(info, "心跳检查: 当前客户端数量 {}", clients.len());

            for (client_id, client) in clients.iter() {
                // 发送ping
                if let Err(e) = client.write.lock().await.send(Message::Ping(vec![])).await {
                    log_important!(warn, "[{}] 发送ping失败: {}", client_id, e);
                    emit_ws_log(client_id, "error", "❌ 发送心跳失败");
                    clients_to_remove.push(client_id.clone());
                    continue;
                } else {
                    log_important!(info, "[{}] 已发送ping", client_id);
                    emit_ws_log(client_id, "info", "💓 发送心跳");
                }

                // 检查pong超时
                let last_pong = client.last_pong_time.lock().await;
                if last_pong.elapsed() > pong_timeout {
                    log_important!(warn, "[{}] pong超时,移除客户端", client_id);
                    emit_ws_log(client_id, "warning", "⚠️ 心跳超时,连接已断开");
                    clients_to_remove.push(client_id.clone());
                }
            }

            drop(clients);

            // 移除超时客户端
            if !clients_to_remove.is_empty() {
                let mut clients = self.clients.lock().await;
                for client_id in clients_to_remove {
                    clients.remove(&client_id);
                }
            }
        }
    }

    /// 获取所有已连接客户端
    pub async fn get_connected_clients(&self) -> Vec<(String, Instant)> {
        let clients = self.clients.lock().await;
        clients.iter()
            .map(|(id, conn)| (id.clone(), conn.connected_at))
            .collect()
    }
}

