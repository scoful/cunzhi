<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { defineComponent, h, onMounted, onUnmounted, ref } from 'vue'
import { useMessage } from 'naive-ui'

// Message设置组件
const MessageSetup = defineComponent({
  name: 'MessageSetup',
  emits: ['setup'],
  setup(_, { emit }) {
    const message = useMessage()
    emit('setup', message)
    return () => h('div', { style: { display: 'none' } })
  }
})

// 类型定义
interface WebSocketServerConfig {
  id: string
  name: string
  host: string
  port: number
  api_key: string
  enabled: boolean
  auto_connect: boolean
}

interface WebSocketServersConfig {
  servers: WebSocketServerConfig[]
}

type ConnectionStatus =
  | { type: 'disconnected' }
  | { type: 'connecting' }
  | { type: 'connected' }
  | { type: 'error', message: string }

interface ActivityLog {
  id: string
  timestamp: Date
  type: 'info' | 'success' | 'warning' | 'error'
  server_name: string
  message: string
}

const appInfo = ref('')
const servers = ref<WebSocketServerConfig[]>([])
const showAddDialog = ref(false)
const editingServer = ref<WebSocketServerConfig | null>(null)
const visibleApiKeys = ref<Set<string>>(new Set())
const connectionStatus = ref<Map<string, ConnectionStatus>>(new Map())
const activityLogs = ref<ActivityLog[]>([])
const maxLogs = 100 // 最大日志条数
let statusCheckInterval: number | null = null

// Message API实例（在模板挂载后初始化）
let message: any = null

// 设置message实例
function setupMessage(messageInstance: any) {
  message = messageInstance
}

// 切换API密钥显示/隐藏
function toggleApiKeyVisibility(serverId: string) {
  if (visibleApiKeys.value.has(serverId)) {
    visibleApiKeys.value.delete(serverId)
  } else {
    visibleApiKeys.value.add(serverId)
  }
}

// 获取显示的API密钥
function getDisplayApiKey(server: WebSocketServerConfig) {
  if (!server.api_key) return '未设置'
  if (visibleApiKeys.value.has(server.id)) {
    return server.api_key
  }
  return '••••••••'
}

