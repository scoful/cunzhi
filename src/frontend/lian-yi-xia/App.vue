<script setup lang="ts">
import { ref, computed, onMounted, onUnmounted, defineComponent } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { open } from '@tauri-apps/plugin-dialog'
import {
  NConfigProvider,
  NMessageProvider,
  NNotificationProvider,
  NDialogProvider,
  NCard,
  NButton,
  NTag,
  NCode,
  NInput,
  NInputNumber,
  NSwitch,
  NSpace,
  NCollapse,
  NCollapseItem,
  NRadioGroup,
  NRadio,
  useMessage,
} from 'naive-ui'

// Message设置组件
const MessageSetup = defineComponent({
  name: 'MessageSetup',
  emits: ['setup'],
  setup(_, { emit }) {
    const message = useMessage()
    emit('setup', message)
    return () => null
  },
})

// 类型定义
interface ConnectedClient {
  client_id: string
  connected_at: string
}

interface ActivityLog {
  id: string
  timestamp: Date
  type: 'info' | 'success' | 'warning' | 'error'
  server_name: string
  message: string
}

interface SshTunnelConfig {
  enabled: boolean
  remote_host: string
  remote_user: string
  ssh_key_path: string | null
  remote_port: number  // 远程端口
  auto_start: boolean
  verbose_level: number  // 0=关闭, 1=-vvv
}

const appInfo = ref('')
const wsServerPort = ref(9000)
const originalWsServerPort = ref(9000)  // 原始端口,用于检测未保存更改
const connectedClients = ref<ConnectedClient[]>([])
const activityLogs = ref<ActivityLog[]>([])
const maxLogs = 200  // 最大日志数量 (心跳日志每30秒2条,200条约保留50分钟)
let statusCheckInterval: number | null = null

// WebSocket服务器状态
const wsServerStatus = ref<'running' | 'error'>('running')
const wsServerAddress = ref('127.0.0.1:9000')
const wsServerUptime = ref('0秒')
const wsServerClientCount = ref(0)

// SSH隧道相关状态
const sshTunnelConfig = ref<SshTunnelConfig | null>(null)
const originalSshTunnelConfig = ref<SshTunnelConfig | null>(null)  // 原始配置,用于检测未保存更改
const sshTunnelStatus = ref<'stopped' | 'starting' | 'running' | 'error'>('stopped')
const sshTunnelCommand = ref<string | null>(null)
const showSshConfig = ref(false)

// 检测WebSocket端口是否有未保存的更改
const hasUnsavedPortChanges = computed(() => {
  return wsServerPort.value !== originalWsServerPort.value
})

// 检测SSH隧道配置是否有未保存的更改
const hasUnsavedChanges = computed(() => {
  if (!sshTunnelConfig.value || !originalSshTunnelConfig.value) {
    return false
  }
  return JSON.stringify(sshTunnelConfig.value) !== JSON.stringify(originalSshTunnelConfig.value)
})

let message: ReturnType<typeof useMessage> | null = null

function setupMessage(msg: ReturnType<typeof useMessage>) {
  message = msg
}

// 添加活动日志
function addActivityLog(type: ActivityLog['type'], serverName: string, logMessage: string) {
  const log: ActivityLog = {
    id: `${Date.now()}-${Math.random()}`,
    timestamp: new Date(),
    type,
    server_name: serverName,
    message: logMessage,
  }

  activityLogs.value.unshift(log) // 新日志添加到开头

  // 限制日志数量
  if (activityLogs.value.length > maxLogs) {
    activityLogs.value = activityLogs.value.slice(0, maxLogs)
  }
}

// 清空活动日志
function clearActivityLogs() {
  activityLogs.value = []
  message?.success('日志已清空')
}

// 格式化时间
function formatTime(date: Date): string {
  const hours = date.getHours().toString().padStart(2, '0')
  const minutes = date.getMinutes().toString().padStart(2, '0')
  const seconds = date.getSeconds().toString().padStart(2, '0')
  return `${hours}:${minutes}:${seconds}`
}

