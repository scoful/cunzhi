pub mod commands;
pub mod types;
pub mod ws_server;
pub mod deng_yi_xia_launcher;
pub mod ssh_tunnel_manager;

// 明确导出所有命令和类型
pub use commands::{
    set_app_handle,
    get_app_handle,
    get_lian_yi_xia_app_info,
    // 新架构命令
    set_ws_server,
    get_ws_server,
    get_connected_clients,
    get_ws_server_status,
    get_ws_server_port,
    save_ws_server_port,
    ConnectedClient,
    WsServerStatus,
    // SSH隧道管理命令
    set_ssh_tunnel_manager,
    get_ssh_tunnel_manager,
    get_ssh_tunnel_config,
    update_ssh_tunnel_config,
    update_ws_server_port,
    start_ssh_tunnel,
    stop_ssh_tunnel,
    restart_ssh_tunnel,
    get_ssh_tunnel_status,
    get_ssh_tunnel_command,
};
pub use types::{
    LianYiXiaState,
};
