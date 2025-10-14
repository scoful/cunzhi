// "连一下" - WebSocket客户端管理器入口点
// Release模式下隐藏Windows控制台窗口,Debug模式保留(方便调试)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cunzhi::utils::auto_init_logger;
use cunzhi::log_important;
use cunzhi::lian_yi_xia::{LianYiXiaState, WebSocketServerConfig, WebSocketServersConfig};
use cunzhi::config::{AppState, storage::load_config};
use tauri::{State, Manager, LogicalSize, AppHandle, WindowEvent};
use anyhow::Result;
use tauri::Builder;

// Wrapper commands in bin crate so generate_handler! resolves within this crate
#[tauri::command]
async fn get_lian_yi_xia_app_info() -> Result<String, String> {
    cunzhi::lian_yi_xia::get_lian_yi_xia_app_info().await
}

#[tauri::command]
async fn get_websocket_servers(state: State<'_, LianYiXiaState>) -> Result<WebSocketServersConfig, String> {
    cunzhi::lian_yi_xia::get_websocket_servers(state).await
}

#[tauri::command]
async fn add_websocket_server(
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
    cunzhi::lian_yi_xia::add_websocket_server(name, host, port, api_key, enabled, auto_connect, lian_yi_xia_state, app_state, app).await
}

#[tauri::command]
async fn update_websocket_server(
    server_config: WebSocketServerConfig,
    lian_yi_xia_state: State<'_, LianYiXiaState>,
    app_state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    cunzhi::lian_yi_xia::update_websocket_server(server_config, lian_yi_xia_state, app_state, app).await
}

#[tauri::command]
async fn delete_websocket_server(
    server_id: String,
    lian_yi_xia_state: State<'_, LianYiXiaState>,
    app_state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    cunzhi::lian_yi_xia::delete_websocket_server(server_id, lian_yi_xia_state, app_state, app).await
}

#[tauri::command]
async fn generate_api_key() -> Result<String, String> {
    cunzhi::lian_yi_xia::generate_api_key().await
}

#[tauri::command]
async fn connect_to_server(server_id: String) -> Result<(), String> {
    cunzhi::lian_yi_xia::connect_to_server(server_id).await
}

#[tauri::command]
async fn disconnect_from_server(server_id: String) -> Result<(), String> {
    cunzhi::lian_yi_xia::disconnect_from_server(server_id).await
}

#[tauri::command]
async fn get_server_connection_status(server_id: String) -> Result<cunzhi::lian_yi_xia::ConnectionStatus, String> {
    cunzhi::lian_yi_xia::get_server_connection_status(server_id).await
}

#[tauri::command]
async fn get_all_connection_status() -> Result<std::collections::HashMap<String, cunzhi::lian_yi_xia::ConnectionStatus>, String> {
    cunzhi::lian_yi_xia::get_all_connection_status().await
}

#[tauri::command]
async fn reload_servers_from_config(
    app_state: tauri::State<'_, AppState>,
    lian_yi_xia_state: tauri::State<'_, LianYiXiaState>,
    app: AppHandle,
) -> Result<Vec<cunzhi::lian_yi_xia::WebSocketServerConfig>, String> {
    cunzhi::lian_yi_xia::reload_servers_from_config(app_state, lian_yi_xia_state, app).await
}

/// 设置"连一下"窗口事件监听器
fn setup_lian_yi_xia_window_events(app_handle: &AppHandle) {
    if let Some(window) = app_handle.get_webview_window("main") {
        let app_handle_clone = app_handle.clone();

        window.on_window_event(move |event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                // 阻止默认的关闭行为
                api.prevent_close();

                let app_handle = app_handle_clone.clone();

                // 异步处理退出请求
                tauri::async_runtime::spawn(async move {
                    log_important!(info, "🖱️ 连一下窗口关闭按钮被点击");

                    // 断开所有WebSocket连接
                    let manager = cunzhi::lian_yi_xia::get_ws_manager();
                    if let Err(e) = manager.disconnect_all().await {
                        log_important!(warn, "断开WebSocket连接失败: {}", e);
                    }

                    // 关闭窗口
                    if let Some(window) = app_handle.get_webview_window("main") {
                        let _ = window.close();
                    }

                    // 短暂延迟后退出应用
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                    app_handle.exit(0);

                    log_important!(info, "连一下应用已退出");
                });
            }
        });
    }
}