// 加载已连接客户端
async function loadConnectedClients() {
  try {
    connectedClients.value = await invoke('get_connected_clients') as ConnectedClient[]
  } catch (error) {
    console.error('加载客户端列表失败:', error)
  }
}

// 加载WebSocket服务器状态
async function loadWsServerStatus() {
  try {
    const status = await invoke('get_ws_server_status') as {
      status: string
      address: string
      uptime: string
      client_count: number
    }

    if (status.status.startsWith('error:')) {
      wsServerStatus.value = 'error'
    } else {
      wsServerStatus.value = 'running'
    }

    wsServerAddress.value = status.address
    wsServerUptime.value = status.uptime
    wsServerClientCount.value = status.client_count
  } catch (error) {
    console.error('加载WebSocket服务器状态失败:', error)
    wsServerStatus.value = 'error'
  }
}

// 加载WebSocket服务器端口
async function loadWsServerPort() {
  try {
    const port = await invoke('get_ws_server_port') as number
    wsServerPort.value = port
    originalWsServerPort.value = port  // 保存原始端口
  } catch (error) {
    console.error('获取服务器端口失败:', error)
  }
}

// 保存WebSocket服务器端口
async function saveWsServerPort() {
  try {
    const oldPort = originalWsServerPort.value
    const newPort = wsServerPort.value

    // 如果SSH隧道的remote_port等于旧的WebSocket端口,自动更新为新端口
    if (sshTunnelConfig.value && sshTunnelConfig.value.remote_port === oldPort) {
      sshTunnelConfig.value.remote_port = newPort
      // 保存SSH隧道配置
      await invoke('update_ssh_tunnel_config', { config: sshTunnelConfig.value })
      originalSshTunnelConfig.value = JSON.parse(JSON.stringify(sshTunnelConfig.value))
      addActivityLog('info', 'SSH隧道', `远程端口已自动更新为 ${newPort}`)
    }

    await invoke('save_ws_server_port', { port: newPort })
    originalWsServerPort.value = newPort  // 更新原始端口
    message?.warning('端口已保存,请重启应用以生效')
    addActivityLog('success', 'WebSocket', `端口已更新为 ${newPort},需要重启应用`)
    // 重新加载SSH隧道命令(因为local_port变了)
    await loadSshTunnelCommand()
  } catch (error) {
    console.error('保存端口失败:', error)
    message?.error('保存端口失败')
  }
}

// 加载SSH隧道配置
async function loadSshTunnelConfig() {
  try {
    const config = await invoke('get_ssh_tunnel_config') as SshTunnelConfig | null
    // 如果配置为null,初始化默认配置
    if (!config) {
      sshTunnelConfig.value = {
        enabled: false,
        remote_host: '',
        remote_user: '',
        ssh_key_path: null,
        remote_port: wsServerPort.value,  // 默认使用本地端口
        auto_start: false,
        verbose_level: 0,  // 默认关闭Debug日志
      }
    } else {
      // 确保verbose_level和remote_port有默认值(兼容旧配置)
      sshTunnelConfig.value = {
        ...config,
        verbose_level: config.verbose_level ?? 0,
        remote_port: config.remote_port > 0 ? config.remote_port : wsServerPort.value,
      }
    }
    // 保存原始配置副本,用于检测未保存更改
    originalSshTunnelConfig.value = JSON.parse(JSON.stringify(sshTunnelConfig.value))
  } catch (error) {
    console.error('加载SSH隧道配置失败:', error)
  }
}

// 加载SSH隧道状态
async function loadSshTunnelStatus() {
  try {
    sshTunnelStatus.value = await invoke('get_ssh_tunnel_status') as 'stopped' | 'starting' | 'running' | 'error'
  } catch (error) {
    console.error('获取SSH隧道状态失败:', error)
  }
}

