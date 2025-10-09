<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { useMessage } from 'naive-ui'
import { computed, onMounted, ref } from 'vue'

const message = useMessage()

// 本地状态 - 自管理模式
const localConfig = ref({
  enabled: false,
  host: '127.0.0.1',
  port: 9000,
  auto_connect: true, // 默认开启
  api_key: '', // API密钥
})

// 连接状态
const connectionStatus = ref('disconnected') // 'disconnected' | 'connecting' | 'connected' | 'error'
const connectionError = ref('')
const isConnecting = ref(false)

// API Key 相关状态
const isGeneratingKey = ref(false)
const showApiKey = ref(false)

// 计算连接状态显示
const statusText = computed(() => {
  switch (connectionStatus.value) {
    case 'connected':
      return '已连接'
    case 'connecting':
      return '连接中...'
    case 'error':
      return '连接失败'
    default:
      return '未连接'
  }
})

const statusColor = computed(() => {
  switch (connectionStatus.value) {
    case 'connected':
      return 'success'
    case 'connecting':
      return 'warning'
    case 'error':
      return 'error'
    default:
      return 'default'
  }
})

// API Key 显示文本（部分遮掩）
const maskedApiKey = computed(() => {
  if (!localConfig.value.api_key) {
    return '未设置'
  }
  if (showApiKey.value) {
    return localConfig.value.api_key
  }
  // 显示前8位和后4位，中间用*代替
  const key = localConfig.value.api_key
  if (key.length <= 12) {
    return '*'.repeat(key.length)
  }
  return key.substring(0, 8) + '*'.repeat(key.length - 12) + key.substring(key.length - 4)
})

// 加载WebSocket配置
async function loadWebSocketConfig() {
  try {
    const config = await invoke('get_websocket_config')
    localConfig.value = config as any
  }
  catch (error) {
    console.error('加载WebSocket配置失败:', error)
  }
}

// 更新配置
async function updateConfig() {
  try {
    await invoke('update_websocket_config', {
      websocketConfig: localConfig.value,
    })
    message.success('WebSocket配置已保存')
  }
  catch (error) {
    console.error('保存WebSocket配置失败:', error)
    message.error('保存WebSocket配置失败')
  }
}

// 生成新的API Key
async function generateApiKey() {
  if (isGeneratingKey.value)
    return

  try {
    isGeneratingKey.value = true
    const newApiKey = await invoke('generate_websocket_api_key')
    localConfig.value.api_key = newApiKey as string
    await updateConfig()
    message.success('API Key已生成并保存')
  }
  catch (error) {
    console.error('生成API Key失败:', error)
    message.error('生成API Key失败')
  }
  finally {
    isGeneratingKey.value = false
  }
}

// 复制API Key到剪贴板
async function copyApiKey() {
  if (!localConfig.value.api_key) {
    message.warning('请先生成API Key')
    return
  }

  try {
    await navigator.clipboard.writeText(localConfig.value.api_key)
    message.success('API Key已复制到剪贴板')
  }
  catch (error) {
    console.error('复制失败:', error)
    message.error('复制失败，请手动复制')
  }
}

// 切换API Key显示/隐藏
function toggleApiKeyVisibility() {
  showApiKey.value = !showApiKey.value
}

// 切换启用状态
function toggleEnabled(enabled: boolean) {
  localConfig.value.enabled = enabled
  updateConfig()

  if (!enabled && connectionStatus.value === 'connected') {
    // 如果禁用且当前已连接，则断开连接
    disconnectWebSocket()
  }
}

// 更新主机地址
function updateHost(host: string) {
  localConfig.value.host = host
  updateConfig()
}

// 更新端口
function updatePort(port: number) {
  localConfig.value.port = port
  updateConfig()
}

// 切换自动连接
function toggleAutoConnect(auto_connect: boolean) {
  localConfig.value.auto_connect = auto_connect
  updateConfig()
}

// 连接WebSocket
async function connectWebSocket() {
  if (isConnecting.value)
    return

  try {
    isConnecting.value = true
    connectionStatus.value = 'connecting'
    connectionError.value = ''

    const serverUrl = `ws://${localConfig.value.host}:${localConfig.value.port}`
    await invoke('connect_websocket', { serverUrl })

    connectionStatus.value = 'connected'
    message.success('WebSocket连接成功')
  }
  catch (error) {
    connectionStatus.value = 'error'
    connectionError.value = error as string
    message.error(`连接失败: ${error}`)
  }
  finally {
    isConnecting.value = false
  }
}

// 断开WebSocket连接
async function disconnectWebSocket() {
  try {
    await invoke('disconnect_websocket')
    connectionStatus.value = 'disconnected'
    connectionError.value = ''
    message.info('WebSocket连接已断开')
  }
  catch (error) {
    message.error(`断开连接失败: ${error}`)
  }
}

// 检查连接状态
async function checkConnectionStatus() {
  try {
    const status = await invoke('get_websocket_status') as string
    connectionStatus.value = status
  }
  catch (error) {
    console.error('检查WebSocket状态失败:', error)
  }
}

// 组件挂载时加载配置和检查状态
onMounted(async () => {
  await loadWebSocketConfig()
  checkConnectionStatus()
})
</script>