/// 构建"连一下"Tauri应用
pub fn build_lian_yi_xia_app() -> Builder<tauri::Wry> {
    tauri::Builder::default()
        .manage(LianYiXiaState::default())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_lian_yi_xia_app_info,
            get_websocket_servers,
            add_websocket_server,
            update_websocket_server,
            delete_websocket_server,
            generate_api_key,
            connect_to_server,
            disconnect_from_server,
            get_server_connection_status,
            get_all_connection_status,
            reload_servers_from_config,
        ])
        .setup(|app| {
            // 加载配置并应用窗口设置
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let app_state = app_handle.state::<AppState>();
                let lian_yi_xia_state = app_handle.state::<LianYiXiaState>();

                // 加载配置
                if let Err(e) = load_config(&app_state, &app_handle).await {
                    log_important!(warn, "加载配置失败: {}", e);
                }

                // 从配置文件加载服务器配置到运行时状态
                let servers_to_add = {
                    let config = app_state.config.lock().map_err(|e| anyhow::anyhow!("获取配置失败: {}", e)).ok()?;
                    let mut lian_yi_xia_config = lian_yi_xia_state.servers_config.lock().ok()?;

                    // 将配置文件中的服务器配置转换为运行时配置
                    lian_yi_xia_config.servers = config.lian_yi_xia_servers_config.servers.iter().map(|s| {
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

                    log_important!(info, "已加载 {} 个WebSocket服务器配置", lian_yi_xia_config.servers.len());

                    // 克隆服务器配置列表，在锁外使用
                    lian_yi_xia_config.servers.clone()
                };

                // 将服务器配置添加到WebSocket管理器并尝试自动连接（在锁外执行）
                for server_config in servers_to_add {
                    if let Err(e) = cunzhi::lian_yi_xia::get_ws_manager().add_server_with_auto_connect(server_config.clone()).await {
                        log::warn!("添加服务器到管理器失败: {} - {}", server_config.name, e);
                    }
                }

                // 应用窗口设置（复用"等一下"的窗口配置）
                let window_config = {
                    let config = app_state.config.lock().map_err(|e| anyhow::anyhow!("获取配置失败: {}", e)).ok()?;
                    config.ui_config.window_config.clone()
                };

                if let Some(window) = app_handle.get_webview_window("main") {
                    // 应用窗口大小约束
                    if let Err(e) = window.set_min_size(Some(LogicalSize::new(
                        window_config.min_width,
                        window_config.min_height,
                    ))) {
                        log::warn!("设置最小窗口大小失败: {}", e);
                    }

                    if let Err(e) = window.set_max_size(Some(LogicalSize::new(
                        window_config.max_width,
                        window_config.max_height,
                    ))) {
                        log::warn!("设置最大窗口大小失败: {}", e);
                    }

                    // 根据当前模式设置窗口大小
                    let (target_width, target_height) = if window_config.fixed {
                        (window_config.fixed_width, window_config.fixed_height)
                    } else {
                        (window_config.free_width, window_config.free_height)
                    };

                    if let Err(e) = window.set_size(LogicalSize::new(target_width, target_height)) {
                        log::warn!("设置窗口大小失败: {}", e);
                    }

                    log_important!(info, "连一下窗口配置已应用: {}x{}", target_width, target_height);
                }

                Some(())
            });

            // 设置窗口关闭事件监听器
            setup_lian_yi_xia_window_events(&app.handle());

            log_important!(info, "连一下应用初始化完成");
            Ok(())
        })
}



/// 运行"连一下"Tauri应用
pub fn run_lian_yi_xia_app() {
    build_lian_yi_xia_app()
        .run(tauri::generate_context!("lian-yi-xia.conf.json"))
        .expect("error while running 连一下 application");
}

fn main() -> Result<()> {
    // 初始化日志系统
    if let Err(e) = auto_init_logger() {
        eprintln!("初始化日志系统失败: {}", e);
    }

    log_important!(info, "启动连一下 - WebSocket客户端管理器");

    // 启动Tauri GUI应用
    run_lian_yi_xia_app();

    Ok(())
}
