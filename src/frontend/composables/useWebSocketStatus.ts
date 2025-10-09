import { invoke } from '@tauri-apps/api/core'
import { onMounted, onUnmounted, ref } from 'vue'

export interface WebSocketConfig {
  enabled: boolean
  host: string
  port: number
  auto_connect: boolean
  api_key: string
}

export type WebSocketStatus = 'disconnected' | 'connecting' | 'connected' | 'error'

/**
 * WebSocket状态管理组合式函数
 */
export function useWebSocketStatus() {
  const config = ref<WebSocketConfig>({
    enabled: false,
    host: '127.0.0.1',
    port: 9000,
    auto_connect: true,
    api_key: '',
  })

  const status = ref<WebSocketStatus>('disconnected')
  const isLoading = ref(true)

  // 状态检查定时器
  let statusCheckInterval: number | null = null

  /**
   * 加载WebSocket配置
   */
  async function loadConfig() {
    try {
      const wsConfig = await invoke('get_websocket_config')
      config.value = wsConfig as WebSocketConfig
    }
    catch (error) {
      console.error('加载WebSocket配置失败:', error)
    }
  }

  /**
   * 检查WebSocket连接状态
   */
  async function checkStatus() {
    try {
      const wsStatus = await invoke('get_websocket_status') as string
      status.value = wsStatus as WebSocketStatus
    }
    catch (error) {
      console.error('检查WebSocket状态失败:', error)
      status.value = 'error'
    }
  }

  /**
   * 获取状态显示文本
   */
  function getStatusText(): string {
    if (!config.value.enabled) {
      return 'WebSocket 客户端已禁用'
    }

    switch (status.value) {
      case 'connected':
        return 'WebSocket 已连接'
      case 'connecting':
        return 'WebSocket 连接中'
      case 'error':
        return 'WebSocket 连接失败'
      default:
        return 'WebSocket 未连接'
    }
  }

  /**
   * 获取状态类型（用于UI样式）
   */
  function getStatusType(): 'success' | 'warning' | 'error' | 'default' {
    if (!config.value.enabled) {
      return 'default'
    }

    switch (status.value) {
      case 'connected':
        return 'success'
      case 'connecting':
        return 'warning'
      case 'error':
        return 'error'
      default:
        return 'default'
    }
  }

  /**
   * 是否显示脉冲动画
   */
  function shouldPulse(): boolean {
    return config.value.enabled && (status.value === 'connected' || status.value === 'connecting')
  }

  /**
   * 是否应该显示WebSocket状态
   */
  function shouldShow(): boolean {
    return config.value.enabled
  }

  /**
   * 开始状态检查
   */
  function startStatusCheck() {
    // 立即检查一次
    checkStatus()

    // 每3秒检查一次状态
    statusCheckInterval = window.setInterval(() => {
      checkStatus()
    }, 3000)
  }

  /**
   * 停止状态检查
   */
  function stopStatusCheck() {
    if (statusCheckInterval) {
      clearInterval(statusCheckInterval)
      statusCheckInterval = null
    }
  }

  /**
   * 初始化
   */
  async function initialize() {
    isLoading.value = true
    try {
      await loadConfig()
      if (config.value.enabled) {
        startStatusCheck()
      }
    }
    finally {
      isLoading.value = false
    }
  }

  /**
   * 刷新状态（用于配置变更后）
   */
  async function refresh() {
    await loadConfig()

    if (config.value.enabled) {
      if (!statusCheckInterval) {
        startStatusCheck()
      }
    }
    else {
      stopStatusCheck()
      status.value = 'disconnected'
    }
  }

  // 组件挂载时初始化
  onMounted(() => {
    initialize()
  })

  // 组件卸载时清理
  onUnmounted(() => {
    stopStatusCheck()
  })

  return {
    config,
    status,
    isLoading,
    getStatusText,
    getStatusType,
    shouldPulse,
    shouldShow,
    refresh,
    checkStatus,
  }
}
