use crate::config::load_standalone_telegram_config;
use crate::telegram::handle_telegram_only_mcp_request;
use crate::ui::run_ws_client;
use crate::log_important;
use crate::app::builder::run_tauri_app;
use anyhow::Result;

/// 处理命令行参数
pub fn handle_cli_args() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();

    match args.len() {
        // 无参数：正常启动GUI
        1 => {
            run_tauri_app();
        }
        // 单参数：帮助或版本
        2 => {
            match args[1].as_str() {
                "--help" | "-h" => print_help(),
                "--version" | "-v" => print_version(),
                _ => {
                    eprintln!("未知参数: {}", args[1]);
                    print_help();
                    std::process::exit(1);
                }
            }
        }
        // 多参数：MCP请求模式或WebSocket客户端模式
        _ => {
            if args[1] == "--mcp-request" && args.len() >= 3 {
                handle_mcp_request(&args[2])?;
            } else if args[1] == "--ws-client" && args.len() >= 3 {
                handle_ws_client(&args[2])?;
            } else {
                eprintln!("无效的命令行参数");
                print_help();
                std::process::exit(1);
            }
        }
    }

    Ok(())
}

/// 处理MCP请求
fn handle_mcp_request(request_file: &str) -> Result<()> {
    // 检查Telegram配置，决定是否启用纯Telegram模式
    match load_standalone_telegram_config() {
        Ok(telegram_config) => {
            if telegram_config.enabled && telegram_config.hide_frontend_popup {
                // 纯Telegram模式：不启动GUI，直接处理
                if let Err(e) = tokio::runtime::Runtime::new()
                    .unwrap()
                    .block_on(handle_telegram_only_mcp_request(request_file))
                {
                    log_important!(error, "处理Telegram请求失败: {}", e);
                    std::process::exit(1);
                }
            } else {
                // 正常模式：启动GUI处理弹窗
                run_tauri_app();
            }
        }
        Err(e) => {
            log_important!(warn, "加载Telegram配置失败: {}，使用默认GUI模式", e);
            // 配置加载失败时，使用默认行为（启动GUI）
            run_tauri_app();
        }
    }
    Ok(())
}

/// 处理WebSocket客户端模式
fn handle_ws_client(server_url: &str) -> Result<()> {
    log_important!(info, "启动WebSocket客户端模式: {}", server_url);

    // 创建tokio runtime并运行WebSocket客户端
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        if let Err(e) = run_ws_client(server_url).await {
            log_important!(error, "WebSocket客户端失败: {}", e);
            std::process::exit(1);
        }
    });

    Ok(())
}

/// 显示帮助信息
fn print_help() {
    println!("寸止 - 智能代码审查工具");
    println!();
    println!("用法:");
    println!("  等一下                          启动设置界面");
    println!("  等一下 --mcp-request <文件>     处理 MCP 请求");
    println!("  等一下 --ws-client <服务器URL>  WebSocket客户端模式");
    println!("  等一下 --help                   显示此帮助信息");
    println!("  等一下 --version                显示版本信息");
    println!();
    println!("示例:");
    println!("  等一下 --ws-client ws://192.168.1.100:9000");
}

/// 显示版本信息
fn print_version() {
    println!("寸止 v{}", env!("CARGO_PKG_VERSION"));
}
