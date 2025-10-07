use anyhow::Result;
use std::process::Command;
use std::fs;
use std::path::Path;
use std::sync::Arc;
use once_cell::sync::OnceCell;

use crate::mcp::types::PopupRequest;
use crate::mcp::ws_server::WsServer;
use crate::log_important;

/// 全局WebSocket服务器实例
static WS_SERVER: OnceCell<Arc<WsServer>> = OnceCell::new();

/// 设置全局WebSocket服务器实例
pub fn set_ws_server(server: Arc<WsServer>) {
    let _ = WS_SERVER.set(server);
}

/// 创建 Tauri 弹窗
///
/// 优先使用WebSocket推送,失败时fallback到本地进程调用
pub async fn create_tauri_popup(request: &PopupRequest) -> Result<String> {
    // 1. 优先尝试WebSocket推送
    if let Some(ws_server) = WS_SERVER.get() {
        let has_clients = ws_server.has_clients().await;

        if has_clients {
            log_important!(info, "使用WebSocket推送弹窗请求");
            match ws_server.send_popup_request(request).await {
                Ok(response) => {
                    log_important!(info, "WebSocket响应成功");
                    return Ok(response);
                }
                Err(e) => {
                    log_important!(warn, "WebSocket推送失败,fallback到本地进程: {}", e);
                }
            }
        }
    }

    // 2. Fallback到本地进程调用
    log_important!(info, "使用本地进程调用弹窗");
    create_local_popup(request)
}

/// 本地进程调用弹窗(原有逻辑)
fn create_local_popup(request: &PopupRequest) -> Result<String> {
    // 创建临时请求文件 - 跨平台适配
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("mcp_request_{}.json", request.id));
    let request_json = serde_json::to_string_pretty(request)?;
    fs::write(&temp_file, request_json)?;

    // 尝试找到等一下命令的路径
    let command_path = find_ui_command()?;

    // 调用等一下命令
    let output = Command::new(&command_path)
        .arg("--mcp-request")
        .arg(temp_file.to_string_lossy().to_string())
        .output()?;

    // 清理临时文件
    let _ = fs::remove_file(&temp_file);

    if output.status.success() {
        let response = String::from_utf8_lossy(&output.stdout);
        let response = response.trim();
        if response.is_empty() {
            Ok("用户取消了操作".to_string())
        } else {
            Ok(response.to_string())
        }
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("UI进程失败: {}", error);
    }
}

/// 查找等一下 UI 命令的路径
///
/// 按优先级查找：同目录 -> 全局版本 -> 开发环境
fn find_ui_command() -> Result<String> {
    // 1. 优先尝试与当前 MCP 服务器同目录的等一下命令
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            let local_ui_path = exe_dir.join("等一下");
            if local_ui_path.exists() && is_executable(&local_ui_path) {
                return Ok(local_ui_path.to_string_lossy().to_string());
            }
        }
    }

    // 2. 尝试全局命令（最常见的部署方式）
    if test_command_available("等一下") {
        return Ok("等一下".to_string());
    }

    // 3. 如果都找不到，返回详细错误信息
    anyhow::bail!(
        "找不到等一下 UI 命令。请确保：\n\
         1. 已编译项目：cargo build --release\n\
         2. 或已全局安装：./install.sh\n\
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
fn is_executable(path: &Path) -> bool {
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
