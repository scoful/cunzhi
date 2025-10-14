// "等一下" - MCP客户端入口点
// Release模式下隐藏Windows控制台窗口,Debug模式保留(方便调试)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use cunzhi::app::{handle_cli_args, run_tauri_app};
use cunzhi::utils::auto_init_logger;
use anyhow::Result;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    run_tauri_app();
}

fn main() -> Result<()> {
    // 初始化日志系统
    if let Err(e) = auto_init_logger() {
        eprintln!("初始化日志系统失败: {}", e);
    }

    // 处理命令行参数
    handle_cli_args()
}