// 加载SSH隧道命令
async function loadSshTunnelCommand() {
  try {
    sshTunnelCommand.value = await invoke('get_ssh_tunnel_command') as string | null
  } catch (error) {
    console.error('获取SSH隧道命令失败:', error)
  }
}

// 保存SSH隧道配置
async function saveSshTunnelConfig() {
  try {
    await invoke('update_ssh_tunnel_config', { sshConfig: sshTunnelConfig.value })
    message?.success('SSH隧道配置已保存')
    addActivityLog('success', 'SSH隧道', '配置已保存')
    await loadSshTunnelCommand()
    // 保存成功后更新原始配置,清除未保存标记
    originalSshTunnelConfig.value = JSON.parse(JSON.stringify(sshTunnelConfig.value))
  } catch (error) {
    console.error('保存SSH隧道配置失败:', error)
    message?.error('保存配置失败')
    addActivityLog('error', 'SSH隧道', `保存配置失败: ${error}`)
  }
}

// 启动SSH隧道
async function startSshTunnel() {
  try {
    await invoke('start_ssh_tunnel')
    message?.success('SSH隧道已启动')
    addActivityLog('success', 'SSH隧道', '已启动')
    await loadSshTunnelStatus()
  } catch (error) {
    console.error('启动SSH隧道失败:', error)
    message?.error(`启动失败: ${error}`)
    addActivityLog('error', 'SSH隧道', `启动失败: ${error}`)
  }
}

// 停止SSH隧道
async function stopSshTunnel() {
  try {
    await invoke('stop_ssh_tunnel')
    message?.success('SSH隧道已停止')
    addActivityLog('info', 'SSH隧道', '已停止')
    await loadSshTunnelStatus()
  } catch (error) {
    console.error('停止SSH隧道失败:', error)
    message?.error(`停止失败: ${error}`)
    addActivityLog('error', 'SSH隧道', `停止失败: ${error}`)
  }
}

// 重启SSH隧道
async function restartSshTunnel() {
  try {
    await invoke('restart_ssh_tunnel')
    message?.success('SSH隧道已重启')
    addActivityLog('success', 'SSH隧道', '已重启')
    await loadSshTunnelStatus()
  } catch (error) {
    console.error('重启SSH隧道失败:', error)
    message?.error(`重启失败: ${error}`)
    addActivityLog('error', 'SSH隧道', `重启失败: ${error}`)
  }
}

// 复制SSH隧道命令
async function copySshTunnelCommand() {
  try {
    if (sshTunnelCommand.value) {
      await navigator.clipboard.writeText(sshTunnelCommand.value)
      message?.success('SSH命令已复制到剪贴板')
    } else {
      message?.warning('请先配置SSH隧道')
    }
  } catch (error) {
    console.error('复制失败:', error)
    message?.error('复制失败')
  }
}

// 选择SSH密钥文件
async function selectSshKeyFile() {
  try {
    const selected = await open({
      multiple: false,
      directory: false,
      title: '选择SSH密钥文件',
    })

    if (selected && sshTunnelConfig.value) {
      sshTunnelConfig.value.ssh_key_path = selected as string
    }
  } catch (error) {
    console.error('选择文件失败:', error)
    message?.error('选择文件失败')
  }
}

// 启动客户端列表刷新定时器
function startClientRefresh() {
  loadConnectedClients()
  loadWsServerStatus()
  loadSshTunnelStatus()
  statusCheckInterval = window.setInterval(() => {
    loadConnectedClients()
    loadWsServerStatus()
    loadSshTunnelStatus()
  }, 3000)
}

// 停止刷新定时器
function stopClientRefresh() {
  if (statusCheckInterval) {
    clearInterval(statusCheckInterval)
    statusCheckInterval = null
  }
}

// 获取应用信息
async function getAppInfo() {
  try {
    appInfo.value = await invoke('get_lian_yi_xia_app_info')
  } catch (error) {
    console.error('获取应用信息失败:', error)
  }
}

