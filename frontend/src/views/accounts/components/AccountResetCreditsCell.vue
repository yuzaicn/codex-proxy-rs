<script setup lang="ts">
import type { AccountRow } from '../constants'
import { computed } from 'vue'

import { useUiClock } from '@/composables/useUiClock'
import { formatDateTime, formatRelativeTime } from '@/utils/date'

const props = defineProps<{
  account: AccountRow
}>()

// 只有 openai 账号具备重置卡能力；其余平台展示占位，避免被误读成 0 张。
const supported = computed(() => props.account.provider === 'openai')
const observation = computed(() => props.account.resetCredits ?? null)

const now = useUiClock()
const freshnessText = computed(() =>
  observation.value ? formatRelativeTime(observation.value.observedAt, now.value) : '',
)
const observedAtTitle = computed(() =>
  observation.value ? `${formatDateTime(observation.value.observedAt)} 观测` : undefined,
)
</script>

<template>
  <span v-if="!supported" class="text-cp-text-quaternary" title="该平台不支持主动重置卡">
    —
  </span>
  <span
    v-else-if="!observation"
    class="text-cp-xs font-emphasis text-cp-text-quaternary"
    title="后台尚未观测过该账号的重置卡数量"
  >
    待观测
  </span>
  <div v-else class="grid justify-items-center gap-0.5" :title="observedAtTitle">
    <span
      class="font-mono text-cp leading-none font-heavy tabular-nums"
      :class="observation.availableCount > 0 ? 'text-cp-text' : 'text-cp-text-secondary'"
    >
      {{ observation.availableCount }}
    </span>
    <span class="font-mono text-[10px] leading-none tabular-nums text-cp-text-quaternary">
      {{ freshnessText }}
    </span>
  </div>
</template>