// 复制API密钥环境变量
async function copyApiKeyEnv(server: WebSocketServerConfig) {
  try {
    const envText = `CUNZHI_WS_API_KEY=${server.api_key}`
    await navigator.clipboard.writeText(envText)
    message?.success('环境变量已复制到剪贴板')
  } catch (error) {
    console.error('复制失败:', error)
    message?.error('复制失败')
  }
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

// 获取连接状态
function getConnectionStatus(serverId: string): ConnectionStatus {
  return connectionStatus.value.get(serverId) || { type: 'disconnected' }
}

// 检查服务器是否已连接
function isServerConnected(serverId: string): boolean {
  const status = getConnectionStatus(serverId)
  return status.type === 'connected' || status.type === 'connecting'
}

// 获取状态显示文本
function getStatusText(status: ConnectionStatus): string {
  switch (status.type) {
    case 'connected': return '已连接'
    case 'connecting': return '连接中'
    case 'error': return '错误'
    default: return '未连接'
  }
}

// 获取状态标签类型
function getStatusType(status: ConnectionStatus): 'success' | 'warning' | 'error' | 'default' {
  switch (status.type) {
    case 'connected': return 'success'
    case 'connecting': return 'warning'
    case 'error': return 'error'
    default: return 'default'
  }
}

// 检查所有服务器的连接状态
async function checkAllConnectionStatus() {
  try {
    const statusMap = await invoke('get_all_connection_status') as Record<string, ConnectionStatus>
    const newStatusMap = new Map(Object.entries(statusMap))

    // 检测状态变化并记录日志
    for (const [serverId, newStatus] of newStatusMap.entries()) {
      const oldStatus = connectionStatus.value.get(serverId)
      const server = servers.value.find(s => s.id === serverId)

      if (server) {
        // 如果是首次检查(oldStatus不存在)且状态为connected,记录自动连接日志
        if (!oldStatus && newStatus.type === 'connected') {
          addActivityLog('success', server.name, '自动连接成功')
        }
        // 如果状态发生变化,记录变化日志
        else if (oldStatus && oldStatus.type !== newStatus.type) {
          if (newStatus.type === 'connected') {
            addActivityLog('success', server.name, '连接成功')
          } else if (newStatus.type === 'disconnected') {
            addActivityLog('info', server.name, '已断开连接')
          } else if (newStatus.type === 'error') {
            const errorMsg = newStatus.type === 'error' ? newStatus.message : '连接错误'
            addActivityLog('error', server.name, `连接错误: ${errorMsg}`)
          }
        }
      }
    }

    connectionStatus.value = newStatusMap
  } catch (error) {
    console.error('检查连接状态失败:', error)
  }
}

// 连接到服务器
async function connectToServer(serverId: string) {
  const server = servers.value.find(s => s.id === serverId)
  if (!server) return

  // 立即设置为连接中状态，提供即时反馈
  connectionStatus.value.set(serverId, { type: 'connecting' })
  addActivityLog('info', server.name, '正在连接...')
  message?.success('正在连接...')

  try {
    await invoke('connect_to_server', { serverId })
    // 立即检查一次状态
    await checkAllConnectionStatus()
  } catch (error) {
    console.error('连接失败:', error)
    addActivityLog('error', server.name, String(error))
    message?.error(String(error))
    // 连接失败时，立即更新状态为error，确保按钮可以重试
    connectionStatus.value.set(serverId, { type: 'error', message: String(error) })
  }
}

// 断开服务器连接
async function disconnectFromServer(serverId: string) {
  const server = servers.value.find(s => s.id === serverId)
  if (!server) return

  // 立即设置为未连接状态
  connectionStatus.value.set(serverId, { type: 'disconnected' })

  try {
    await invoke('disconnect_from_server', { serverId })
    addActivityLog('info', server.name, '已断开连接')
    message?.success('已断开连接')
    await checkAllConnectionStatus()
  } catch (error) {
    console.error('断开失败:', error)
    addActivityLog('error', server.name, String(error))
    message?.error(String(error))
  }
}

// 启动状态检查定时器
function startStatusCheck() {
  checkAllConnectionStatus()
  statusCheckInterval = window.setInterval(() => {
    checkAllConnectionStatus()
  }, 3000) // 每3秒检查一次
}

// 停止状态检查定时器
function stopStatusCheck() {
  if (statusCheckInterval) {
    clearInterval(statusCheckInterval)
    statusCheckInterval = null
  }
}

// 表单数据
const formData = ref({
  name: '',
  host: '127.0.0.1',
  port: 9000,
  api_key: '',
  enabled: true,
  auto_connect: true,
})

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
    await loadServers()
    startStatusCheck()
    // 添加启动日志
    addActivityLog('success', '系统', '连一下启动成功')

    // 监听WebSocket日志事件
    listen('ws_log', (event: any) => {
      const { type, server_name, message } = event.payload
      addActivityLog(type, server_name, message)
    })
  } catch (error) {
    console.error('初始化失败:', error)
    addActivityLog('error', '系统', '初始化失败')
  }
})

// 清理
onUnmounted(() => {
  stopStatusCheck()
})

// 加载服务器列表
async function loadServers() {
  try {
    const config = await invoke('get_websocket_servers') as WebSocketServersConfig
    servers.value = config.servers
  } catch (error) {
    console.error('加载服务器列表失败:', error)
    message?.error('加载服务器列表失败')
  }
}

// 从配置文件重新加载服务器列表
async function reloadServersFromConfig() {
  try {
    servers.value = await invoke('reload_servers_from_config') as WebSocketServerConfig[]
    addActivityLog('success', '系统', '已从配置文件重新加载服务器列表')
    message?.success('配置已刷新')
    // 刷新后立即检查连接状态
    await checkAllConnectionStatus()
  } catch (error) {
    console.error('刷新配置失败:', error)
    addActivityLog('error', '系统', `刷新配置失败: ${error}`)
    message?.error(`刷新配置失败: ${error}`)
  }
}

// 生成API密钥
async function generateApiKey() {
  try {
    const apiKey = await invoke('generate_api_key') as string
    formData.value.api_key = apiKey
    message?.success('API密钥已生成')
  } catch (error) {
    console.error('生成API密钥失败:', error)
    message?.error('生成API密钥失败')
  }
}

// 打开添加对话框
function openAddDialog() {
  editingServer.value = null
  formData.value = {
    name: '',
    host: '127.0.0.1',
    port: 9000,
    api_key: '',
    enabled: true,
    auto_connect: true,
  }
  showAddDialog.value = true
}

// 打开编辑对话框
function openEditDialog(server: WebSocketServerConfig) {
  editingServer.value = server
  formData.value = { ...server }
  showAddDialog.value = true
}

