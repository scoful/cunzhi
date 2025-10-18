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
use serde_json::json;
use tauri::Emitter;
use std::time::{Duration, Instant};
use crate::constants::network::{
    WEBSOCKET_PING_TIMEOUT_SECS,
    WEBSOCKET_RECONNECT_INITIAL_DELAY_SECS,
    WEBSOCKET_RECONNECT_MAX_DELAY_SECS,
};

/// 发送WebSocket日志事件到前端
fn emit_ws_log(server_name: &str, log_type: &str, message: &str) {
    if let Some(app) = crate::lian_yi_xia::get_app_handle() {
        let _ = app.emit("ws_log", json!({
            "type": log_type,
            "server_name": server_name,
            "message": message
        }));
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

/// 单个WebSocket连接
struct SingleConnection {
    config: WebSocketServerConfig,
    status: Arc<Mutex<ConnectionStatus>>,
    connection: Arc<Mutex<Option<WsConnection>>>,
    last_ping_time: Arc<Mutex<Instant>>, // 最后收到ping的时间
    should_reconnect: Arc<Mutex<bool>>, // 是否应该自动重连
}

impl SingleConnection {
    fn new(config: WebSocketServerConfig) -> Self {
        Self {
            config,
            status: Arc::new(Mutex::new(ConnectionStatus::Disconnected)),
            connection: Arc::new(Mutex::new(None)),
            last_ping_time: Arc::new(Mutex::new(Instant::now())),
            should_reconnect: Arc::new(Mutex::new(false)),
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
                // 使用友好的错误提示
                let error_msg = if let Some(friendly_msg) = Self::get_friendly_error_message(&e) {
                    friendly_msg
                } else {
                    format!("连接失败: {}", e)
                };

                log_important!(error, "[{}] {}", self.config.name, error_msg);
                emit_ws_log(&self.config.name, "error", &error_msg);
                // 连接失败时设置错误状态
                self.set_status(ConnectionStatus::Error(error_msg.clone())).await;
                return Err(anyhow::anyhow!(error_msg));
            }
        };

        log_important!(info, "[{}] WebSocket连接成功", self.config.name);
        emit_ws_log(&self.config.name, "success", "连接成功");

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
                emit_ws_log(&self.config.name, "error", &error_msg);
                // 认证失败时设置错误状态
                self.set_status(ConnectionStatus::Error(error_msg.clone())).await;
                return Err(anyhow::anyhow!(error_msg));
            }
            log_important!(info, "[{}] 已发送认证消息", self.config.name);
            emit_ws_log(&self.config.name, "info", "→ 发送认证消息");
        }

        // 重置心跳时间并启用自动重连
        *self.last_ping_time.lock().await = Instant::now();
        *self.should_reconnect.lock().await = true;

        // 启动消息处理任务
        let status_arc = self.status.clone();
        let server_name = self.config.name.clone();
        let connection_arc = self.connection.clone();
        let last_ping_time_arc = self.last_ping_time.clone();
        let handle = tokio::spawn(async move {
            while let Some(msg) = read.next().await {
                match msg {
                    Ok(Message::Text(text)) => {
                        log_important!(info, "[{}] 收到WebSocket消息: {}", server_name, text);

                        // 解析消息类型并记录日志
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                            if let Some(msg_type) = json.get("type").and_then(|v| v.as_str()) {
                                match msg_type {
                                    "auth_response" => {
                                        let success = json.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                                        if success {
                                            emit_ws_log(&server_name, "success", "← 认证成功");
                                        } else {
                                            emit_ws_log(&server_name, "error", "← 认证失败");
                                        }
                                    }
                                    "popup_request" => {
                                        emit_ws_log(&server_name, "info", "← 收到弹窗请求");
                                    }
                                    _ => {
                                        emit_ws_log(&server_name, "info", &format!("← 收到消息: {}", msg_type));
                                    }
                                }
                            }
                        }

                        // 处理消息，启动"等一下"实例
                        if let Err(e) = Self::handle_message(&text, &server_name, &connection_arc).await {
                            log_important!(warn, "[{}] 处理WebSocket消息失败: {}", server_name, e);
                            emit_ws_log(&server_name, "error", &format!("处理消息失败: {}", e));
                        }
                    }
                    Ok(Message::Ping(_)) => {
                        // 收到ping，更新最后ping时间
                        *last_ping_time_arc.lock().await = Instant::now();
                    }
                    Ok(Message::Close(_)) => {
                        log_important!(info, "[{}] WebSocket服务器关闭连接", server_name);
                        emit_ws_log(&server_name, "info", "连接已关闭");
                        *status_arc.lock().await = ConnectionStatus::Disconnected;
                        break;
                    }
                    Err(e) => {
                        // 使用友好的错误提示
                        let error_msg = if let Some(friendly_msg) = Self::get_friendly_error_message(&e) {
                            friendly_msg
                        } else {
                            format!("连接错误: {}", e)
                        };

                        log_important!(error, "[{}] {}", server_name, error_msg);
                        emit_ws_log(&server_name, "error", &error_msg);
                        *status_arc.lock().await = ConnectionStatus::Error(error_msg);
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

        // 禁用自动重连(手动断开)
        *self.should_reconnect.lock().await = false;

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

    /// 获取友好的错误提示(包含技术原因)
    fn get_friendly_error_message(e: &tokio_tungstenite::tungstenite::Error) -> Option<String> {
        if let tokio_tungstenite::tungstenite::Error::Io(io_err) = e {
            match io_err.kind() {
                // 10061 - 连接被拒绝
                std::io::ErrorKind::ConnectionRefused => {
                    Some("连接被拒绝,请检查WebSocket配置是否正确".to_string())
                }
                // 10054 - 连接被重置
                std::io::ErrorKind::ConnectionReset => {
                    Some("连接被重置,远程服务器已断开,请检查".to_string())
                }
                // 10053 - 连接被中止
                std::io::ErrorKind::ConnectionAborted => {
                    Some("连接被中止,远程服务器已断开,请检查".to_string())
                }
                // 109 - 管道断开
                std::io::ErrorKind::BrokenPipe => {
                    Some("管道断开,远程服务器已断开,请检查".to_string())
                }
                _ => None
            }
        } else if matches!(e, tokio_tungstenite::tungstenite::Error::ConnectionClosed) {
            Some("连接已关闭,远程服务器已断开,请检查".to_string())
        } else {
            None
        }
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
                                if let Err(e) = Self::send_response(connection_arc, &final_response, server_name).await {
                                    log_important!(warn, "[{}] 发送响应失败: {}", server_name, e);
                                    emit_ws_log(server_name, "error", &format!("发送响应失败: {}", e));
                                }
                            }
                            Err(e) => {
                                log_important!(warn, "[{}] 处理响应失败: {}", server_name, e);
                                emit_ws_log(server_name, "error", &format!("处理响应失败: {}", e));
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
        response: &str,
        server_name: &str
    ) -> Result<()> {
        let mut connection_guard = connection_arc.lock().await;

        if let Some((write, _)) = connection_guard.as_mut() {
            let response_msg = Message::Text(response.to_string());
            write.send(response_msg).await?;
            log_important!(info, "[{}] 已发送响应到WebSocket服务器: {}", server_name, response);
            emit_ws_log(server_name, "info", "→ 发送用户响应");
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
    heartbeat_tasks: Arc<Mutex<HashMap<String, tokio::task::JoinHandle<()>>>>, // 心跳任务句柄
}

impl LianYiXiaWebSocketManager {
    pub fn new() -> Self {
        Self {
            connections: Arc::new(Mutex::new(HashMap::new())),
            heartbeat_tasks: Arc::new(Mutex::new(HashMap::new())),
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

        // 添加到连接池
        self.connections.lock().await.insert(server_id.clone(), connection);

        // 如果启用且自动连接，则尝试连接（失败不影响添加）
        if should_connect {
            if let Err(e) = self.connect_server(&server_id).await {
                log::warn!("[{}] 自动连接失败: {}", config.name, e);
            }
        }

        Ok(())
    }

    /// 移除服务器
    pub async fn remove_server(&self, server_id: &str) -> Result<()> {
        // 停止心跳任务
        if let Some(handle) = self.heartbeat_tasks.lock().await.remove(server_id) {
            handle.abort();
        }

        // 断开并移除连接
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

        connection.connect().await?;
        drop(connections); // 释放锁

        // 启动心跳任务
        self.start_heartbeat_task(server_id).await;

        Ok(())
    }

    /// 断开指定服务器
    pub async fn disconnect_server(&self, server_id: &str) -> Result<()> {
        // 停止心跳任务
        if let Some(handle) = self.heartbeat_tasks.lock().await.remove(server_id) {
            handle.abort();
        }

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

    /// 启动心跳检测和自动重连任务
    async fn start_heartbeat_task(&self, server_id: &str) {
        let server_id_owned = server_id.to_string();
        let server_id_for_task = server_id_owned.clone();
        let connections = self.connections.clone();
        let manager = self.clone_for_heartbeat();

        let handle = tokio::spawn(async move {
            let server_id = server_id_for_task;
            let ping_timeout = Duration::from_secs(WEBSOCKET_PING_TIMEOUT_SECS);
            let check_interval = Duration::from_secs(10); // 每10秒检查一次

            loop {
                tokio::time::sleep(check_interval).await;

                // 获取连接
                let connections_guard = connections.lock().await;
                let connection = match connections_guard.get(&server_id) {
                    Some(conn) => conn,
                    None => {
                        log_important!(info, "[{}] 连接已移除，退出心跳任务", server_id);
                        break;
                    }
                };

                // 检查是否应该继续运行
                if !*connection.should_reconnect.lock().await {
                    log_important!(info, "[{}] 自动重连已禁用，退出心跳任务", connection.config.name);
                    break;
                }

                // 检查当前状态
                let status = connection.get_status().await;
                let server_name = connection.config.name.clone();

                drop(connections_guard); // 释放锁

                match status {
                    ConnectionStatus::Connected => {
                        // 检查ping超时
                        let connections_guard = connections.lock().await;
                        if let Some(connection) = connections_guard.get(&server_id) {
                            let last_ping = *connection.last_ping_time.lock().await;
                            if last_ping.elapsed() > ping_timeout {
                                log_important!(warn, "[{}] 心跳超时，触发重连", server_name);
                                emit_ws_log(&server_name, "warn", "心跳超时，正在重连...");
                                drop(connections_guard);

                                // 触发重连
                                manager.reconnect_with_backoff(&server_id).await;
                                // 重连成功后继续监控,不退出
                            }
                        }
                    }
                    ConnectionStatus::Disconnected | ConnectionStatus::Error(_) => {
                        // 连接断开，触发重连
                        log_important!(info, "[{}] 检测到断线，触发重连", server_name);
                        manager.reconnect_with_backoff(&server_id).await;
                        // 重连成功后继续监控,不退出
                    }
                    _ => {}
                }
            }
        });

        // 保存任务句柄
        self.heartbeat_tasks.lock().await.insert(server_id_owned, handle);
    }

    /// 克隆用于心跳任务
    fn clone_for_heartbeat(&self) -> Self {
        Self {
            connections: self.connections.clone(),
            heartbeat_tasks: self.heartbeat_tasks.clone(),
        }
    }

    /// 带指数退避的重连
    async fn reconnect_with_backoff(&self, server_id: &str) {
        let mut delay = WEBSOCKET_RECONNECT_INITIAL_DELAY_SECS;
        let max_delay = WEBSOCKET_RECONNECT_MAX_DELAY_SECS;

        loop {
            // 获取连接
            let connections_guard = self.connections.lock().await;
            let connection = match connections_guard.get(server_id) {
                Some(conn) => conn,
                None => {
                    log_important!(info, "[{}] 连接已移除，停止重连", server_id);
                    return;
                }
            };

            // 检查是否应该继续重连
            if !*connection.should_reconnect.lock().await {
                log_important!(info, "[{}] 自动重连已禁用，停止重连", connection.config.name);
                return;
            }

            let server_name = connection.config.name.clone();
            drop(connections_guard);

            // 尝试重连(直接调用connection.connect,不spawn新任务)
            log_important!(info, "[{}] 尝试重连...", server_name);
            emit_ws_log(&server_name, "info", &format!("尝试重连... ({}秒后重试)", delay));

            let connections_guard = self.connections.lock().await;
            let result = if let Some(connection) = connections_guard.get(server_id) {
                connection.connect().await
            } else {
                log_important!(info, "[{}] 连接已移除，停止重连", server_id);
                return;
            };
            drop(connections_guard);

            match result {
                Ok(_) => {
                    log_important!(info, "[{}] 重连成功", server_name);
                    emit_ws_log(&server_name, "success", "重连成功");

                    // 重连成功,返回让心跳任务继续监控
                    return;
                }
                Err(e) => {
                    log_important!(warn, "[{}] 重连失败: {}, {}秒后重试", server_name, e, delay);
                    emit_ws_log(&server_name, "warn", &format!("重连失败，{}秒后重试", delay));

                    // 等待后重试
                    tokio::time::sleep(Duration::from_secs(delay)).await;

                    // 指数退避，但不超过最大延迟
                    delay = (delay * 2).min(max_delay);
                }
            }
        }
    }
}

