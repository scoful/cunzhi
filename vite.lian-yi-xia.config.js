import process from 'node:process'
import vue from '@vitejs/plugin-vue'
import UnoCSS from 'unocss/vite'
import { defineConfig } from 'vite'

export default defineConfig({
  plugins: [
    vue(),
    UnoCSS(),
  ],
  clearScreen: false,
  // Tauri应用需要使用相对路径
  base: './',
  // 设置根目录，使开发服务器能找到正确的HTML入口
  root: '.',
  server: {
    port: 5178, // 使用不同的端口避免与主应用冲突
    strictPort: true,
    host: '0.0.0.0',
    hmr: {
      port: 5179,
    },
  },
  envPrefix: ['VITE_', 'TAURI_'],
  build: {
    target: process.env.TAURI_PLATFORM === 'windows' ? 'chrome105' : 'safari13',
    minify: !process.env.TAURI_DEBUG ? 'esbuild' : false,
    sourcemap: !!process.env.TAURI_DEBUG,
    chunkSizeWarningLimit: 1500,
    outDir: 'dist-lian-yi-xia', // 独立的构建输出目录
    rollupOptions: {
      input: 'lian-yi-xia.html', // 使用独立的HTML入口
      output: {
        manualChunks: {
          vendor: ['vue', '@vueuse/core'],
        },
      },
    },
  },
})
