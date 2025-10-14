use crate::lian_yi_xia::types::{ConnectionStatus, WebSocketServerConfig};
use crate::log_important;
use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use std::process::Command;
use std::fs;

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

/// 单个WebSocket连接
struct SingleConnection {
    config: WebSocketServerConfig,
    status: Arc<Mutex<ConnectionStatus>>,
    connection: Arc<Mutex<Option<WsConnection>>>,
}

impl SingleConnection {
    fn new(config: WebSocketServerConfig) -> Self {
        Self {
            config,
            status: Arc::new(Mutex::new(ConnectionStatus::Disconnected)),
            connection: Arc::new(Mutex::new(None)),
        }
    }

    async fn get_status(&self) -> ConnectionStatus {
        self.status.lock().await.clone()
    }

    async fn set_status(&self, status: ConnectionStatus) {
        *self.status.lock().await = status;
    }

    /// 连接到WebSocket服务器
    async fn connect(&self) -> Result<()> {
        let server_url = format!("ws://{}:{}", self.config.host, self.config.port);
        log_important!(info, "[{}] 开始连接WebSocket服务器: {}", self.config.name, server_url);

        // 先断开现有连接
        self.disconnect().await?;

        // 设置连接中状态
        self.set_status(ConnectionStatus::Connecting).await;

        // 建立WebSocket连接
        let (ws_stream, _) = match connect_async(&server_url).await {
            Ok(stream) => stream,
            Err(e) => {
                let error_msg = format!("连接失败: {}", e);
                log_important!(warn, "[{}] {}", self.config.name, error_msg);
                // 连接失败时设置错误状态
                self.set_status(ConnectionStatus::Error(error_msg.clone())).await;
                return Err(anyhow::anyhow!(error_msg));
            }
        };

        log_important!(info, "[{}] WebSocket连接成功", self.config.name);

        let (mut write, mut read) = ws_stream.split();

        // 发送认证消息
        if !self.config.api_key.is_empty() {
            let auth_msg = serde_json::json!({
                "type": "auth",
                "api_key": self.config.api_key
            });
            if let Err(e) = write.send(Message::Text(auth_msg.to_string())).await {
                let error_msg = format!("发送认证消息失败: {}", e);
                log_important!(warn, "[{}] {}", self.config.name, error_msg);
                // 认证失败时设置错误状态
                self.set_status(ConnectionStatus::Error(error_msg.clone())).await;
                return Err(anyhow::anyhow!(error_msg));
            }
            log_important!(info, "[{}] 已发送认证消息", self.config.name);
        }

        // 启动消息处理任务
        let status_arc = self.status.clone();
        let server_name = self.config.name.clone();
        let connection_arc = self.connection.clone();
        let handle = tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        log_important!(info, "[{}] 收到WebSocket消息: {}", server_name, text);

                        // 处理消息，启动"等一下"实例
                        if let Err(e) = Self::handle_message(&text, &server_name, &connection_arc).await {
                            log_important!(warn, "[{}] 处理WebSocket消息失败: {}", server_name, e);
                        }
                    }
                    Ok(Message::Close(_)) => {
                        log_important!(info, "[{}] WebSocket服务器关闭连接", server_name);
                        *status_arc.lock().await = ConnectionStatus::Disconnected;
                        break;
                    }
                    Err(e) => {
                        log_important!(warn, "[{}] WebSocket接收消息失败: {}", server_name, e);
                        *status_arc.lock().await = ConnectionStatus::Error(format!("接收失败: {}", e));
                        break;
                    }
                    _ => {}
                }
            }
        });

        // 保存连接句柄
        *self.connection.lock().await = Some((write, handle));
        self.set_status(ConnectionStatus::Connected).await;

        Ok(())
    }

    /// 断开连接
    async fn disconnect(&self) -> Result<()> {
        log_important!(info, "[{}] 断开WebSocket连接", self.config.name);

        // 关闭现有连接
        if let Some((mut write, handle)) = self.connection.lock().await.take() {
            // 发送关闭消息
            let _ = write.send(Message::Close(None)).await;
            // 取消任务
            handle.abort();
        }

        self.set_status(ConnectionStatus::Disconnected).await;

        log_important!(info, "[{}] WebSocket连接已断开", self.config.name);
        Ok(())
    }

    /// 处理接收到的消息
    async fn handle_message(
        text: &str,
        server_name: &str,
        connection_arc: &Arc<Mutex<Option<WsConnection>>>
    ) -> Result<()> {
        // 解析JSON消息
        let json: serde_json::Value = serde_json::from_str(text)?;

        // 检查消息类型
        let msg_type = json.get("type")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("消息缺少type字段"))?;

        match msg_type {
            "auth_response" => {
                // 认证响应
                let success = json.get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if success {
                    log_important!(info, "[{}] 认证成功", server_name);
                } else {
                    let error = json.get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("未知错误");
                    log_important!(warn, "[{}] 认证失败: {}", server_name, error);
                }
            }
            "popup_request" => {
                // 弹窗请求
                log_important!(info, "[{}] 收到弹窗请求", server_name);

                // 提取request_id
                let request_id = json.get("request_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());

                // 启动"等一下"实例处理弹窗
                match Self::launch_deng_yi_xia(&json, server_name).await {
                    Ok(response) => {
                        // 添加request_id到响应中
                        match Self::add_request_id_to_response(&response, request_id) {
                            Ok(final_response) => {
                                // 发送响应回服务器
                                if let Err(e) = Self::send_response(connection_arc, &final_response).await {
                                    log_important!(warn, "[{}] 发送响应失败: {}", server_name, e);
                                }
                            }
                            Err(e) => {
                                log_important!(warn, "[{}] 处理响应失败: {}", server_name, e);
                            }
                        }
                    }
                    Err(e) => {
                        log_important!(warn, "[{}] 启动等一下实例失败: {}", server_name, e);
                    }
                }
            }
            _ => {
                log_important!(warn, "[{}] 未知消息类型: {}", server_name, msg_type);
            }
        }

        Ok(())
    }

    /// 添加request_id到响应JSON中
    fn add_request_id_to_response(response: &str, request_id: Option<String>) -> Result<String> {
        // 解析响应JSON
        let mut response_json: serde_json::Value = serde_json::from_str(response)?;

        // 添加request_id到顶层
        if let Some(id) = request_id {
            response_json["request_id"] = serde_json::Value::String(id);
            log_important!(info, "已添加request_id到响应: {}", response_json["request_id"]);
        } else {
            log_important!(warn, "原始请求缺少request_id,响应可能无法正确路由");
        }

        // 转换回JSON字符串
        Ok(serde_json::to_string(&response_json)?)
    }

    /// 发送响应消息到WebSocket服务器
    async fn send_response(
        connection_arc: &Arc<Mutex<Option<WsConnection>>>,
        response: &str
    ) -> Result<()> {
        let mut connection_guard = connection_arc.lock().await;

        if let Some((write, _)) = connection_guard.as_mut() {
            let response_msg = Message::Text(response.to_string());
            write.send(response_msg).await?;
            log_important!(info, "已发送响应到WebSocket服务器: {}", response);
        } else {
            anyhow::bail!("WebSocket连接不可用");
        }

        Ok(())
    }

    /// 启动"等一下"实例处理弹窗请求
    async fn launch_deng_yi_xia(json: &serde_json::Value, server_name: &str) -> Result<String> {
        // 提取请求ID
        let request_id = json.get("request_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("缺少request_id"))?;

        // 创建临时请求文件
        let temp_dir = std::env::temp_dir();
        let temp_file = temp_dir.join(format!("mcp_request_{}.json", request_id));
        let request_json = serde_json::to_string_pretty(json)?;
        fs::write(&temp_file, &request_json)?;

        log_important!(info, "[{}] 创建临时请求文件: {}", server_name, temp_file.display());

        // 查找"等一下"命令路径
        let command_path = Self::find_deng_yi_xia_command()?;
        log_important!(info, "[{}] 使用等一下命令: {}", server_name, command_path);

        // 调用"等一下"命令
        let output = Command::new(&command_path)
            .arg("--mcp-request")
            .arg(temp_file.to_string_lossy().to_string())
            .output()?;

        // 清理临时文件
        let _ = fs::remove_file(&temp_file);

        if output.status.success() {
            let response = String::from_utf8_lossy(&output.stdout);
            let response_str = response.trim();
            log_important!(info, "[{}] 等一下实例返回: {}", server_name, response_str);

            // 返回响应内容
            if response_str.is_empty() {
                Ok("用户取消了操作".to_string())
            } else {
                Ok(response_str.to_string())
            }
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            log_important!(warn, "[{}] 等一下实例失败: {}", server_name, error);
            anyhow::bail!("等一下实例失败: {}", error)
        }
    }

    /// 查找"等一下"命令路径
    fn find_deng_yi_xia_command() -> Result<String> {
        // 1. 优先尝试与当前"连一下"同目录的"等一下"命令
        if let Ok(current_exe) = std::env::current_exe() {
            if let Some(exe_dir) = current_exe.parent() {
                let local_ui_path = exe_dir.join("等一下");
                if local_ui_path.exists() && Self::is_executable(&local_ui_path) {
                    return Ok(local_ui_path.to_string_lossy().to_string());
                }
            }
        }

        // 2. 尝试全局命令
        if Self::test_command_available("等一下") {
            return Ok("等一下".to_string());
        }

        // 3. 找不到则返回错误
        anyhow::bail!(
            "找不到等一下命令。请确保：\n\
             1. 已编译项目：cargo build --release\n\
             2. 或已全局安装\n\
             3. 或等一下命令在同目录下"
        )
    }

    /// 测试命令是否可用
    fn test_command_available(command: &str) -> bool {
        Command::new(command)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// 检查文件是否可执行
    fn is_executable(path: &std::path::Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            path.metadata()
                .map(|metadata| metadata.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        }

        #[cfg(windows)]
        {
            // Windows 上检查文件扩展名
            path.extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| ext.eq_ignore_ascii_case("exe"))
                .unwrap_or(false)
        }
    }
}

