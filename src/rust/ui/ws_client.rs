use anyhow::Result;
use futures_util::{SinkExt, StreamExt};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use std::process::Command;
use std::fs;

use crate::mcp::types::PopupRequest;
use crate::log_important;

/// 启动WebSocket客户端
pub async fn run_ws_client(server_url: &str) -> Result<()> {
    log_important!(info, "连接WebSocket服务器: {}", server_url);

    // 连接服务器
    let (ws_stream, _) = connect_async(server_url).await?;
    log_important!(info, "WebSocket连接成功");

    let (mut write, mut read) = ws_stream.split();

    // 消息处理循环
    while let Some(msg) = read.next().await {
        match msg {
            Ok(Message::Text(text)) => {
                log_important!(info, "收到消息: {}", text);

                // 解析消息
                match serde_json::from_str::<serde_json::Value>(&text) {
                    Ok(json) => {
                        if let Err(e) = handle_message(&mut write, json).await {
                            log_important!(warn, "处理消息失败: {}", e);
                        }
                    }
                    Err(e) => {
                        log_important!(warn, "解析消息失败: {}", e);
                    }
                }
            }
            Ok(Message::Close(_)) => {
                log_important!(info, "服务器关闭连接");
                break;
            }
            Err(e) => {
                log_important!(warn, "接收消息失败: {}", e);
                break;
            }
            _ => {}
        }
    }

    Ok(())
}

/// 处理服务器消息
async fn handle_message(
    write: &mut futures_util::stream::SplitSink<
        tokio_tungstenite::WebSocketStream<
            tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
        >,
        Message,
    >,
    json: serde_json::Value,
) -> Result<()> {
    // 检查消息类型
    if json.get("type").and_then(|v| v.as_str()) != Some("popup_request") {
        return Ok(());
    }

    // 提取请求数据
    let request_id = json
        .get("request_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少request_id"))?;

    let message = json
        .get("message")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("缺少message"))?;

    let predefined_options = json
        .get("predefined_options")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        });

    let is_markdown = json
        .get("is_markdown")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    // 构建弹窗请求
    let popup_request = PopupRequest {
        id: request_id.to_string(),
        message: message.to_string(),
        predefined_options,
        is_markdown,
    };

    log_important!(info, "显示弹窗: {}", popup_request.message);

    // 调用本地弹窗逻辑(通过进程调用"等一下")
    let response = match call_local_popup(&popup_request) {
        Ok(resp) => resp,
        Err(e) => {
            log_important!(warn, "弹窗失败: {}", e);
            format!("错误: {}", e)
        }
    };

    log_important!(info, "用户响应: {}", response);

    // 发送响应
    let response_json = serde_json::json!({
        "type": "popup_response",
        "request_id": request_id,
        "response": response,
    });

    write
        .send(Message::Text(response_json.to_string()))
        .await?;

    Ok(())
}

/// 调用本地"等一下"进程显示弹窗
fn call_local_popup(request: &PopupRequest) -> Result<String> {
    // 创建临时请求文件
    let temp_dir = std::env::temp_dir();
    let temp_file = temp_dir.join(format!("mcp_request_{}.json", request.id));
    let request_json = serde_json::to_string_pretty(request)?;
    fs::write(&temp_file, request_json)?;

    // 查找"等一下"命令
    let command_path = find_ui_command()?;

    // 调用"等一下"命令
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
        anyhow::bail!("UI进程失败: {}", error)
    }
}

/// 查找"等一下"命令路径
fn find_ui_command() -> Result<String> {
    // 1. 尝试与当前程序同目录
    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(exe_dir) = current_exe.parent() {
            #[cfg(windows)]
            let local_ui_path = exe_dir.join("等一下.exe");
            #[cfg(not(windows))]
            let local_ui_path = exe_dir.join("等一下");

            if local_ui_path.exists() {
                return Ok(local_ui_path.to_string_lossy().to_string());
            }
        }
    }

    // 2. 尝试全局命令
    #[cfg(windows)]
    let command_name = "等一下.exe";
    #[cfg(not(windows))]
    let command_name = "等一下";

    if test_command_available(command_name) {
        return Ok(command_name.to_string());
    }

    anyhow::bail!("找不到等一下命令")
}

/// 测试命令是否可用
fn test_command_available(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