// 保存服务器
async function saveServer() {
  if (!formData.value.name.trim()) {
    message?.error('请输入服务器名称')
    return
  }

  if (!formData.value.host.trim()) {
    message?.error('请输入服务器地址')
    return
  }

  if (!formData.value.api_key.trim()) {
    message?.error('请输入API密钥')
    return
  }

  // 检查服务器名称的唯一性
  const isNameDuplicate = servers.value.some(server => {
    // 编辑时排除当前服务器
    if (editingServer.value && server.id === editingServer.value.id) {
      return false
    }
    return server.name === formData.value.name
  })

  if (isNameDuplicate) {
    message?.error(`服务器名称 "${formData.value.name}" 已存在`)
    return
  }

  // 检查IP+端口的唯一性
  const isAddressDuplicate = servers.value.some(server => {
    // 编辑时排除当前服务器
    if (editingServer.value && server.id === editingServer.value.id) {
      return false
    }
    return server.host === formData.value.host && server.port === formData.value.port
  })

  if (isAddressDuplicate) {
    message?.error(`服务器 ${formData.value.host}:${formData.value.port} 已存在`)
    return
  }

  try {
    if (editingServer.value) {
      // 更新服务器
      await invoke('update_websocket_server', {
        serverConfig: {
          id: editingServer.value.id,
          ...formData.value,
        },
      })
      addActivityLog('success', formData.value.name, '服务器配置已更新')
      message?.success('服务器配置已更新')
    } else {
      // 添加服务器
      await invoke('add_websocket_server', {
        name: formData.value.name,
        host: formData.value.host,
        port: formData.value.port,
        apiKey: formData.value.api_key,
        enabled: formData.value.enabled,
        autoConnect: formData.value.auto_connect,
      })
      addActivityLog('success', formData.value.name, '服务器已添加')
      message?.success('服务器已添加')
    }

    showAddDialog.value = false
    await loadServers()
  } catch (error) {
    console.error('保存服务器失败:', error)
    const errorMessage = typeof error === 'string' ? error : '保存服务器失败'
    addActivityLog('error', formData.value.name, `保存失败: ${errorMessage}`)
    message?.error(errorMessage)
  }
}

// 切换服务器启用状态
async function toggleServerEnabled(server: WebSocketServerConfig) {
  try {
    const newEnabled = !server.enabled
    await invoke('update_websocket_server', {
      serverConfig: {
        ...server,
        enabled: newEnabled,
      },
    })
    const action = newEnabled ? '已启用' : '已禁用'
    addActivityLog('info', server.name, `服务器${action}`)
    message?.success(`服务器${action}`)
    await loadServers()
  } catch (error) {
    console.error('切换服务器状态失败:', error)
    addActivityLog('error', server.name, '切换状态失败')
    message?.error('切换服务器状态失败')
  }
}

// 删除服务器
async function deleteServer(serverId: string) {
  const server = servers.value.find(s => s.id === serverId)
  if (!server) return

  try {
    await invoke('delete_websocket_server', { serverId: serverId })
    addActivityLog('info', server.name, '服务器已删除')
    message?.success('服务器已删除')
    await loadServers()
  } catch (error) {
    console.error('删除服务器失败:', error)
    addActivityLog('error', server.name, '删除失败')
    message?.error('删除服务器失败')
  }
}

