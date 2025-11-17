/// SSH隧道管理器
///
/// 负责启动、停止和监控SSH反向隧道进程

use anyhow::{Result, Context};
use tokio::process::{Child, Command};
use std::process::Stdio;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::io::{AsyncBufReadExt, BufReader};
use tauri::Emitter;
use crate::config::settings::SshTunnelConfig;
use crate::log_important;

/// SSH隧道状态
#[derive(Debug, Clone, PartialEq)]
pub enum TunnelStatus {
    Stopped,      // 已停止
    Starting,     // 启动中
    Running,      // 运行中
    Error(String), // 错误
}

/// SSH隧道管理器
pub struct SshTunnelManager {
    config: Arc<Mutex<Option<SshTunnelConfig>>>,
    process: Arc<Mutex<Option<Child>>>,
    status: Arc<Mutex<TunnelStatus>>,
    port: Arc<Mutex<u16>>,
}

impl SshTunnelManager {
    /// 创建新的SSH隧道管理器
    pub fn new(port: u16) -> Self {
        Self {
            config: Arc::new(Mutex::new(None)),
            process: Arc::new(Mutex::new(None)),
            status: Arc::new(Mutex::new(TunnelStatus::Stopped)),
            port: Arc::new(Mutex::new(port)),
        }
    }

    /// 更新配置
    pub async fn update_config(&self, config: Option<SshTunnelConfig>) {
        let mut cfg = self.config.lock().await;
        *cfg = config;
    }

    /// 更新端口
    pub async fn update_port(&self, port: u16) {
        let mut p = self.port.lock().await;
        *p = port;
    }

    /// 获取当前状态
    pub async fn get_status(&self) -> TunnelStatus {
        self.status.lock().await.clone()
    }

