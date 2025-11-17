use crate::lian_yi_xia::ws_server::LianYiXiaWsServer;
use crate::lian_yi_xia::ssh_tunnel_manager::{SshTunnelManager, TunnelStatus};
use crate::log_important;
use crate::config::{AppState, storage::save_config, settings::SshTunnelConfig};
use tauri::{State, AppHandle, Manager};
use std::sync::Arc;
use once_cell::sync::OnceCell;

/// 获取应用信息
#[tauri::command]
pub fn get_lian_yi_xia_app_info() -> String {
    format!("连一下 v{}", env!("CARGO_PKG_VERSION"))
}

/// 全局WebSocket服务器(新架构)
static WS_SERVER: OnceCell<Arc<LianYiXiaWsServer>> = OnceCell::new();

/// 全局SSH隧道管理器(新架构)
static SSH_TUNNEL_MANAGER: OnceCell<Arc<SshTunnelManager>> = OnceCell::new();

/// 全局AppHandle(用于发送事件到前端)
static APP_HANDLE: OnceCell<AppHandle> = OnceCell::new();

/// 设置全局WebSocket服务器实例(新架构)
pub fn set_ws_server(server: Arc<LianYiXiaWsServer>) {
    WS_SERVER.set(server).ok();
}

/// 获取全局WebSocket服务器实例(新架构)
pub fn get_ws_server() -> Option<&'static Arc<LianYiXiaWsServer>> {
    WS_SERVER.get()
}

/// 设置全局SSH隧道管理器实例(新架构)
pub fn set_ssh_tunnel_manager(manager: Arc<SshTunnelManager>) {
    SSH_TUNNEL_MANAGER.set(manager).ok();
}

/// 获取全局SSH隧道管理器实例(新架构)
pub fn get_ssh_tunnel_manager() -> Option<&'static Arc<SshTunnelManager>> {
    SSH_TUNNEL_MANAGER.get()
}

/// 设置全局AppHandle(在应用启动时调用)
pub fn set_app_handle(app: AppHandle) {
    APP_HANDLE.set(app).ok();
}

/// 获取全局AppHandle
pub fn get_app_handle() -> Option<&'static AppHandle> {
    APP_HANDLE.get()
}

// ============ 新架构命令 ============

/// 客户端信息
#[derive(serde::Serialize)]
pub struct ConnectedClient {
    pub client_id: String,
    pub connected_at: String,
}

/// WebSocket服务器状态信息
#[derive(serde::Serialize)]
pub struct WsServerStatus {
    pub status: String,
    pub address: String,
    pub uptime: String,
    pub client_count: usize,
}

/// 获取已连接的客户端列表
#[tauri::command]
pub async fn get_connected_clients() -> Result<Vec<ConnectedClient>, String> {
    let server = get_ws_server()
        .ok_or_else(|| "WebSocket服务器未启动".to_string())?;

    let clients = server.get_connected_clients().await;

    let result = clients.into_iter().map(|(client_id, connected_at)| {
        // 计算连接时长
        let duration = connected_at.elapsed();
        let total_secs = duration.as_secs();
        let hours = total_secs / 3600;
        let minutes = (total_secs % 3600) / 60;
        let seconds = total_secs % 60;

        let connected_time = if hours > 0 {
            format!("{}小时{}分钟", hours, minutes)
        } else if minutes > 0 {
            format!("{}分钟{}秒", minutes, seconds)
        } else {
            format!("{}秒", seconds)
        };

        ConnectedClient {
            client_id,
            connected_at: connected_time,
        }
    }).collect();

    Ok(result)
}

/// 获取WebSocket服务器状态
#[tauri::command]
pub async fn get_ws_server_status() -> Result<WsServerStatus, String> {
    let server = get_ws_server()
        .ok_or_else(|| "WebSocket服务器未启动".to_string())?;

    let (status, address, uptime, client_count) = server.get_status_info().await;

    Ok(WsServerStatus {
        status,
        address,
        uptime,
        client_count,
    })
}

/// 获取WebSocket服务器端口
#[tauri::command]
pub async fn get_ws_server_port() -> Result<u16, String> {
    // 从配置文件读取
    let app = get_app_handle()
        .ok_or_else(|| "应用未初始化".to_string())?;

    let app_state: State<AppState> = app.state();
    let config = app_state.config.lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;

    Ok(config.lian_yi_xia_config.port)
}

