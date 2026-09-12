<script setup lang="ts">
import type { DetectionRecord, DetectionRound } from '@/api'
import { ChevronDown, Eye, PauseCircle, PlayCircle, RefreshCw } from '@lucide/vue'
import { computed, onBeforeUnmount, onMounted, reactive, ref, shallowReactive } from 'vue'

import { getDetectionRecordHtmlUrl, getDetectionRecords, getDetectionRounds, setSchedulingSuspended } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseEmpty from '@/components/base/BaseEmpty.vue'
import BaseModal from '@/components/base/BaseModal/index.vue'
import BasePageHeader from '@/components/base/BasePageHeader.vue'
import BaseScrollbar from '@/components/base/BaseScrollbar.vue'
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
const detailModalVisible = ref(false)
const currentRecord = ref<DetectionRecord | null>(null)

type PreviewStatus = 'idle' | 'loading' | 'rendering' | 'ready' | 'missing' | 'error' | 'timeout'

interface PreviewState {
  status: PreviewStatus
  srcdoc?: string
  controller?: AbortController
  renderTimeout?: number
}

const previewStates = reactive<Record<number, PreviewState>>({})
const previewElements = new Map<number, HTMLElement>()
let previewObserver: IntersectionObserver | null = null

const PREVIEW_FETCH_TIMEOUT_MS = 8_000
const PREVIEW_RENDER_TIMEOUT_MS = 4_000

// 空白正文与旧记录的 null 同样按「未采集」展示，避免弹出空 <pre>。
const currentReasoning = computed(() => {
  const reasoning = currentRecord.value?.reasoning_content
  return reasoning?.trim() ? reasoning : null
})

// 旧记录没有提示词快照，直接隐藏该区块，不做占位。
const currentPrompt = computed(() => {
  const prompt = currentRecord.value?.prompt_used
  return prompt?.trim() ? prompt : null
})

const recordColumns = [
  { key: 'account', label: '账号', kind: 'identity' as const, size: '2xl' as const },
  { key: 'preview', label: '响应预览', kind: 'custom' as const, size: '3xl' as const },
  { key: 'checked_at', label: '检测时间', kind: 'datetime' as const, size: 'xl' as const },
  { key: 'degraded', label: '结论', kind: 'status' as const, size: 'md' as const, align: 'center' as const },
  { key: 'scheduling_suspended', label: '调度状态', kind: 'status' as const, size: 'md' as const, align: 'center' as const },
  { key: 'actions', label: '操作', kind: 'actions' as const, size: '2xl' as const },
]

function getPreviewState(recordId: number): PreviewState {
  return previewStates[recordId] ?? (previewStates[recordId] = { status: 'idle' })
}

function previewStatusLabel(status: PreviewStatus) {
  switch (status) {
    case 'loading':
      return '正在加载预览…'
    case 'rendering':
      return '正在渲染预览…'
    case 'missing':
      return '该记录没有 HTML 效果'
    case 'error':
      return '预览加载失败'
    case 'timeout':
      return '预览渲染超时'
    default:
      return '响应效果预览'
  }
}

function previewDocument(html: string) {
  const cspMeta = '<meta http-equiv="Content-Security-Policy" content="sandbox allow-scripts">'
  if (/<head\b[^>]*>/i.test(html))
    return html.replace(/<head\b[^>]*>/i, match => `${match}${cspMeta}`)
  return `<!doctype html><html><head>${cspMeta}</head><body>${html}</body></html>`
}

function clearPreviewTimeout(state: PreviewState) {
  if (state.renderTimeout !== undefined) {
    window.clearTimeout(state.renderTimeout)
    state.renderTimeout = undefined
  }
}

function cancelPreviewLoad(recordId: number) {
  const state = getPreviewState(recordId)
  if (state.status !== 'loading' && state.status !== 'rendering')
    return
  clearPreviewTimeout(state)
  state.controller?.abort()
  state.controller = undefined
  state.srcdoc = undefined
  state.status = 'idle'
}

function cancelRoundPreviewLoads(roundId: string) {
  for (const record of recordsByRound[roundId] ?? [])
    cancelPreviewLoad(record.id)
}

