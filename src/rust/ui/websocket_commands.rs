use crate::config::{save_config, AppState};
use crate::mcp::types::PopupRequest;
use crate::log_important;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use once_cell::sync::OnceCell;

/// WebSocket客户端状态
#[derive(Debug, Clone)]
pub enum WebSocketStatus {
    Disconnected,
    Connecting,
    Connected,
    Error(String),
}

impl WebSocketStatus {
    pub fn as_string(&self) -> String {
        match self {
            WebSocketStatus::Disconnected => "disconnected".to_string(),
            WebSocketStatus::Connecting => "connecting".to_string(),
            WebSocketStatus::Connected => "connected".to_string(),
            WebSocketStatus::Error(_) => "error".to_string(),
        }
    }
}

/// WebSocket连接句柄
type WsConnection = (
    futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    tokio::task::JoinHandle<()>,
);

/// WebSocket客户端管理器
pub struct WebSocketManager {
    status: Arc<Mutex<WebSocketStatus>>,
    server_url: Arc<Mutex<Option<String>>>,
    connection: Arc<Mutex<Option<WsConnection>>>,
    app_handle: Arc<Mutex<Option<AppHandle>>>,
}

impl WebSocketManager {
    pub fn new() -> Self {
        Self {
            status: Arc::new(Mutex::new(WebSocketStatus::Disconnected)),
            server_url: Arc::new(Mutex::new(None)),
            connection: Arc::new(Mutex::new(None)),
            app_handle: Arc::new(Mutex::new(None)),
        }
    }

    /// 设置AppHandle（在应用启动后调用）
    pub async fn set_app_handle(&self, app_handle: AppHandle) {
        *self.app_handle.lock().await = Some(app_handle);
    }

    pub async fn get_status(&self) -> WebSocketStatus {
        self.status.lock().await.clone()
    }

    pub async fn set_status(&self, status: WebSocketStatus) {
        *self.status.lock().await = status;
    }

    pub async fn connect(&self, server_url: String) -> Result<()> {
        log_important!(info, "开始连接WebSocket服务器: {}", server_url);

        // 先断开现有连接
        self.disconnect().await?;

        // 设置连接中状态
        self.set_status(WebSocketStatus::Connecting).await;
        *self.server_url.lock().await = Some(server_url.clone());

        // 建立WebSocket连接
        let (ws_stream, _) = connect_async(&server_url).await
            .map_err(|e| anyhow::anyhow!("连接失败: {}", e))?;

        log_important!(info, "WebSocket连接成功");

        let (write, mut read) = ws_stream.split();

        // 获取AppHandle用于发送事件
        let app_handle = {
            let app_guard = self.app_handle.lock().await;
            app_guard.clone()
        };

        // 启动消息处理任务
        let status_arc = self.status.clone();
        let handle = tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        log_important!(info, "收到WebSocket消息: {}", text);

                        // 解析消息并处理
                        if let Err(e) = Self::handle_websocket_message(&text, &app_handle).await {
                            log_important!(warn, "处理WebSocket消息失败: {}", e);
                        }
                    }
                    Ok(Message::Close(_)) => {
                        log_important!(info, "WebSocket服务器关闭连接");
                        *status_arc.lock().await = WebSocketStatus::Disconnected;
                        break;
                    }
                    Err(e) => {
                        log_important!(warn, "WebSocket接收消息失败: {}", e);
                        *status_arc.lock().await = WebSocketStatus::Error(format!("接收失败: {}", e));
                        break;
                    }
                    _ => {}
                }
            }
        });

        // 保存连接句柄
        *self.connection.lock().await = Some((write, handle));
        self.set_status(WebSocketStatus::Connected).await;

        Ok(())
    }

    pub async fn disconnect(&self) -> Result<()> {
        log_important!(info, "断开WebSocket连接");

        // 关闭现有连接
        if let Some((mut write, handle)) = self.connection.lock().await.take() {
            // 发送关闭消息
            let _ = write.send(Message::Close(None)).await;
            // 取消任务
            handle.abort();
        }

        self.set_status(WebSocketStatus::Disconnected).await;
        *self.server_url.lock().await = None;

        log_important!(info, "WebSocket连接已断开");
        Ok(())
    }

    /// 处理WebSocket消息
    async fn handle_websocket_message(text: &str, app_handle: &Option<AppHandle>) -> Result<()> {
        // 解析消息
        let json: serde_json::Value = serde_json::from_str(text)?;

        // 检查消息类型
        if json.get("type").and_then(|v| v.as_str()) != Some("popup_request") {
            return Ok(());
        }

        // 提取请求数据
        let request_id = json
            .get("request_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("缺少request_id"))?;

        let message = json
            .get("message")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("缺少message"))?;

        let predefined_options = json
            .get("predefined_options")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            });

        let is_markdown = json
            .get("is_markdown")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 构建弹窗请求
        let popup_request = PopupRequest {
            id: request_id.to_string(),
            message: message.to_string(),
            predefined_options,
            is_markdown,
        };

        log_important!(info, "处理弹窗请求: {}", popup_request.message);

        // 发送事件到前端
        if let Some(app) = app_handle {
            app.emit("mcp-request", &popup_request)
                .map_err(|e| anyhow::anyhow!("发送事件失败: {}", e))?;
            log_important!(info, "已发送mcp-request事件到前端");
        } else {
            return Err(anyhow::anyhow!("AppHandle未设置"));
        }

        Ok(())
    }

    /// 发送响应到WebSocket服务器
    pub async fn send_response(&self, request_id: &str, response: &str) -> Result<()> {
        let mut connection_guard = self.connection.lock().await;

        if let Some((write, _)) = connection_guard.as_mut() {
            let response_json = serde_json::json!({
                "type": "popup_response",
                "request_id": request_id,
                "response": response,
            });

            write.send(Message::Text(response_json.to_string())).await
                .map_err(|e| anyhow::anyhow!("发送响应失败: {}", e))?;

            log_important!(info, "已发送WebSocket响应: {}", request_id);
            Ok(())
        } else {
            Err(anyhow::anyhow!("WebSocket未连接"))
        }
    }

    /// 检查是否已连接
    pub async fn is_connected(&self) -> bool {
        matches!(self.get_status().await, WebSocketStatus::Connected)
    }
}