// 初始化
onMounted(async () => {
  try {
    await getAppInfo()
    await loadWsServerPort()
    await loadSshTunnelConfig()
    await loadSshTunnelCommand()
    await loadConnectedClients()
    startClientRefresh()
    addActivityLog('success', '系统', '连一下启动成功')

    listen('ws_log', (event: any) => {
      const { type, server_name, message } = event.payload
      addActivityLog(type, server_name, message)
    })

    // 监听SSH隧道日志事件
    listen('log-event', (event: any) => {
      const logMessage = event.payload as string
      // SSH日志显示为info类型
      addActivityLog('info', 'SSH', logMessage)
    })

    // 监听SSH隧道状态变更事件
    listen('ssh-tunnel-status', (event: any) => {
      const status = event.payload as string
      if (status === 'running') {
        sshTunnelStatus.value = 'running'
        addActivityLog('success', 'SSH隧道', '隧道已成功建立')
      } else if (status === 'error') {
        sshTunnelStatus.value = 'error'
        addActivityLog('error', 'SSH隧道', '隧道建立失败或进程异常退出')
      } else if (status === 'stopped') {
        sshTunnelStatus.value = 'stopped'
      }
    })
  } catch (error) {
    console.error('初始化失败:', error)
    addActivityLog('error', '系统', '初始化失败')
  }
})

// 清理
onUnmounted(() => {
  stopClientRefresh()
})
</script>