/// 保存WebSocket服务器端口
#[tauri::command]
pub async fn save_ws_server_port(port: u16) -> Result<(), String> {
    log_important!(info, "保存WebSocket服务器端口: {}", port);

    // 验证端口范围
    if port == 0 {
        return Err("端口不能为0".to_string());
    }

    let app = get_app_handle()
        .ok_or_else(|| "应用未初始化".to_string())?;

    let app_state: State<AppState> = app.state();

    // 更新配置
    {
        let mut config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        config.lian_yi_xia_config.port = port;
    }

    // 保存到文件
    save_config(&app_state, &app).await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 更新SSH隧道管理器的端口
    if let Some(ssh_manager) = get_ssh_tunnel_manager() {
        ssh_manager.update_port(port).await;
    }

    log_important!(info, "WebSocket服务器端口已更新为: {},需要重启应用才能生效", port);
    Ok(())
}

// ============ SSH隧道管理命令 ============

/// 获取SSH隧道配置
#[tauri::command]
pub async fn get_ssh_tunnel_config(app: AppHandle) -> Result<Option<SshTunnelConfig>, String> {
    let app_state: State<AppState> = app.state();
    let config = app_state.config.lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;

    Ok(config.lian_yi_xia_config.ssh_tunnel.clone())
}

/// 更新SSH隧道配置
#[tauri::command]
pub async fn update_ssh_tunnel_config(
    app: AppHandle,
    ssh_config: Option<SshTunnelConfig>,
) -> Result<(), String> {
    // 更新配置文件
    {
        let app_state: State<AppState> = app.state();
        let mut config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        config.lian_yi_xia_config.ssh_tunnel = ssh_config.clone();
    }

    // 保存配置
    let app_state: State<AppState> = app.state();
    save_config(&app_state, &app).await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 更新SSH隧道管理器配置
    if let Some(manager) = get_ssh_tunnel_manager() {
        manager.update_config(ssh_config).await;
    }

    log_important!(info, "SSH隧道配置已更新");
    Ok(())
}

/// 更新WebSocket服务器端口
#[tauri::command]
pub async fn update_ws_server_port(
    app: AppHandle,
    port: u16,
) -> Result<(), String> {
    // 更新配置文件
    {
        let app_state: State<AppState> = app.state();
        let mut config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        config.lian_yi_xia_config.port = port;
    }

    // 保存配置
    let app_state: State<AppState> = app.state();
    save_config(&app_state, &app).await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    // 更新SSH隧道管理器端口
    if let Some(manager) = get_ssh_tunnel_manager() {
        manager.update_port(port).await;
    }

    log_important!(info, "WebSocket服务器端口已更新: {}", port);
    Ok(())
}

/// 启动SSH隧道
#[tauri::command]
pub async fn start_ssh_tunnel() -> Result<(), String> {
    let manager = get_ssh_tunnel_manager()
        .ok_or_else(|| "SSH隧道管理器未初始化".to_string())?;

    manager.start().await
        .map_err(|e| format!("启动SSH隧道失败: {}", e))?;

    Ok(())
}

/// 停止SSH隧道
#[tauri::command]
pub async fn stop_ssh_tunnel() -> Result<(), String> {
    let manager = get_ssh_tunnel_manager()
        .ok_or_else(|| "SSH隧道管理器未初始化".to_string())?;

    manager.stop().await
        .map_err(|e| format!("停止SSH隧道失败: {}", e))?;

    Ok(())
}

/// 重启SSH隧道
#[tauri::command]
pub async fn restart_ssh_tunnel() -> Result<(), String> {
    let manager = get_ssh_tunnel_manager()
        .ok_or_else(|| "SSH隧道管理器未初始化".to_string())?;

    manager.restart().await
        .map_err(|e| format!("重启SSH隧道失败: {}", e))?;

    Ok(())
}

/// 获取SSH隧道状态
#[tauri::command]
pub async fn get_ssh_tunnel_status() -> Result<String, String> {
    let manager = get_ssh_tunnel_manager()
        .ok_or_else(|| "SSH隧道管理器未初始化".to_string())?;

    let status = manager.get_status().await;

    let status_str = match status {
        TunnelStatus::Stopped => "stopped",
        TunnelStatus::Starting => "starting",
        TunnelStatus::Running => "running",
        TunnelStatus::Error(_) => "error",
    };

    Ok(status_str.to_string())
}

/// 获取SSH隧道命令字符串
#[tauri::command]
pub async fn get_ssh_tunnel_command() -> Result<Option<String>, String> {
    let manager = get_ssh_tunnel_manager()
        .ok_or_else(|| "SSH隧道管理器未初始化".to_string())?;

    Ok(manager.get_command_string().await)
}
