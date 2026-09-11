<script setup lang="ts">
import type { DetectionRecord, DetectionRound } from '@/api'
import { ChevronDown, Eye, PauseCircle, PlayCircle, RefreshCw } from '@lucide/vue'
import { onMounted, ref, shallowReactive } from 'vue'

import { getDetectionRecordHtmlUrl, getDetectionRecords, getDetectionRounds, setSchedulingSuspended } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseTable from '@/components/base/BaseTable/index.vue'
import { toast } from '@/components/base/BaseToast'
import { errorMessage } from '@/utils/async'
import { formatDateTime } from '@/utils/date'
import AccountIdentityCell from '@/views/accounts/components/AccountIdentityCell.vue'

const rounds = ref<DetectionRound[]>([])
const roundsLoading = ref(true)
const roundsError = ref('')
const openRounds = ref(new Set<string>())
const recordsByRound = shallowReactive<Record<string, DetectionRecord[]>>({})
const recordsLoading = shallowReactive<Record<string, boolean>>({})
const actionBusyKey = ref<string | null>(null)
const htmlModalVisible = ref(false)
const currentRecordId = ref<number | null>(null)

const recordColumns = [
  { key: 'account', label: '账号', kind: 'identity' as const, size: '2xl' as const },
  { key: 'checked_at', label: '检测时间', kind: 'datetime' as const, size: 'xl' as const },
  { key: 'degraded', label: '结论', kind: 'status' as const, size: 'md' as const, align: 'center' as const },
  { key: 'scheduling_suspended', label: '调度状态', kind: 'status' as const, size: 'md' as const, align: 'center' as const },
  { key: 'actions', label: '操作', kind: 'actions' as const, size: '2xl' as const },
]

async function loadRounds() {
  roundsLoading.value = true
  roundsError.value = ''
  try {
    rounds.value = await getDetectionRounds()
    const latest = rounds.value[0]
    if (latest) {
      openRounds.value = new Set([latest.detection_round_id])
      await loadRoundRecords(latest)
    }
  }
  catch (error) {
    roundsError.value = errorMessage(error, '检测记录加载失败')
  }
  finally {
    roundsLoading.value = false
  }
}

async function loadRoundRecords(round: DetectionRound, force = false) {
  const roundId = round.detection_round_id
  if (!force && recordsByRound[roundId])
    return
  if (recordsLoading[roundId])
    return

  recordsLoading[roundId] = true
  try {
    recordsByRound[roundId] = await getDetectionRecords(roundId)
  }
  catch (error) {
    toast.error(errorMessage(error, '批次明细加载失败'))
  }
  finally {
    recordsLoading[roundId] = false
  }
}

function toggleRound(round: DetectionRound, event: Event) {
  const details = event.currentTarget as HTMLDetailsElement
  const next = new Set(openRounds.value)
  if (details.open) {
    next.add(round.detection_round_id)
    void loadRoundRecords(round)
  }
  else {
    next.delete(round.detection_round_id)
  }
  openRounds.value = next
}

function accountIdentity(record: DetectionRecord) {
  return {
    id: record.account_id,
    accountId: record.account_id,
    email: record.account_email ?? record.account_name ?? record.account_id,
    planType: record.account_plan_type ?? null,
    planTypeDisplay: record.account_plan_type_display ?? '',
  }
}

async function toggleScheduling(round: DetectionRound, record: DetectionRecord) {
  const suspended = !record.scheduling_suspended
  actionBusyKey.value = `${round.detection_round_id}:${record.id}`
  try {
    await setSchedulingSuspended({ accountId: record.account_id, suspended })
    await loadRoundRecords(round, true)
  }
  catch (error) {
    toast.error(errorMessage(error, suspended ? '暂停调度失败' : '恢复调度失败'))
  }
  finally {
    actionBusyKey.value = null
  }
}

function viewHtml(record: DetectionRecord) {
  currentRecordId.value = record.id
  htmlModalVisible.value = true
}

onMounted(() => {
  void loadRounds()
})
</script>