<template>
  <div class="min-h-screen bg-gray-50 p-4">
    <n-config-provider>
      <n-message-provider>
        <n-notification-provider>
          <n-dialog-provider>
            <MessageSetup @setup="setupMessage" />
            <div class="max-w-6xl mx-auto">
              <!-- 标题栏 -->
              <div class="mb-6">
                <h1 class="text-2xl font-bold text-gray-800 mb-2">
                  {{ appInfo || '连一下 - WebSocket服务器' }}
                </h1>
                <p class="text-gray-600">
                  接收远程"寸止"实例连接，转发弹窗请求到本地"等一下"
                </p>
              </div>

              <!-- 服务器状态卡片 -->
              <n-card title="WebSocket服务器" class="mb-6">
                <div class="space-y-3">
                  <!-- 服务器状态 -->
                  <div class="flex items-center gap-2">
                    <span class="w-24 font-medium">状态:</span>
                    <n-tag v-if="wsServerStatus === 'running'" type="success">运行中</n-tag>
                    <n-tag v-else type="error">错误</n-tag>
                  </div>

                  <!-- 监听地址 -->
                  <div class="flex items-center gap-2">
                    <span class="w-24">监听地址:</span>
                    <n-tag type="info">{{ wsServerAddress }}</n-tag>
                  </div>

                  <!-- 运行时长 -->
                  <div class="flex items-center gap-2">
                    <span class="w-24">运行时长:</span>
                    <span class="text-gray-600">{{ wsServerUptime }}</span>
                  </div>

                  <!-- 客户端数量 -->
                  <div class="flex items-center gap-2">
                    <span class="w-24">客户端数:</span>
                    <n-tag :type="wsServerClientCount > 0 ? 'success' : 'default'">
                      {{ wsServerClientCount }}
                    </n-tag>
                  </div>

                  <n-divider />

                  <!-- 端口配置 -->
                  <div class="flex items-center gap-2">
                    <span class="w-24">端口配置:</span>
                    <n-input-number
                      v-model:value="wsServerPort"
                      :min="1"
                      :max="65535"
                      placeholder="WebSocket服务器端口"
                      style="flex: 1"
                    />
                    <n-button
                      type="primary"
                      :disabled="!hasUnsavedPortChanges"
                      @click="saveWsServerPort"
                    >
                      {{ hasUnsavedPortChanges ? '保存端口 *' : '保存端口' }}
                    </n-button>
                  </div>

                  <!-- 提示信息 -->
                  <div v-if="hasUnsavedPortChanges" class="text-orange-500 text-sm">
                    ⚠️ 修改端口后需要重启应用才能生效
                  </div>
                </div>
              </n-card>

              <!-- SSH隧道配置卡片 -->
              <n-card title="SSH隧道管理" class="mb-6">
                <div class="space-y-4">
                  <!-- 状态显示 -->
                  <div class="flex items-center gap-2">
                    <span class="font-medium">状态:</span>
                    <n-tag v-if="sshTunnelStatus === 'stopped'" type="default">已停止</n-tag>
                    <n-tag v-else-if="sshTunnelStatus === 'starting'" type="warning">启动中</n-tag>
                    <n-tag v-else-if="sshTunnelStatus === 'running'" type="success">运行中</n-tag>
                    <n-tag v-else type="error">错误</n-tag>
                  </div>

                  <!-- 控制按钮 -->
                  <n-space>
                    <n-button
                      type="primary"
                      :disabled="sshTunnelStatus === 'running' || !sshTunnelConfig?.enabled"
                      @click="startSshTunnel"
                    >
                      启动
                    </n-button>
                    <n-button
                      :disabled="sshTunnelStatus === 'stopped'"
                      @click="stopSshTunnel"
                    >
                      停止
                    </n-button>
                    <n-button
                      :disabled="sshTunnelStatus === 'stopped'"
                      @click="restartSshTunnel"
                    >
                      重启
                    </n-button>
                  </n-space>

                  <!-- SSH命令显示 -->
                  <div v-if="sshTunnelCommand">
                    <div class="font-medium mb-2">SSH命令:</div>
                    <div style="background-color: #f0fdf4; border: 1px solid #86efac; border-radius: 3px; padding: 12px;">
                      <n-code
                        :code="sshTunnelCommand"
                        language="bash"
                        word-wrap
                      />
                    </div>
                  </div>

                  <!-- 配置折叠面板 -->
                  <n-collapse>
                    <n-collapse-item title="SSH隧道配置" name="ssh-config">
                      <div v-if="sshTunnelConfig" class="space-y-3">
                        <!-- 启用开关 -->
                        <div class="flex items-center gap-2">
                          <span class="w-24">启用:</span>
                          <n-switch v-model:value="sshTunnelConfig.enabled" />
                        </div>

                        <!-- 远程主机 -->
                        <div class="flex items-center gap-2">
                          <span class="w-24">远程主机:</span>
                          <n-input
                            v-model:value="sshTunnelConfig.remote_host"
                            placeholder="例: example.com"
                            :disabled="!sshTunnelConfig.enabled"
                            style="flex: 1"
                          />
                        </div>

                        <!-- 远程用户 -->
                        <div class="flex items-center gap-2">
                          <span class="w-24">远程用户:</span>
                          <n-input
                            v-model:value="sshTunnelConfig.remote_user"
                            placeholder="例: user"
                            :disabled="!sshTunnelConfig.enabled"
                            style="flex: 1"
                          />
                        </div>

                        <!-- 远程端口 -->
                        <div class="flex items-center gap-2">
                          <span class="w-24">远程端口:</span>
                          <n-input-number
                            v-model:value="sshTunnelConfig.remote_port"
                            :min="1"
                            :max="65535"
                            placeholder="远程服务器监听端口"
                            :disabled="!sshTunnelConfig.enabled"
                            style="flex: 1"
                          />
                        </div>

                        <!-- SSH密钥路径 -->
                        <div class="flex flex-col gap-1">
                          <div class="flex items-center gap-2">
                            <span class="w-24">密钥路径:</span>
                            <n-input
                              v-model:value="sshTunnelConfig.ssh_key_path"
                              placeholder="可选,例: ~/.ssh/id_rsa"
                              :disabled="!sshTunnelConfig.enabled"
                              style="flex: 1"
                            />
                            <n-button
                              size="small"
                              @click="selectSshKeyFile"
                              :disabled="!sshTunnelConfig.enabled"
                            >
                              浏览
                            </n-button>
                          </div>
                          <div class="text-gray-500 text-xs ml-24">
                            选择SSH私钥文件(如 ~/.ssh/id_rsa),公钥需提前部署到远程服务器的 ~/.ssh/authorized_keys
                          </div>
                        </div>

                        <!-- 自动启动 -->
                        <div class="flex items-center gap-2">
                          <span class="w-24">自动启动:</span>
                          <n-switch
                            v-model:value="sshTunnelConfig.auto_start"
                            :disabled="!sshTunnelConfig.enabled"
                          />
                        </div>

                        <!-- Debug日志级别 -->
                        <div class="flex flex-col gap-1">
                          <div class="flex items-center gap-2">
                            <span class="w-24">Debug 日志:</span>
                            <n-radio-group
                              v-model:value="sshTunnelConfig.verbose_level"
                              :disabled="!sshTunnelConfig.enabled"
                            >
                              <n-radio :value="0">关闭</n-radio>
                              <n-radio :value="1">开启(-vvv)</n-radio>
                            </n-radio-group>
                          </div>
                          <div class="text-gray-500 text-xs ml-24">
                            控制SSH连接的调试信息输出到活动日志(最详细级别)
                          </div>
                        </div>

                        <!-- 保存按钮 -->
                        <div class="flex justify-end">
                          <n-button
                            :type="hasUnsavedChanges ? 'warning' : 'primary'"
                            @click="saveSshTunnelConfig"
                          >
                            保存配置{{ hasUnsavedChanges ? ' *' : '' }}
                          </n-button>
                        </div>
                      </div>
                      <div v-else class="text-gray-500">
                        配置未初始化
                      </div>
                    </n-collapse-item>
                  </n-collapse>
                </div>
              </n-card>

              <!-- 主要内容区域 -->
              <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
                <!-- 已连接客户端列表 -->
                <n-card title="已连接客户端" class="h-fit">
                  <div class="space-y-2">
                    <div v-if="connectedClients.length === 0" class="text-center text-gray-500 py-8">
                      暂无客户端连接
                    </div>

                    <div v-for="client in connectedClients" :key="client.client_id" class="border rounded-lg p-3">
                      <div class="flex items-center justify-between">
                        <div>
                          <div class="font-medium">{{ client.client_id }}</div>
                          <div class="text-sm text-gray-500">连接时间: {{ client.connected_at }}</div>
                        </div>
                        <n-tag type="success">在线</n-tag>
                      </div>
                    </div>
                  </div>
                </n-card>

                <!-- 活动日志 -->
                <n-card title="活动日志" class="h-fit">
                  <template #header-extra>
                    <n-button size="small" @click="clearActivityLogs">
                      清空日志
                    </n-button>
                  </template>

                  <div class="h-64 overflow-y-auto bg-gray-50 p-3 rounded font-mono text-sm">
                    <div v-if="activityLogs.length === 0" class="text-center text-gray-400 py-8">
                      暂无活动日志
                    </div>
                    <div
                      v-for="log in activityLogs"
                      :key="log.id"
                      class="mb-1 flex items-start gap-2"
                      :class="{
                        'text-blue-600': log.type === 'info',
                        'text-green-600': log.type === 'success',
                        'text-yellow-600': log.type === 'warning',
                        'text-red-600': log.type === 'error',
                      }"
                    >
                      <span class="text-gray-500 flex-shrink-0">[{{ formatTime(log.timestamp) }}]</span>
                      <span class="font-medium flex-shrink-0">[{{ log.server_name }}]</span>
                      <span class="flex-1">{{ log.message }}</span>
                    </div>
                  </div>
                </n-card>
              </div>
            </div>
          </n-dialog-provider>
        </n-notification-provider>
      </n-message-provider>
    </n-config-provider>
  </div>
</template>

<style scoped>
/* 自定义样式 */
</style>

