<script setup lang="ts">
import LastUsedAtCell from '@/components/LastUsedAtCell.vue'
import { accountTimelineLabels } from '../constants'

defineProps<{
  lastUsedAt: string | null
  addedAtDisplay: string
  updatedAtDisplay: string
}>()
</script>

<template>
  <dl class="grid min-w-0 grid-cols-[2.5rem_minmax(0,1fr)] gap-x-1 text-[10px] leading-3 tabular-nums">
    <dt class="font-sans font-emphasis text-cp-text-quaternary">
      {{ accountTimelineLabels[0] }}
    </dt>
    <dd class="m-0 min-w-0 whitespace-normal text-cp-text-secondary">
      <LastUsedAtCell :value="lastUsedAt" />
    </dd>
    <dt class="font-sans font-emphasis text-cp-text-quaternary">
      {{ accountTimelineLabels[1] }}
    </dt>
    <dd class="m-0 min-w-0 whitespace-normal text-cp-text-secondary" :title="addedAtDisplay">
      {{ addedAtDisplay }}
    </dd>
    <dt class="font-sans font-emphasis text-cp-text-quaternary">
      {{ accountTimelineLabels[2] }}
    </dt>
    <dd class="m-0 min-w-0 whitespace-normal text-cp-text-secondary" :title="updatedAtDisplay">
      {{ updatedAtDisplay }}
    </dd>
  </dl>
</template>
