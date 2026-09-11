import type { DetectionConfig } from '@/api'

import { defineStore } from 'pinia'
import { computed, ref } from 'vue'

import { getDetectionConfig } from '@/api'

export const useSettingsStore = defineStore('settings', () => {
  const detectionConfig = ref<DetectionConfig | null>(null)
  const detectionLoading = ref(false)

  const detectionEnabled = computed(() => detectionConfig.value?.enabled ?? false)

  async function loadDetectionConfig() {
    if (detectionLoading.value)
      return
    detectionLoading.value = true
    try {
      detectionConfig.value = await getDetectionConfig()
    }
    catch {
      // 配置接口不可用时不影响主导航，其余设置页仍可正常使用。
    }
    finally {
      detectionLoading.value = false
    }
  }

  function applyDetectionConfig(config: DetectionConfig) {
    detectionConfig.value = config
  }

  return {
    detectionConfig,
    detectionLoading,
    detectionEnabled,
    loadDetectionConfig,
    applyDetectionConfig,
  }
})
