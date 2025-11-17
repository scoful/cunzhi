// "连一下" - WebSocket客户端管理器入口点
// Release模式下隐藏Windows控制台窗口,Debug模式保留(方便调试)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cunzhi::utils::auto_init_logger;
use cunzhi::log_important;
use cunzhi::lian_yi_xia::LianYiXiaState;
use cunzhi::config::{AppState, storage::load_config};
use tauri::{Manager, LogicalSize, AppHandle, WindowEvent};
use anyhow::Result;
use tauri::Builder;

// Wrapper commands in bin crate so generate_handler! resolves within this crate
#[tauri::command]
fn get_lian_yi_xia_app_info() -> String {
    cunzhi::lian_yi_xia::get_lian_yi_xia_app_info()
}

// 新架构命令
#[tauri::command]
async fn get_connected_clients() -> Result<Vec<cunzhi::lian_yi_xia::ConnectedClient>, String> {
    cunzhi::lian_yi_xia::get_connected_clients().await
}

#[tauri::command]
async fn get_ws_server_status() -> Result<cunzhi::lian_yi_xia::WsServerStatus, String> {
    cunzhi::lian_yi_xia::get_ws_server_status().await
}

#[tauri::command]
async fn get_ws_server_port() -> Result<u16, String> {
    cunzhi::lian_yi_xia::get_ws_server_port().await
}

#[tauri::command]
async fn save_ws_server_port(port: u16) -> Result<(), String> {
    cunzhi::lian_yi_xia::save_ws_server_port(port).await
}

// SSH隧道管理命令
#[tauri::command]
async fn get_ssh_tunnel_config(app: AppHandle) -> Result<Option<cunzhi::config::settings::SshTunnelConfig>, String> {
    cunzhi::lian_yi_xia::get_ssh_tunnel_config(app).await
}

#[tauri::command]
async fn update_ssh_tunnel_config(app: AppHandle, ssh_config: Option<cunzhi::config::settings::SshTunnelConfig>) -> Result<(), String> {
    cunzhi::lian_yi_xia::update_ssh_tunnel_config(app, ssh_config).await
}

#[tauri::command]
async fn update_ws_server_port(app: AppHandle, port: u16) -> Result<(), String> {
    cunzhi::lian_yi_xia::update_ws_server_port(app, port).await
}

#[tauri::command]
async fn start_ssh_tunnel() -> Result<(), String> {
    cunzhi::lian_yi_xia::start_ssh_tunnel().await
}

#[tauri::command]
async fn stop_ssh_tunnel() -> Result<(), String> {
    cunzhi::lian_yi_xia::stop_ssh_tunnel().await
}

#[tauri::command]
async fn restart_ssh_tunnel() -> Result<(), String> {
    cunzhi::lian_yi_xia::restart_ssh_tunnel().await
}

#[tauri::command]
async fn get_ssh_tunnel_status() -> Result<String, String> {
    cunzhi::lian_yi_xia::get_ssh_tunnel_status().await
}

#[tauri::command]
async fn get_ssh_tunnel_command() -> Result<Option<String>, String> {
    cunzhi::lian_yi_xia::get_ssh_tunnel_command().await
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

                    // 新架构: 停止SSH隧道(如果有)
                    if let Some(ssh_manager) = cunzhi::lian_yi_xia::get_ssh_tunnel_manager() {
                        if let Err(e) = ssh_manager.stop().await {
                            log_important!(warn, "停止SSH隧道失败: {}", e);
                        }
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
        .plugin(tauri_plugin_dialog::init())
        .manage(LianYiXiaState::default())
        .manage(AppState::default())
        .invoke_handler(tauri::generate_handler![
            get_lian_yi_xia_app_info,
            // 新架构命令
            get_connected_clients,
            get_ws_server_status,
            get_ws_server_port,
            save_ws_server_port,
            // SSH隧道管理命令
            get_ssh_tunnel_config,
            update_ssh_tunnel_config,
            update_ws_server_port,
            start_ssh_tunnel,
            stop_ssh_tunnel,
            restart_ssh_tunnel,
            get_ssh_tunnel_status,
            get_ssh_tunnel_command,
        ])
        .setup(|app| {
            // 设置全局AppHandle(用于WebSocket日志事件)
            cunzhi::lian_yi_xia::set_app_handle(app.handle().clone());

            // 启动WebSocket服务器
            {
                use std::sync::Arc;
                let ws_server = Arc::new(cunzhi::lian_yi_xia::ws_server::LianYiXiaWsServer::new());

                // 保存全局实例
                cunzhi::lian_yi_xia::set_ws_server(ws_server.clone());

                // 启动服务器
                tauri::async_runtime::spawn(async move {
                    log_important!(info, "正在启动WebSocket服务器...");
                    if let Err(e) = ws_server.start().await {
                        log_important!(error, "WebSocket服务器启动失败: {}", e);
                    }
                });
            }

            // 初始化SSH隧道管理器
            {
                use std::sync::Arc;

                // 从配置读取端口
                let app_state = app.state::<AppState>();
                let port = {
                    let config = app_state.config.lock().ok();
                    config.map(|c| c.lian_yi_xia_config.port).unwrap_or(9000)
                };

                let ssh_manager = Arc::new(cunzhi::lian_yi_xia::ssh_tunnel_manager::SshTunnelManager::new(port));

                // 保存全局实例
                cunzhi::lian_yi_xia::set_ssh_tunnel_manager(ssh_manager.clone());

                log_important!(info, "SSH隧道管理器已初始化");
            }

            // 加载配置并应用窗口设置
            let app_handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let app_state = app_handle.state::<AppState>();

                // 加载配置
                if let Err(e) = load_config(&app_state, &app_handle).await {
                    log_important!(warn, "加载配置失败: {}", e);
                }

                // 加载SSH隧道配置并自动启动
                {
                    // 先获取配置数据,然后立即释放锁
                    let (ssh_config, port) = {
                        let config = app_state.config.lock().ok();
                        if let Some(config) = config {
                            (
                                config.lian_yi_xia_config.ssh_tunnel.clone(),
                                config.lian_yi_xia_config.port,
                            )
                        } else {
                            (None, 9000)
                        }
                    };

                    // 更新SSH隧道管理器配置
                    if let Some(manager) = cunzhi::lian_yi_xia::get_ssh_tunnel_manager() {
                        manager.update_config(ssh_config.clone()).await;
                        manager.update_port(port).await;

                        // 如果配置了SSH隧道且启用了auto_start,则自动启动
                        if let Some(ssh_cfg) = ssh_config {
                            if ssh_cfg.enabled && ssh_cfg.auto_start {
                                log_important!(info, "自动启动SSH隧道...");
                                if let Err(e) = manager.start().await {
                                    log_important!(error, "自动启动SSH隧道失败: {}", e);
                                } else {
                                    log_important!(info, "SSH隧道已自动启动");
                                }
                            }
                        }
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
