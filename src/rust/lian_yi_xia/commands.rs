use crate::lian_yi_xia::types::{ConnectionStatus, LianYiXiaState, WebSocketServerConfig, WebSocketServersConfig};
use crate::lian_yi_xia::websocket_manager::LianYiXiaWebSocketManager;
use crate::log_important;
use crate::config::{AppState, storage::{save_config, load_config}, settings::LianYiXiaServerConfig};
use tauri::{State, AppHandle};
use uuid::Uuid;
use std::collections::HashMap;
use once_cell::sync::OnceCell;

/// 全局WebSocket管理器
static WS_MANAGER: OnceCell<LianYiXiaWebSocketManager> = OnceCell::new();

/// 获取WebSocket管理器实例（公开给bin使用）
pub fn get_ws_manager() -> &'static LianYiXiaWebSocketManager {
    WS_MANAGER.get_or_init(|| LianYiXiaWebSocketManager::new())
}

/// 获取连一下应用信息
#[tauri::command]
pub async fn get_lian_yi_xia_app_info() -> Result<String, String> {
    Ok(format!("连一下 v{}", env!("CARGO_PKG_VERSION")))
}

/// 获取所有WebSocket服务器配置
#[tauri::command]
pub async fn get_websocket_servers(state: State<'_, LianYiXiaState>) -> Result<WebSocketServersConfig, String> {
    let config = state.servers_config.lock()
        .map_err(|e| format!("获取配置失败: {}", e))?;
    Ok(config.clone())
}

/// 添加WebSocket服务器配置
#[tauri::command]
pub async fn add_websocket_server(
    name: String,
    host: String,
    port: u16,
    api_key: String,
    enabled: bool,
    auto_connect: bool,
    lian_yi_xia_state: State<'_, LianYiXiaState>,
    app_state: State<'_, AppState>,
    app: AppHandle,
) -> Result<String, String> {
    // 检查IP+端口的唯一性
    {
        let config = lian_yi_xia_state.servers_config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        if config.servers.iter().any(|s| s.host == host && s.port == port) {
            return Err(format!("服务器 {}:{} 已存在", host, port));
        }
    }

    let server_id = Uuid::new_v4().to_string();
    let server_config = WebSocketServerConfig {
        id: server_id.clone(),
        name: name.clone(),
        host: host.clone(),
        port,
        api_key: api_key.clone(),
        enabled,
        auto_connect,
    };

    // 更新运行时状态
    {
        let mut config = lian_yi_xia_state.servers_config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.servers.push(server_config.clone());
    }

    // 添加到WebSocket管理器
    let manager = get_ws_manager();
    manager.add_server(server_config).await
        .map_err(|e| format!("添加服务器到管理器失败: {}", e))?;

    // 保存到配置文件
    {
        let mut config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.lian_yi_xia_servers_config.servers.push(LianYiXiaServerConfig {
            id: server_id.clone(),
            name,
            host,
            port,
            api_key,
            enabled,
            auto_connect,
        });
    }

    save_config(&app_state, &app).await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    log_important!(info, "添加WebSocket服务器配置: {}", server_id);
    Ok(server_id)
}

/// 更新WebSocket服务器配置
#[tauri::command]
pub async fn update_websocket_server(
    server_config: WebSocketServerConfig,
    lian_yi_xia_state: State<'_, LianYiXiaState>,
    app_state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // 检查IP+端口的唯一性（排除当前服务器）
    {
        let config = lian_yi_xia_state.servers_config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        if config.servers.iter().any(|s|
            s.id != server_config.id &&
            s.host == server_config.host &&
            s.port == server_config.port
        ) {
            return Err(format!("服务器 {}:{} 已存在", server_config.host, server_config.port));
        }
    }

    // 更新运行时状态
    {
        let mut config = lian_yi_xia_state.servers_config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        if let Some(server) = config.servers.iter_mut().find(|s| s.id == server_config.id) {
            *server = server_config.clone();
        } else {
            return Err(format!("未找到ID为 {} 的服务器", server_config.id));
        }
    }

    // 更新WebSocket管理器
    let manager = get_ws_manager();
    manager.update_server(server_config.clone()).await
        .map_err(|e| format!("更新服务器管理器失败: {}", e))?;

    // 保存到配置文件
    {
        let mut config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        if let Some(server) = config.lian_yi_xia_servers_config.servers.iter_mut().find(|s| s.id == server_config.id) {
            server.name = server_config.name;
            server.host = server_config.host;
            server.port = server_config.port;
            server.api_key = server_config.api_key;
            server.enabled = server_config.enabled;
            server.auto_connect = server_config.auto_connect;
        } else {
            return Err(format!("未找到ID为 {} 的服务器", server_config.id));
        }
    }

    save_config(&app_state, &app).await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    log_important!(info, "更新WebSocket服务器配置: {}", server_config.id);
    Ok(())
}