async function loadPreview(record: DetectionRecord) {
  const state = getPreviewState(record.id)
  if (state.status !== 'idle')
    return

  const controller = new AbortController()
  state.controller = controller
  state.status = 'loading'
  const fetchTimeout = window.setTimeout(() => {
    if (state.status === 'loading') {
      state.status = 'timeout'
      controller.abort()
    }
  }, PREVIEW_FETCH_TIMEOUT_MS)

  try {
    const response = await fetch(getDetectionRecordHtmlUrl(record.id), {
      credentials: 'same-origin',
      signal: controller.signal,
    })
    if (response.status === 404 || response.status === 204) {
      state.status = 'missing'
      return
    }
    if (!response.ok) {
      state.status = 'error'
      return
    }

    const html = await response.text()
    if (!html.trim()) {
      state.status = 'missing'
      return
    }

    state.srcdoc = previewDocument(html)
    state.status = 'rendering'
    state.renderTimeout = window.setTimeout(() => {
      if (state.status === 'rendering') {
        state.status = 'timeout'
        controller.abort()
      }
    }, PREVIEW_RENDER_TIMEOUT_MS)
  }
  catch {
    if (!controller.signal.aborted)
      state.status = 'error'
  }
  finally {
    window.clearTimeout(fetchTimeout)
    if (state.controller === controller)
      state.controller = undefined
  }
}

function onPreviewFrameLoad(recordId: number) {
  const state = getPreviewState(recordId)
  if (state.status !== 'rendering')
    return
  clearPreviewTimeout(state)
  state.status = 'ready'
}

function onPreviewFrameError(recordId: number) {
  const state = getPreviewState(recordId)
  if (state.status !== 'rendering')
    return
  clearPreviewTimeout(state)
  state.status = 'error'
}

function handlePreviewKeydown(record: DetectionRecord, event: KeyboardEvent) {
  if (event.key !== 'Enter' && event.key !== ' ')
    return
  event.preventDefault()
  viewDetail(record)
}

function registerPreviewElement(element: unknown, record: DetectionRecord, roundId: string) {
  const previous = previewElements.get(record.id)
  if (previous && previous !== element)
    previewObserver?.unobserve(previous)
  if (!(element instanceof HTMLElement)) {
    previewElements.delete(record.id)
    return
  }
  previewElements.set(record.id, element)
  element.dataset.previewRecordId = String(record.id)
  element.dataset.previewRoundId = roundId
  previewObserver?.observe(element)
}

function createPreviewObserver() {
  if (typeof window === 'undefined' || !('IntersectionObserver' in window))
    return
  previewObserver = new IntersectionObserver((entries) => {
    for (const entry of entries) {
      const element = entry.target as HTMLElement
      const recordId = Number(element.dataset.previewRecordId)
      const roundId = element.dataset.previewRoundId
      const record = (roundId ? recordsByRound[roundId] : undefined)?.find(item => item.id === recordId)
      if (!record || !roundId)
        continue
      if (entry.isIntersecting && openRounds.value.has(roundId))
        void loadPreview(record)
      else if (!entry.isIntersecting)
        cancelPreviewLoad(recordId)
    }
  }, { rootMargin: '0px' })
  for (const element of previewElements.values())
    previewObserver.observe(element)
}

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
    cancelRoundPreviewLoads(round.detection_round_id)
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

function viewDetail(record: DetectionRecord) {
  currentRecord.value = record
  detailModalVisible.value = true
}

onMounted(() => {
  createPreviewObserver()
  void loadRounds()
})

