use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;

/// WebSocket服务器配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebSocketServerConfig {
    pub id: String,           // 唯一标识
    pub name: String,         // 显示名称
    pub host: String,         // 服务器地址
    pub port: u16,            // 服务器端口
    pub api_key: String,      // API密钥
    pub enabled: bool,        // 是否启用
    pub auto_connect: bool,   // 是否自动连接
}

/// 多服务器配置
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSocketServersConfig {
    pub servers: Vec<WebSocketServerConfig>,
}

/// 连接状态
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "message")]
pub enum ConnectionStatus {
    #[serde(rename = "disconnected")]
    Disconnected,
    #[serde(rename = "connecting")]
    Connecting,
    #[serde(rename = "connected")]
    Connected,
    #[serde(rename = "error")]
    Error(String),
}

impl ConnectionStatus {
    pub fn as_string(&self) -> String {
        match self {
            ConnectionStatus::Disconnected => "disconnected".to_string(),
            ConnectionStatus::Connecting => "connecting".to_string(),
            ConnectionStatus::Connected => "connected".to_string(),
            ConnectionStatus::Error(_) => "error".to_string(),
        }
    }
}

/// WebSocket连接信息（运行时状态）
#[derive(Debug, Clone)]
pub struct WebSocketConnectionInfo {
    pub config: WebSocketServerConfig,
    pub status: ConnectionStatus,
}

/// "连一下"应用状态
#[derive(Debug, Default)]
pub struct LianYiXiaState {
    /// WebSocket服务器配置
    pub servers_config: Mutex<WebSocketServersConfig>,
    /// WebSocket服务器连接池（运行时状态）
    pub websocket_connections: Mutex<HashMap<String, WebSocketConnectionInfo>>,
}
