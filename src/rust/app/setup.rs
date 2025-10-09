use crate::config::{AppState, load_config_and_apply_window_settings};
use crate::ui::{initialize_audio_asset_manager, setup_window_event_listeners, initialize_websocket_client};
use crate::ui::exit_handler::setup_exit_handlers;
use crate::ui::websocket_commands::get_websocket_manager;
use crate::log_important;
use tauri::{AppHandle, Manager};

/// 应用设置和初始化
pub async fn setup_application(app_handle: &AppHandle) -> Result<(), String> {
    let state = app_handle.state::<AppState>();

    // 加载配置并应用窗口设置
    if let Err(e) = load_config_and_apply_window_settings(&state, app_handle).await {
        log_important!(warn, "加载配置失败: {}", e);
    }

    // 初始化音频资源管理器
    if let Err(e) = initialize_audio_asset_manager(app_handle) {
        log_important!(warn, "初始化音频资源管理器失败: {}", e);
    }

    // 设置窗口事件监听器
    setup_window_event_listeners(app_handle);

    // 设置退出处理器
    if let Err(e) = setup_exit_handlers(app_handle) {
        log_important!(warn, "设置退出处理器失败: {}", e);
    }

    // 设置WebSocket管理器的AppHandle
    let ws_manager = get_websocket_manager();
    ws_manager.set_app_handle(app_handle.clone()).await;

    // 初始化WebSocket客户端（如果启用了自动连接）
    if let Err(e) = initialize_websocket_client(&state).await {
        log_important!(warn, "初始化WebSocket客户端失败: {}", e);
    }

    Ok(())
}