/// "连一下"WebSocket连接管理器
pub struct LianYiXiaWebSocketManager {
    connections: Arc<Mutex<HashMap<String, SingleConnection>>>,
}

impl LianYiXiaWebSocketManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// 添加服务器配置（不自动连接）
    pub async fn add_server(&self, config: WebSocketServerConfig) -> Result<()> {
        let server_id = config.id.clone();
        let connection = SingleConnection::new(config);

        // 添加到连接池（不自动连接）
        self.connections.lock().await.insert(server_id, connection);

        Ok(())
    }

    /// 添加服务器并尝试自动连接（用于启动时）
    pub async fn add_server_with_auto_connect(&self, config: WebSocketServerConfig) -> Result<()> {
        let server_id = config.id.clone();
        let should_connect = config.enabled && config.auto_connect;

        let connection = SingleConnection::new(config.clone());

        // 如果启用且自动连接，则尝试连接（失败不影响添加）
        if should_connect {
            if let Err(e) = connection.connect().await {
                log::warn!("[{}] 自动连接失败: {}", config.name, e);
            }
        }

        // 添加到连接池
        self.connections.lock().await.insert(server_id, connection);

        Ok(())
    }

    /// 移除服务器
    pub async fn remove_server(&self, server_id: &str) -> Result<()> {
        if let Some(connection) = self.connections.lock().await.remove(server_id) {
            connection.disconnect().await?;
        }
        Ok(())
    }

    /// 连接到指定服务器
    pub async fn connect_server(&self, server_id: &str) -> Result<()> {
        let connections = self.connections.lock().await;
        let connection = connections.get(server_id)
            .ok_or_else(|| anyhow::anyhow!("服务器不存在: {}", server_id))?;
        
        connection.connect().await
    }

    /// 断开指定服务器
    pub async fn disconnect_server(&self, server_id: &str) -> Result<()> {
        let connections = self.connections.lock().await;
        let connection = connections.get(server_id)
            .ok_or_else(|| anyhow::anyhow!("服务器不存在: {}", server_id))?;
        
        connection.disconnect().await
    }

    /// 获取指定服务器的连接状态
    pub async fn get_server_status(&self, server_id: &str) -> Result<ConnectionStatus> {
        let connections = self.connections.lock().await;
        let connection = connections.get(server_id)
            .ok_or_else(|| anyhow::anyhow!("服务器不存在: {}", server_id))?;
        
        Ok(connection.get_status().await)
    }

    /// 获取所有服务器的连接状态
    pub async fn get_all_status(&self) -> HashMap<String, ConnectionStatus> {
        let connections = self.connections.lock().await;
        let mut status_map = HashMap::new();

        for (server_id, connection) in connections.iter() {
            status_map.insert(server_id.clone(), connection.get_status().await);
        }

        status_map
    }

    /// 更新服务器配置
    pub async fn update_server(&self, config: WebSocketServerConfig) -> Result<()> {
        let server_id = config.id.clone();

        // 先移除旧连接
        self.remove_server(&server_id).await?;

        // 添加新配置
        self.add_server(config).await?;

        Ok(())
    }

    /// 断开所有WebSocket连接
    pub async fn disconnect_all(&self) -> Result<()> {
        log_important!(info, "开始断开所有WebSocket连接");

        let connections = self.connections.lock().await;
        let mut disconnect_tasks = Vec::new();

        for (_server_id, connection) in connections.iter() {
            let connection_clone = connection.connection.clone();
            let status_clone = connection.status.clone();
            let server_name = connection.config.name.clone();

            let task = tokio::spawn(async move {
                // 关闭连接
                if let Some((mut write, handle)) = connection_clone.lock().await.take() {
                    let _ = write.send(Message::Close(None)).await;
                    handle.abort();
                    log_important!(info, "[{}] WebSocket连接已断开", server_name);
                }

                // 更新状态
                *status_clone.lock().await = ConnectionStatus::Disconnected;
            });

            disconnect_tasks.push(task);
        }

        // 等待所有断开任务完成
        for task in disconnect_tasks {
            let _ = task.await;
        }

        log_important!(info, "所有WebSocket连接已断开");
        Ok(())
    }
}