<template>
  <div class="w-full">
    <BasePageHeader title="检测记录" description="查看每次降智检测的结果与账号调度状态">
      <template #actions>
        <BaseButton variant="secondary" size="sm" :loading="roundsLoading" aria-label="刷新检测记录" @click="loadRounds">
          <template #icon>
            <RefreshCw class="size-3.5" />
          </template>
          刷新
        </BaseButton>
      </template>
    </BasePageHeader>

    <BaseCard class="mt-5" padding="none">
      <div v-if="roundsLoading && rounds.length === 0" class="grid min-h-48 place-items-center p-6 text-cp-text-secondary">
        <div class="inline-flex items-center gap-2 text-cp font-semibold">
          <RefreshCw class="size-4 animate-spin motion-reduce:animate-none" />
          正在加载检测批次…
        </div>
      </div>

      <BaseEmpty
        v-else-if="roundsError"
        class="m-5"
        title="检测记录暂时不可用"
        :description="roundsError"
        surface="inset"
      >
        <template #action>
          <BaseButton size="sm" @click="loadRounds">
            重试
          </BaseButton>
        </template>
      </BaseEmpty>

      <BaseEmpty
        v-else-if="rounds.length === 0"
        class="m-5"
        title="暂无检测记录"
        description="开启降智检测后，完成的检测批次会显示在这里。"
        surface="inset"
      />

      <div v-else class="divide-y divide-cp-border-secondary">
        <details
          v-for="(round, index) in rounds"
          :key="round.detection_round_id"
          class="group"
          :open="openRounds.has(round.detection_round_id)"
          @toggle="toggleRound(round, $event)"
        >
          <summary class="flex cursor-pointer list-none items-center gap-3 bg-cp-bg-elevated px-5 py-4 outline-none transition-colors hover:bg-cp-fill-quaternary focus-visible:ring-2 focus-visible:ring-inset focus-visible:ring-cp-control-outline [&::-webkit-details-marker]:hidden">
            <ChevronDown class="size-4 shrink-0 text-cp-text-secondary transition-transform duration-200 motion-reduce:transition-none group-open:rotate-180" />
            <span class="min-w-0 flex-1 text-cp font-heavy text-cp-text">
              {{ formatDateTime(round.checked_at) }}
              <span v-if="index === 0" class="ml-2 text-cp-xs font-bold text-cp-text-tertiary">最新</span>
            </span>
            <span class="inline-flex shrink-0 items-center gap-1 rounded-cp-sm bg-cp-error-container px-2 py-1 text-cp-xs font-bold text-cp-error-on-container">
              降智 {{ round.degraded_count }} 个
            </span>
            <span class="inline-flex shrink-0 items-center gap-1 rounded-cp-sm bg-cp-success-container px-2 py-1 text-cp-xs font-bold text-cp-success-on-container">
              正常 {{ round.normal_count }} 个
            </span>
          </summary>

          <div class="bg-cp-bg-container p-4 sm:p-5">
            <div v-if="recordsLoading[round.detection_round_id] && !recordsByRound[round.detection_round_id]" class="grid min-h-32 place-items-center text-cp-text-secondary">
              <span class="inline-flex items-center gap-2 text-cp-sm font-semibold">
                <RefreshCw class="size-4 animate-spin motion-reduce:animate-none" />
                正在加载账号明细…
              </span>
            </div>
            <BaseEmpty
              v-else-if="recordsByRound[round.detection_round_id]?.length === 0"
              size="sm"
              title="该批次暂无账号明细"
              surface="none"
            />
            <BaseTable
              v-else
              :columns="recordColumns"
              :rows="recordsByRound[round.detection_round_id] ?? []"
              :loading="recordsLoading[round.detection_round_id]"
              row-key="id"
              density="compact"
              empty-text="暂无账号明细"
            >
              <template #account="{ row }">
                <AccountIdentityCell :account="accountIdentity(row)" size="md" title-mode="email" />
              </template>
              <template #checked_at="{ row }">
                <span class="whitespace-nowrap font-mono text-cp-sm text-cp-text-secondary">{{ formatDateTime(row.checked_at) }}</span>
              </template>
              <template #degraded="{ row }">
                <span
                  class="inline-flex rounded-cp-sm px-2 py-1 text-cp-xs font-bold"
                  :class="row.degraded ? 'bg-cp-error-container text-cp-error-on-container' : 'bg-cp-success-container text-cp-success-on-container'"
                >
                  {{ row.degraded ? '降智' : '不降智' }}
                </span>
              </template>
              <template #scheduling_suspended="{ row }">
                <span :class="row.scheduling_suspended ? 'text-cp-warning' : 'text-cp-success'" class="font-semibold">
                  {{ row.scheduling_suspended ? '已暂停' : '正常' }}
                </span>
              </template>
              <template #actions="{ row }">
                <div class="flex flex-wrap items-center gap-1.5">
                  <BaseButton
                    size="sm"
                    variant="ghost"
                    :loading="actionBusyKey === `${round.detection_round_id}:${row.id}`"
                    :aria-label="row.scheduling_suspended ? '恢复调度' : '暂停调度'"
                    @click="toggleScheduling(round, row)"
                  >
                    <template #icon>
                      <PlayCircle v-if="row.scheduling_suspended" class="size-3.5" />
                      <PauseCircle v-else class="size-3.5" />
                    </template>
                    {{ row.scheduling_suspended ? '恢复调度' : '暂停调度' }}
                  </BaseButton>
                  <BaseButton size="sm" variant="ghost" aria-label="查看检测响应效果" @click="viewHtml(row)">
                    <template #icon>
                      <Eye class="size-3.5" />
                    </template>
                    查看效果
                  </BaseButton>
                </div>
              </template>
            </BaseTable>
          </div>
        </details>
      </div>
    </BaseCard>

    <BaseModal v-model="htmlModalVisible" title="检测响应效果" size="lg">
      <iframe
        v-if="currentRecordId !== null"
        :src="getDetectionRecordHtmlUrl(currentRecordId)"
        sandbox="allow-scripts"
        title="检测响应效果"
        class="h-[520px] w-full rounded-cp border border-cp-border-secondary"
      />
    </BaseModal>
  </div>
</template>
