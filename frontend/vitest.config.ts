import { fileURLToPath } from 'node:url'
import { defineConfig } from 'vitest/config'

// 组合式函数与工具层的单测配置；不加载 Vue 插件，测试中不得引入 SFC。
export default defineConfig({
  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
    },
  },
  test: {
    environment: 'node',
    include: ['src/**/__tests__/*.spec.ts'],
  },
})