    /// 启动SSH隧道
    pub async fn start(&self) -> Result<()> {
        // 检查配置
        let config = self.config.lock().await;
        let config = config.as_ref()
            .context("SSH隧道配置未设置")?;

        if !config.enabled {
            return Err(anyhow::anyhow!("SSH隧道未启用"));
        }

        // 检查是否已经在运行
        let mut status = self.status.lock().await;
        if *status == TunnelStatus::Running {
            return Err(anyhow::anyhow!("SSH隧道已在运行"));
        }

        *status = TunnelStatus::Starting;
        drop(status);

        // 获取端口
        let local_port = *self.port.lock().await;
        let remote_port = if config.remote_port > 0 {
            config.remote_port
        } else {
            local_port  // 如果未配置remote_port,默认使用本地端口
        };

        // 构建SSH命令
        let mut cmd = Command::new("ssh");

        // 添加反向隧道参数: -R remote_port:localhost:local_port
        cmd.arg("-R")
            .arg(format!("{}:localhost:{}", remote_port, local_port));

        // 添加SSH密钥路径(如果有)
        if let Some(key_path) = &config.ssh_key_path {
            cmd.arg("-i").arg(key_path);
        }

        // 添加其他SSH选项
        cmd.arg("-N")  // 不执行远程命令
            .arg("-T")  // 禁用伪终端分配
            .arg("-o").arg("ServerAliveInterval=60")  // 保持连接
            .arg("-o").arg("ServerAliveCountMax=3")   // 最多3次心跳失败
            .arg("-o").arg("ExitOnForwardFailure=yes") // 转发失败时退出
            .arg("-vvv");  // 始终使用详细输出以便状态检测

        // 添加远程主机
        let remote = format!("{}@{}", config.remote_user, config.remote_host);
        cmd.arg(&remote);

        // 始终捕获stderr以便状态检测
        cmd.stdout(Stdio::null())  // stdout不需要
            .stderr(Stdio::piped());  // stderr用于状态检测

        if config.verbose_level > 0 {
            log_important!(info, "启动SSH隧道(Debug日志已开启): ssh -R {}:localhost:{} -vvv {}", remote_port, local_port, remote);
        } else {
            log_important!(info, "启动SSH隧道: ssh -R {}:localhost:{} -vvv {}", remote_port, local_port, remote);
        }

        // Windows平台: 隐藏控制台窗口
        #[cfg(windows)]
        cmd.creation_flags(0x08000000);  // CREATE_NO_WINDOW = 0x08000000

        // 启动进程
        let mut child = cmd.spawn()
            .context("启动SSH进程失败")?;

        // 获取stderr(stdout已设置为null,不需要处理)
        let stderr = child.stderr.take();

        // 保存进程
        let mut process = self.process.lock().await;
        *process = Some(child);
        drop(process);  // 释放锁,避免在后台任务中持有

        // 保持Starting状态,等待SSH输出确认
        log_important!(info, "SSH隧道进程已启动,等待连接确认...");

        // 保存verbose_level和Arc引用用于后台任务
        let verbose_level = config.verbose_level;
        let status_for_stderr = self.status.clone();
        let status_for_timeout = self.status.clone();

        // 启动后台任务读取stderr并解析状态
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let reader = BufReader::new(stderr);
                let mut lines = reader.lines();

                while let Ok(Some(line)) = lines.next_line().await {
                    log_important!(info, "[SSH stderr] {}", line);

                    // 检测成功标志
                    if line.contains("forwarding_success") || line.contains("remote forward success") {
                        let mut status = status_for_stderr.lock().await;
                        *status = TunnelStatus::Running;
                        drop(status);

                        log_important!(info, "SSH隧道已成功建立");

                        if let Some(app_handle) = crate::lian_yi_xia::get_app_handle() {
                            let _ = app_handle.emit("ssh-tunnel-status", "running");
                        }
                    }

                    // 检测失败标志(只检测真正的致命错误)
                    if line.contains("Connection refused") ||
                       line.contains("Permission denied") ||
                       line.contains("Could not request local forwarding") ||
                       line.contains("remote port forwarding failed") {
                        let mut status = status_for_stderr.lock().await;
                        if *status == TunnelStatus::Starting {
                            *status = TunnelStatus::Error("隧道建立失败".to_string());
                            drop(status);

                            log_important!(error, "SSH隧道建立失败: {}", line);

                            if let Some(app_handle) = crate::lian_yi_xia::get_app_handle() {
                                let _ = app_handle.emit("ssh-tunnel-status", "error");
                            }
                        }
                    }

                    // 仅在verbose_level > 0时发送到前端活动日志
                    if verbose_level > 0 {
                        if let Some(app_handle) = crate::lian_yi_xia::get_app_handle() {
                            let _ = app_handle.emit("log-event", format!("[SSH] {}", line));
                        }
                    }
                }

                // stderr读取结束,意味着进程退出
                // 检查状态,如果不是主动停止,就是异常退出
                let mut status = status_for_stderr.lock().await;
                if *status != TunnelStatus::Stopped {
                    log_important!(error, "SSH连接已断开(进程退出)");
                    *status = TunnelStatus::Error("SSH连接已断开".to_string());
                    drop(status);

                    if let Some(app_handle) = crate::lian_yi_xia::get_app_handle() {
                        let _ = app_handle.emit("ssh-tunnel-status", "error");
                    }
                }
            });
        }

        // 启动超时监控任务
        tokio::spawn(async move {
            tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;

            let mut status = status_for_timeout.lock().await;
            if *status == TunnelStatus::Starting {
                log_important!(error, "SSH隧道启动超时");
                *status = TunnelStatus::Error("启动超时(30秒)".to_string());

                if let Some(app_handle) = crate::lian_yi_xia::get_app_handle() {
                    let _ = app_handle.emit("ssh-tunnel-status", "error");
                }
            }
        });

        Ok(())
    }

    /// 停止SSH隧道
    pub async fn stop(&self) -> Result<()> {
        // 先更新状态为Stopped,避免进程退出监控任务误判
        let mut status = self.status.lock().await;
        *status = TunnelStatus::Stopped;
        drop(status);

        let mut process = self.process.lock().await;

        if let Some(mut child) = process.take() {
            log_important!(info, "停止SSH隧道");

            // 尝试优雅关闭
            if let Err(e) = child.kill().await {
                log_important!(warn, "关闭SSH进程失败: {}", e);
            }

            // 等待进程退出
            if let Err(e) = child.wait().await {
                log_important!(warn, "等待SSH进程退出失败: {}", e);
            }

            log_important!(info, "SSH隧道已停止");
        }

        // 通知前端状态变更
        if let Some(app_handle) = crate::lian_yi_xia::get_app_handle() {
            let _ = app_handle.emit("ssh-tunnel-status", "stopped");
        }

        Ok(())
    }

    /// 重启SSH隧道
    pub async fn restart(&self) -> Result<()> {
        log_important!(info, "重启SSH隧道");
        self.stop().await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
        self.start().await?;
        Ok(())
    }

    /// 检查SSH隧道是否在运行
    pub async fn is_running(&self) -> bool {
        let mut process = self.process.lock().await;
        
        if let Some(child) = process.as_mut() {
            // 尝试检查进程状态
            match child.try_wait() {
                Ok(Some(_)) => {
                    // 进程已退出
                    *process = None;
                    let mut status = self.status.lock().await;
                    *status = TunnelStatus::Stopped;
                    false
                }
                Ok(None) => {
                    // 进程仍在运行
                    true
                }
                Err(e) => {
                    // 检查失败
                    log_important!(warn, "检查SSH进程状态失败: {}", e);
                    *process = None;
                    let mut status = self.status.lock().await;
                    *status = TunnelStatus::Error(e.to_string());
                    false
                }
            }
        } else {
            false
        }
    }

    /// 获取SSH隧道命令字符串(用于UI显示)
    pub async fn get_command_string(&self) -> Option<String> {
        let config = self.config.lock().await;
        let config = config.as_ref()?;

        if !config.enabled {
            return None;
        }

        let local_port = *self.port.lock().await;
        let remote_port = if config.remote_port > 0 {
            config.remote_port
        } else {
            local_port  // 如果未配置remote_port,默认使用本地端口
        };
        let remote = format!("{}@{}", config.remote_user, config.remote_host);

        let mut cmd = format!("ssh -R {}:localhost:{}", remote_port, local_port);

        if let Some(key_path) = &config.ssh_key_path {
            cmd.push_str(&format!(" -i {}", key_path));
        }

        cmd.push_str(&format!(" {}", remote));

        Some(cmd)
    }
}

impl Drop for SshTunnelManager {
    fn drop(&mut self) {
        // 同步停止SSH隧道
        if let Some(mut child) = self.process.blocking_lock().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

