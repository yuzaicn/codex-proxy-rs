import { defineStore } from 'pinia'
import { computed, ref } from 'vue'
import { getDetectionConfig } from '@/api'

export const useSettingsStore = defineStore('settings', () => {
  const detectionConfigEnabled = ref(false)
  const detectionEnabled = computed(() => detectionConfigEnabled.value)

  async function loadDetectionConfig() {
    const config = await getDetectionConfig()
    detectionConfigEnabled.value = config.enabled
    return config
  }

  function setDetectionEnabled(enabled: boolean) {
    detectionConfigEnabled.value = enabled
  }

  return {
    detectionEnabled,
    loadDetectionConfig,
    setDetectionEnabled,
  }
})
