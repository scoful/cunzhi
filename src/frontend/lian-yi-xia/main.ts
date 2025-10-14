import {
  create,
  NButton,
  NCard,
  NConfigProvider,
  NDialogProvider,
  NForm,
  NFormItem,
  NGrid,
  NGridItem,
  NInput,
  NInputGroup,
  NInputNumber,
  NMessageProvider,
  NModal,
  NNotificationProvider,
  NSpace,
  NSwitch,
  NTab,
  NTabPane,
  NTabs,
  NTag,
} from 'naive-ui'
import { createApp } from 'vue'
import App from './App.vue'
import 'virtual:uno.css'
import '../assets/styles/style.css'

const naive = create({
  components: [
    NButton,
    NCard,
    NConfigProvider,
    NDialogProvider,
    NForm,
    NFormItem,
    NGrid,
    NGridItem,
    NInput,
    NInputGroup,
    NInputNumber,
    NMessageProvider,
    NModal,
    NNotificationProvider,
    NSpace,
    NSwitch,
    NTab,
    NTabPane,
    NTabs,
    NTag,
  ],
})

const app = createApp(App)
app.use(naive)
app.mount('#app')