onBeforeUnmount(() => {
  for (const recordId of Object.keys(previewStates))
    cancelPreviewLoad(Number(recordId))
  previewObserver?.disconnect()
  previewObserver = null
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
              <template #preview="{ row }">
                <div
                  class="flex min-w-0 items-center gap-2"
                  :aria-label="`${previewStatusLabel(getPreviewState(row.id).status)}，点击查看详情`"
                >
                  <div
                    class="relative h-[150px] w-[240px] shrink-0 cursor-pointer overflow-hidden rounded-cp border border-cp-border-secondary bg-cp-bg-elevated outline-none transition-[border-color,box-shadow] hover:border-cp-primary-border focus-visible:border-cp-control-outline focus-visible:ring-2 focus-visible:ring-cp-control-outline motion-reduce:transition-none"
                    role="button"
                    :ref="element => registerPreviewElement(element, row, round.detection_round_id)"
                    tabindex="0"
                    @click="viewDetail(row)"
                    @keydown="handlePreviewKeydown(row, $event)"
                  >
                    <iframe
                      v-if="getPreviewState(row.id).status === 'rendering' || getPreviewState(row.id).status === 'ready'"
                      :srcdoc="getPreviewState(row.id).srcdoc"
                      sandbox="allow-scripts"
                      title="检测响应缩略预览"
                      aria-hidden="true"
                      class="pointer-events-none absolute left-0 top-0 h-[600px] w-[960px] origin-top-left scale-25 border-0"
                      @load="onPreviewFrameLoad(row.id)"
                      @error="onPreviewFrameError(row.id)"
                    />
                    <div
                      v-if="getPreviewState(row.id).status !== 'rendering' && getPreviewState(row.id).status !== 'ready'"
                      class="absolute inset-0 grid place-items-center p-3 text-center text-cp-xs font-emphasis text-cp-text-secondary"
                    >
                      <span>{{ previewStatusLabel(getPreviewState(row.id).status) }}</span>
                    </div>
                    <span
                      class="absolute bottom-2 left-2 rounded-cp-sm px-2 py-1 text-cp-xs font-bold shadow-[0_1px_3px_var(--cp-color-shadow)]"
                      :class="row.degraded ? 'bg-cp-error-container text-cp-error-on-container' : 'bg-cp-success-container text-cp-success-on-container'"
                    >
                      {{ row.degraded ? '降智' : '不降智' }}
                    </span>
                  </div>
                  <span class="sr-only">{{ row.degraded ? '该账号判定为降智' : '该账号判定为不降智' }}</span>
                </div>
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
                  <BaseButton size="sm" variant="ghost" aria-label="查看检测记录详情" @click="viewDetail(row)">
                    <template #icon>
                      <Eye class="size-3.5" />
                    </template>
                    查看详情
                  </BaseButton>
                </div>
              </template>
            </BaseTable>
          </div>
        </details>
      </div>
    </BaseCard>

    <BaseModal v-model="detailModalVisible" title="检测记录详情" size="lg">
      <div v-if="currentRecord" class="flex flex-col gap-3">
        <div v-if="currentPrompt" class="rounded-lg bg-cp-bg-container px-3 py-2.5">
          <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
            探测提示词
          </p>
          <pre
            class="mt-2 mb-0 whitespace-pre-wrap wrap-break-word font-mono text-cp-sm leading-[1.65] text-cp-text"
            v-text="currentPrompt"
          />
        </div>
        <div class="rounded-lg bg-cp-bg-container px-3 py-2.5">
          <p class="m-0 text-cp-xs font-heavy text-cp-text-quaternary">
            思考过程
          </p>
          <BaseScrollbar v-if="currentReasoning" max-height="240px">
            <div class="pt-2">
              <pre
                class="m-0 whitespace-pre-wrap wrap-break-word font-mono text-cp-sm leading-[1.65] text-cp-text"
                v-text="currentReasoning"
              />
            </div>
          </BaseScrollbar>
          <p v-else class="mt-2 mb-0 text-cp-sm font-emphasis text-cp-text-quaternary">
            该记录未采集思考过程
          </p>
        </div>
        <div>
          <p class="m-0 mb-2 text-cp-xs font-heavy text-cp-text-quaternary">
            响应效果
          </p>
          <iframe
            :src="getDetectionRecordHtmlUrl(currentRecord.id)"
            sandbox="allow-scripts"
            title="检测响应效果"
            class="h-[520px] w-full rounded-cp border border-cp-border-secondary"
          />
        </div>
      </div>
    </BaseModal>
  </div>
</template>
