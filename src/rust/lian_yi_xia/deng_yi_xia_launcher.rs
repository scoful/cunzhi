/// "等一下"启动器
///
/// 提供启动"等一下"进程的工具函数

use anyhow::Result;
use std::fs;
use std::process::Command;
use crate::log_important;

/// 启动"等一下"进程处理popup请求
///
/// # 参数
/// - `json`: popup请求的JSON数据
/// - `client_id`: 客户端ID(用于日志)
///
/// # 返回
/// - `Ok(String)`: 用户响应的JSON字符串
/// - `Err`: 启动失败或用户取消
pub async fn launch_deng_yi_xia(json: &serde_json::Value, client_id: &str) -> Result<String> {
    // 提取请求ID
    let request_id = json.get("request_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少request_id"))?;

    // 创建临时请求文件
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("mcp_request_{}.json", request_id));
    let request_json = serde_json::to_string_pretty(json)?;
    fs::write(&temp_file, &request_json)?;

    log_important!(info, "[{}] 创建临时请求文件: {}", client_id, temp_file.display());

    // 查找"等一下"命令路径
    let command_path = find_deng_yi_xia_command()?;
    log_important!(info, "[{}] 使用等一下命令: {}", client_id, command_path);

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
        log_important!(info, "[{}] 等一下实例返回: {}", client_id, response_str);

        // 返回响应内容
        if response_str.is_empty() {
            Ok("用户取消了操作".to_string())
        } else {
            Ok(response_str.to_string())
        }
    } else {
        let error = String::from_utf8_lossy(&output.stderr);
        log_important!(warn, "[{}] 等一下实例失败: {}", client_id, error);
        anyhow::bail!("等一下实例失败: {}", error)
    }
}

/// 查找"等一下"命令路径
fn find_deng_yi_xia_command() -> Result<String> {
    // 1. 优先尝试与当前"连一下"同目录的"等一下"命令
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            let local_ui_path = exe_dir.join("等一下");
            if local_ui_path.exists() && is_executable(&local_ui_path) {
                return Ok(local_ui_path.to_string_lossy().to_string());
            }
        }
    }

    // 2. 尝试全局命令
    if test_command_available("等一下") {
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

