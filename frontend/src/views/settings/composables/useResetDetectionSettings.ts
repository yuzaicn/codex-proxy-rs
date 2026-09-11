import type { ResetDetectionAccountScope, ResetDetectionSettings } from '@/api'

import { computed, reactive, shallowRef } from 'vue'

import { getResetDetectionSettings, updateResetDetectionSettings } from '@/api'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'

export type ResetDetectionIntervalUnit = 'seconds' | 'hours'

const MIN_INTERVAL_SECONDS = 30

export function useResetDetectionSettings() {
  const loading = shallowRef(false)
  const saving = shallowRef(false)
  const error = shallowRef('')
  const loaded = shallowRef(false)

  const form = reactive({
    enabled: false,
    interval: '3600',
    intervalUnit: 'seconds' as ResetDetectionIntervalUnit,
    accountScope: 'all' as ResetDetectionAccountScope,
    autoUseResetCard: false,
  })

  const intervalSeconds = computed(() => {
    const value = Number(form.interval)
    if (!Number.isFinite(value))
      return null
    return form.intervalUnit === 'hours' ? value * 3600 : value
  })

  function applySettings(settings: ResetDetectionSettings): void {
    const seconds = Number(settings.pollIntervalSecs)
    const hasValidInterval = Number.isFinite(seconds) && seconds > 0
    const showHours = hasValidInterval && seconds % 3600 === 0

    form.enabled = Boolean(settings.enabled)
    form.intervalUnit = showHours ? 'hours' : 'seconds'
    form.interval = hasValidInterval ? String(showHours ? seconds / 3600 : seconds) : '3600'
    form.accountScope = settings.accountScope
    form.autoUseResetCard = Boolean(settings.autoConsumeEnabled)
  }

  async function load(): Promise<void> {
    if (loading.value || loaded.value)
      return

    loading.value = true
    error.value = ''
    try {
      applySettings(await getResetDetectionSettings())
      loaded.value = true
    }
    catch (cause: unknown) {
      error.value = errorMessage(cause, '重置检测配置加载失败')
      toast.error(error.value)
    }
    finally {
      loading.value = false
    }
  }

  function buildPayload(): Omit<ResetDetectionSettings, 'updatedAt'> | null {
    const value = Number(form.interval)
    const seconds = intervalSeconds.value
    if (!Number.isInteger(value) || value <= 0 || seconds === null || !Number.isSafeInteger(seconds)) {
      toast.warning('轮询周期需为正整数')
      return null
    }
    if (seconds < MIN_INTERVAL_SECONDS) {
      toast.warning(`轮询周期不得少于 ${MIN_INTERVAL_SECONDS} 秒`)
      return null
    }

    return {
      enabled: form.enabled,
      pollIntervalSecs: seconds,
      accountScope: form.accountScope,
      autoConsumeEnabled: form.autoUseResetCard,
    }
  }

  async function save(): Promise<boolean> {
    if (saving.value || loading.value)
      return false

    const payload = buildPayload()
    if (!payload)
      return false

    saving.value = true
    try {
      applySettings(await updateResetDetectionSettings(payload))
      loaded.value = true
      toast.success('重置检测配置已保存')
      return true
    }
    catch (cause: unknown) {
      error.value = errorMessage(cause, '重置检测配置保存失败')
      toast.error(error.value)
      loaded.value = false
      await load()
      return false
    }
    finally {
      saving.value = false
    }
  }

  return {
    loading,
    saving,
    error,
    form,
    load,
    save,
  }
}