<template>
  <!-- 设置内容 -->
  <n-space vertical size="large">
    <!-- WebSocket客户端开关 -->
    <div class="flex items-center justify-between">
      <div class="flex items-center">
        <div class="w-1.5 h-1.5 bg-info rounded-full mr-3 flex-shrink-0" />
        <div>
          <div class="text-sm font-medium leading-relaxed">
            WebSocket客户端
          </div>
          <div class="text-xs opacity-60">
            启用后可连接到远程"寸止"服务器接收弹窗请求
          </div>
        </div>
      </div>
      <n-switch
        :value="localConfig.enabled"
        size="small"
        @update:value="toggleEnabled"
      />
    </div>

    <!-- WebSocket配置 -->
    <div v-if="localConfig.enabled" class="space-y-4 pl-6 border-l-2 border-gray-200 dark:border-gray-700">
      <!-- 服务器地址 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center">
          <div class="w-1.5 h-1.5 bg-primary-500 rounded-full mr-3 flex-shrink-0" />
          <div>
            <div class="text-sm font-medium leading-relaxed">
              服务器地址
            </div>
            <div class="text-xs opacity-60">
              "寸止"服务器的IP地址或域名
            </div>
          </div>
        </div>
        <n-input
          :value="localConfig.host"
          size="small"
          placeholder="127.0.0.1"
          style="width: 150px"
          @update:value="updateHost"
        />
      </div>

      <!-- 服务器端口 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center">
          <div class="w-1.5 h-1.5 bg-primary-500 rounded-full mr-3 flex-shrink-0" />
          <div>
            <div class="text-sm font-medium leading-relaxed">
              服务器端口
            </div>
            <div class="text-xs opacity-60">
              WebSocket服务器监听端口
            </div>
          </div>
        </div>
        <n-input-number
          :value="localConfig.port"
          size="small"
          :min="1"
          :max="65535"
          style="width: 100px"
          @update:value="updatePort"
        />
      </div>

      <!-- 自动连接 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center">
          <div class="w-1.5 h-1.5 bg-warning rounded-full mr-3 flex-shrink-0" />
          <div>
            <div class="text-sm font-medium leading-relaxed">
              启动时自动连接
            </div>
            <div class="text-xs opacity-60">
              应用启动时自动连接到WebSocket服务器
            </div>
          </div>
        </div>
        <n-switch
          :value="localConfig.auto_connect"
          size="small"
          @update:value="toggleAutoConnect"
        />
      </div>

      <!-- API Key 管理 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center">
          <div class="w-1.5 h-1.5 bg-error rounded-full mr-3 flex-shrink-0" />
          <div>
            <div class="text-sm font-medium leading-relaxed">
              API 密钥
            </div>
            <div class="text-xs opacity-60">
              用于WebSocket连接认证的安全密钥
            </div>
          </div>
        </div>
        <n-space>
          <n-input
            :value="maskedApiKey"
            size="small"
            readonly
            :type="showApiKey ? 'text' : 'password'"
            placeholder="未设置"
            style="width: 200px"
          />
          <n-button
            size="small"
            type="default"
            :disabled="!localConfig.api_key"
            @click="toggleApiKeyVisibility"
          >
            <template #icon>
              <div :class="showApiKey ? 'i-carbon-view-off' : 'i-carbon-view'" />
            </template>
          </n-button>
          <n-button
            size="small"
            type="default"
            :disabled="!localConfig.api_key"
            @click="copyApiKey"
          >
            <template #icon>
              <div class="i-carbon-copy" />
            </template>
          </n-button>
          <n-button
            size="small"
            type="primary"
            :loading="isGeneratingKey"
            @click="generateApiKey"
          >
            {{ localConfig.api_key ? '重新生成' : '生成密钥' }}
          </n-button>
        </n-space>
      </div>

      <!-- 环境变量设置提示 -->
      <div v-if="localConfig.api_key" class="bg-yellow-50 dark:bg-yellow-900/20 border border-yellow-200 dark:border-yellow-800 rounded-lg p-3">
        <div class="flex items-start">
          <div class="i-carbon-warning text-yellow-600 dark:text-yellow-400 mt-0.5 mr-2 flex-shrink-0" />
          <div class="text-xs text-yellow-800 dark:text-yellow-200">
            <div class="font-medium mb-1">
              服务端配置说明：
            </div>
            <div class="mb-1">
              请在服务端设置环境变量：
            </div>
            <code class="bg-yellow-100 dark:bg-yellow-800 px-2 py-1 rounded text-xs">
              CUNZHI_WS_API_KEY={{ localConfig.api_key }}
            </code>
            <div class="mt-1">
              然后重启"寸止"服务端使配置生效
            </div>
          </div>
        </div>
      </div>

      <!-- 连接状态和操作 -->
      <div class="flex items-center justify-between">
        <div class="flex items-center">
          <div class="w-1.5 h-1.5 bg-success rounded-full mr-3 flex-shrink-0" />
          <div>
            <div class="text-sm font-medium leading-relaxed">
              连接状态
            </div>
            <div class="text-xs opacity-60">
              <n-tag :type="statusColor" size="small">
                {{ statusText }}
              </n-tag>
              <span v-if="connectionError" class="ml-2 text-red-500">
                {{ connectionError }}
              </span>
            </div>
          </div>
        </div>
        <n-space>
          <n-button
            v-if="connectionStatus !== 'connected'"
            size="small"
            type="primary"
            :loading="isConnecting"
            @click="connectWebSocket"
          >
            连接
          </n-button>
          <n-button
            v-else
            size="small"
            type="default"
            @click="disconnectWebSocket"
          >
            断开
          </n-button>
          <n-button
            size="small"
            type="default"
            @click="checkConnectionStatus"
          >
            刷新状态
          </n-button>
        </n-space>
      </div>
    </div>
  </n-space>
</template>