/// 删除WebSocket服务器配置
#[tauri::command]
pub async fn delete_websocket_server(
    server_id: String,
    lian_yi_xia_state: State<'_, LianYiXiaState>,
    app_state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // 从WebSocket管理器移除
    let manager = get_ws_manager();
    manager.remove_server(&server_id).await
        .map_err(|e| format!("从管理器移除服务器失败: {}", e))?;

    // 更新运行时状态
    {
        let mut config = lian_yi_xia_state.servers_config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        let initial_len = config.servers.len();
        config.servers.retain(|s| s.id != server_id);

        if config.servers.len() == initial_len {
            return Err(format!("未找到ID为 {} 的服务器", server_id));
        }
    }

    // 保存到配置文件
    {
        let mut config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;

        config.lian_yi_xia_servers_config.servers.retain(|s| s.id != server_id);
    }

    save_config(&app_state, &app).await
        .map_err(|e| format!("保存配置失败: {}", e))?;

    log_important!(info, "删除WebSocket服务器配置: {}", server_id);
    Ok(())
}

/// 生成新的API Key
#[tauri::command]
pub async fn generate_api_key() -> Result<String, String> {
    let api_key = Uuid::new_v4().to_string();
    log_important!(info, "生成新的API Key");
    Ok(api_key)
}

/// 连接到指定WebSocket服务器
#[tauri::command]
pub async fn connect_to_server(server_id: String) -> Result<(), String> {
    let manager = get_ws_manager();
    manager.connect_server(&server_id).await
        .map_err(|e| e.to_string())
}

/// 断开指定WebSocket服务器
#[tauri::command]
pub async fn disconnect_from_server(server_id: String) -> Result<(), String> {
    let manager = get_ws_manager();
    manager.disconnect_server(&server_id).await
        .map_err(|e| e.to_string())
}

/// 获取指定服务器的连接状态
#[tauri::command]
pub async fn get_server_connection_status(server_id: String) -> Result<ConnectionStatus, String> {
    let manager = get_ws_manager();
    manager.get_server_status(&server_id).await
        .map_err(|e| format!("获取状态失败: {}", e))
}

/// 获取所有服务器的连接状态
#[tauri::command]
pub async fn get_all_connection_status() -> Result<HashMap<String, ConnectionStatus>, String> {
    let manager = get_ws_manager();
    Ok(manager.get_all_status().await)
}

/// 从配置文件重新加载服务器配置
#[tauri::command]
pub async fn reload_servers_from_config(
    app_state: State<'_, AppState>,
    lian_yi_xia_state: State<'_, LianYiXiaState>,
    app: AppHandle,
) -> Result<Vec<WebSocketServerConfig>, String> {
    log_important!(info, "开始从配置文件重新加载服务器配置");

    // 先从磁盘重新加载配置文件到内存
    load_config(&app_state, &app)
        .await
        .map_err(|e| format!("重新加载配置文件失败: {}", e))?;

    // 从配置文件读取
    let config_servers = {
        let config = app_state.config.lock()
            .map_err(|e| format!("获取配置失败: {}", e))?;
        config.lian_yi_xia_servers_config.servers.clone()
    };

    // 转换为运行时配置
    let runtime_servers: Vec<WebSocketServerConfig> = config_servers.iter().map(|s| {
        WebSocketServerConfig {
            id: s.id.clone(),
            name: s.name.clone(),
            host: s.host.clone(),
            port: s.port,
            api_key: s.api_key.clone(),
            enabled: s.enabled,
            auto_connect: s.auto_connect,
        }
    }).collect();

    // 更新运行时状态
    {
        let mut lian_yi_xia_config = lian_yi_xia_state.servers_config.lock()
            .map_err(|e| format!("获取运行时配置失败: {}", e))?;
        lian_yi_xia_config.servers = runtime_servers.clone();
    }

    log_important!(info, "已重新加载 {} 个服务器配置", runtime_servers.len());

    Ok(runtime_servers)
}