/// 全局WebSocket管理器实例
static WEBSOCKET_MANAGER: OnceCell<WebSocketManager> = OnceCell::new();

/// 获取WebSocket管理器实例
pub fn get_websocket_manager() -> &'static WebSocketManager {
    WEBSOCKET_MANAGER.get_or_init(|| WebSocketManager::new())
}

/// 连接WebSocket服务器
#[tauri::command]
pub async fn connect_websocket(server_url: String) -> Result<(), String> {
    let manager = get_websocket_manager();
    
    match manager.connect(server_url).await {
        Ok(_) => Ok(()),
        Err(e) => {
            let error_msg = format!("连接失败: {}", e);
            manager.set_status(WebSocketStatus::Error(error_msg.clone())).await;
            Err(error_msg)
        }
    }
}

/// 断开WebSocket连接
#[tauri::command]
pub async fn disconnect_websocket() -> Result<(), String> {
    let manager = get_websocket_manager();
    
    match manager.disconnect().await {
        Ok(_) => Ok(()),
        Err(e) => Err(format!("断开连接失败: {}", e)),
    }
}

/// 获取WebSocket连接状态
#[tauri::command]
pub async fn get_websocket_status() -> Result<String, String> {
    let manager = get_websocket_manager();
    let status = manager.get_status().await;
    Ok(status.as_string())
}

/// 获取WebSocket配置
#[tauri::command]
pub async fn get_websocket_config(state: State<'_, AppState>) -> Result<serde_json::Value, String> {
    let config = state
        .config
        .lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    
    Ok(serde_json::json!({
        "enabled": config.websocket_config.enabled,
        "host": config.websocket_config.host,
        "port": config.websocket_config.port,
        "auto_connect": config.websocket_config.auto_connect,
    }))
}

/// 更新WebSocket配置
#[tauri::command]
pub async fn update_websocket_config(
    enabled: bool,
    host: String,
    port: u16,
    auto_connect: bool,
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    {
        let mut config = state
            .config
            .lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        
        config.websocket_config.enabled = enabled;
        config.websocket_config.host = host;
        config.websocket_config.port = port;
        config.websocket_config.auto_connect = auto_connect;
    }
    
    // 保存配置到文件
    save_config(&state, &app)
        .await
        .map_err(|e| format!("保存配置失败: {}", e))?;
    
    log_important!(info, "WebSocket配置已更新");
    Ok(())
}

/// 初始化WebSocket客户端（如果启用了自动连接）
pub async fn initialize_websocket_client(state: &State<'_, AppState>) -> Result<()> {
    let (enabled, host, port, auto_connect) = {
        let config = state
            .config
            .lock()
            .map_err(|e| anyhow::anyhow!("获取配置失败: {}", e))?;
        
        (
            config.websocket_config.enabled,
            config.websocket_config.host.clone(),
            config.websocket_config.port,
            config.websocket_config.auto_connect,
        )
    };
    
    if enabled && auto_connect {
        let server_url = format!("ws://{}:{}", host, port);
        log_important!(info, "自动连接WebSocket服务器: {}", server_url);
        
        let manager = get_websocket_manager();
        if let Err(e) = manager.connect(server_url).await {
            log_important!(warn, "自动连接WebSocket失败: {}", e);
        }
    }
    
    Ok(())
}
