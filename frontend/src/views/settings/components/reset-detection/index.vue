<script setup lang="ts">
import { watch } from 'vue'

import { useResetDetectionSettings } from '../../composables/useResetDetectionSettings'
import ResetDetectionSettingsCard from './ResetDetectionSettingsCard.vue'

const props = defineProps<{
  active: boolean
}>()

const {
  loading,
  saving,
  error,
  form,
  load,
  save,
} = useResetDetectionSettings()

watch(
  () => props.active,
  (active) => {
    if (active)
      void load()
  },
  { immediate: true },
)

defineExpose({ load })
</script>

<template>
  <div class="grid w-full gap-5">
    <ResetDetectionSettingsCard
      v-model:enabled="form.enabled"
      v-model:interval="form.interval"
      v-model:interval-unit="form.intervalUnit"
      v-model:account-scope="form.accountScope"
      v-model:auto-use-reset-card="form.autoUseResetCard"
      :loading="loading"
      :saving="saving"
      :error="error"
      @save="save"
    />
  </div>
</template>