onMounted(() => {
  getAppInfo()
  loadServers()
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
                  {{ appInfo || '连一下 - WebSocket管理器' }}
                </h1>
                <p class="text-gray-600">
                  管理多个WebSocket服务器连接，接收消息并启动"等一下"界面
                </p>
              </div>

              <!-- 主要内容区域 -->
              <div class="grid grid-cols-1 lg:grid-cols-2 gap-6">
                <!-- 服务器列表 -->
                <n-card title="WebSocket服务器" class="h-fit">
                  <template #header-extra>
                    <div class="flex gap-2">
                      <n-button @click="reloadServersFromConfig">
                        刷新配置
                      </n-button>
                      <n-button type="primary" @click="openAddDialog">
                        添加服务器
                      </n-button>
                    </div>
                  </template>

                  <div class="space-y-4">
                    <div v-if="servers.length === 0" class="text-center text-gray-500 py-8">
                      暂无配置的服务器
                    </div>

                    <div v-for="server in servers" :key="server.id" class="border rounded-lg p-4">
                      <!-- 第1行: 4个按钮靠右 -->
                      <div class="flex justify-end gap-2 -mb-4">
                        <n-button
                          v-if="getConnectionStatus(server.id).type === 'disconnected' || getConnectionStatus(server.id).type === 'error'"
                          size="small"
                          type="primary"
                          :disabled="!server.enabled"
                          @click="connectToServer(server.id)"
                        >
                          连接
                        </n-button>
                        <n-button
                          v-else-if="getConnectionStatus(server.id).type === 'connected'"
                          size="small"
                          type="warning"
                          @click="disconnectFromServer(server.id)"
                        >
                          断开
                        </n-button>
                        <n-button
                          v-else-if="getConnectionStatus(server.id).type === 'connecting'"
                          size="small"
                          type="default"
                          disabled
                        >
                          连接中...
                        </n-button>
                        <n-button
                          size="small"
                          :type="server.enabled ? 'warning' : 'success'"
                          :disabled="isServerConnected(server.id)"
                          @click="toggleServerEnabled(server)"
                        >
                          {{ server.enabled ? '禁用' : '启用' }}
                        </n-button>
                        <n-button
                          size="small"
                          :disabled="isServerConnected(server.id)"
                          @click="openEditDialog(server)"
                        >
                          编辑
                        </n-button>
                        <n-button
                          size="small"
                          type="error"
                          :disabled="isServerConnected(server.id)"
                          @click="deleteServer(server.id)"
                        >
                          删除
                        </n-button>
                      </div>

                      <!-- 第2行: 服务器名 + 状态标签 -->
                      <div class="flex items-center gap-2 mb-1">
                        <h3 class="font-medium">{{ server.name }}</h3>
                        <n-tag :type="server.enabled ? 'success' : 'default'" size="small">
                          {{ server.enabled ? '启用' : '禁用' }}
                        </n-tag>
                        <n-tag :type="getStatusType(getConnectionStatus(server.id))" size="small">
                          {{ getStatusText(getConnectionStatus(server.id)) }}
                        </n-tag>
                      </div>

                      <!-- 第3行开始: 其他信息 -->
                      <div class="text-sm text-gray-600 space-y-1">
                        <div>地址: {{ server.host }}:{{ server.port }}</div>
                        <div>自动连接: {{ server.auto_connect ? '是' : '否' }}</div>
                        <div class="flex items-center gap-2">
                          <span class="truncate" :title="'API密钥: ' + (server.api_key || '未设置')">API密钥: {{ getDisplayApiKey(server) }}</span>
                          <div class="flex gap-1 flex-shrink-0">
                            <n-button
                              v-if="server.api_key"
                              text
                              size="tiny"
                              @click="toggleApiKeyVisibility(server.id)"
                              @blur="() => visibleApiKeys.delete(server.id)"
                            >
                              {{ visibleApiKeys.has(server.id) ? '隐藏' : '显示' }}
                            </n-button>
                            <n-button
                              v-if="server.api_key"
                              text
                              size="tiny"
                              @click="copyApiKeyEnv(server)"
                            >
                              复制
                            </n-button>
                          </div>
                        </div>
                      </div>
                    </div>
                  </div>
                </n-card>


              </div>

              <!-- 日志区域 -->
              <n-card title="活动日志" class="mt-6">
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

            <!-- 添加/编辑服务器对话框 -->
            <n-modal v-model:show="showAddDialog" preset="dialog" :title="editingServer ? '编辑服务器' : '添加服务器'">
              <n-form :model="formData" label-placement="left" label-width="100px">
                <n-form-item label="服务器名称" required>
                  <n-input v-model:value="formData.name" placeholder="请输入服务器名称" />
                </n-form-item>

                <n-form-item label="服务器地址" required>
                  <n-input v-model:value="formData.host" placeholder="127.0.0.1" />
                </n-form-item>

                <n-form-item label="端口" required>
                  <n-input-number v-model:value="formData.port" :min="1" :max="65535" />
                </n-form-item>

                <n-form-item label="API密钥" required>
                  <n-input-group>
                    <n-input
                      v-model:value="formData.api_key"
                      type="password"
                      show-password-on="click"
                      placeholder="请输入或点击生成"
                    />
                    <n-button @click="generateApiKey">生成</n-button>
                  </n-input-group>
                </n-form-item>

                <n-form-item label="自动连接">
                  <n-switch v-model:value="formData.auto_connect" />
                </n-form-item>
              </n-form>

              <template #action>
                <n-space>
                  <n-button @click="showAddDialog = false">取消</n-button>
                  <n-button type="primary" @click="saveServer">保存</n-button>
                </n-space>
              </template>
            </n-modal>
          </n-dialog-provider>
        </n-notification-provider>
      </n-message-provider>
    </n-config-provider>
  </div>
</template>

<style scoped>
/* 自定义样式 */
</style>
