use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Mutex, oneshot};
use tokio_tungstenite::{accept_async, tungstenite::Message};

use crate::mcp::types::PopupRequest;
use crate::log_important;

/// WebSocket客户端连接
struct WsClient {
    sender: futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<TcpStream>,
        Message,
    >,
}

/// WebSocket服务器状态
pub struct WsServer {
    clients: Arc<Mutex<HashMap<String, WsClient>>>,
    pending_requests: Arc<Mutex<HashMap<String, oneshot::Sender<String>>>>,
}

impl WsServer {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(HashMap::new())),
            pending_requests: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 启动WebSocket服务器
    pub async fn start(self: Arc<Self>) -> Result<()> {
        let addr = "0.0.0.0:9000";
        let listener = TcpListener::bind(addr).await?;
        log_important!(info, "WebSocket服务器启动: {}", addr);

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

        // 注册客户端
        {
            let mut clients = self.clients.lock().await;
            clients.insert(client_id.clone(), WsClient { sender: write });
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
    async fn handle_message(&self, _client_id: &str, text: &str) -> Result<()> {
        let response: serde_json::Value = serde_json::from_str(text)?;

        // 检查是否为弹窗响应
        if let Some(request_id) = response.get("request_id").and_then(|v| v.as_str()) {
            let mut pending = self.pending_requests.lock().await;
            if let Some(sender) = pending.remove(request_id) {
                let _ = sender.send(text.to_string());
            }
        }

        Ok(())
    }

    /// 发送弹窗请求到客户端
    pub async fn send_popup_request(&self, request: &PopupRequest) -> Result<String> {
        // 检查是否有在线客户端
        let client_id = {
            let clients = self.clients.lock().await;
            if clients.is_empty() {
                anyhow::bail!("没有在线的WebSocket客户端");
            }
            clients.keys().next().unwrap().clone()
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

        // 等待响应(30秒超时)
        match tokio::time::timeout(tokio::time::Duration::from_secs(30), rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => anyhow::bail!("响应通道关闭"),
            Err(_) => anyhow::bail!("等待响应超时"),
        }
    }

    /// 检查是否有在线客户端
    pub async fn has_clients(&self) -> bool {
        let clients = self.clients.lock().await;
        !clients.is_empty()
    }
}

