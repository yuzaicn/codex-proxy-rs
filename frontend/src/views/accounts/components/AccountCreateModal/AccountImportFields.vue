<script setup lang="ts">
import type { TokenImportKind, TokenImportRow } from '../../composables/useAccountOnboarding'
import { Copy, RefreshCw, Trash2, Upload } from '@lucide/vue'
import { useFileDialog } from '@vueuse/core'
import { onScopeDispose, ref } from 'vue'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseTextarea from '@/components/base/BaseTextarea.vue'
import { maskToken } from '../../composables/useAccountOnboarding'

withDefaults(defineProps<{
  label: string
  placeholder: string
  uploadable: boolean
  disabled: boolean
  rows?: TokenImportRow[]
}>(), { rows: undefined })
const emit = defineEmits<{
  retryRow: [id: string]
  retryFailed: []
  copyFailed: []
  setRowKind: [payload: { id: string, kind: TokenImportKind }]
  removeRow: [id: string]
  setUnknownKind: [kind: TokenImportKind]
}>()
const text = defineModel<string>({ required: true })
const fileError = ref('')
const { open: openFile, onChange } = useFileDialog({ accept: 'application/json,.json', multiple: false, reset: true })

let readVersion = 0
onScopeDispose(() => {
  readVersion += 1
})

onChange(async (files) => {
  const file = files?.[0]
  if (!file)
    return
  const version = ++readVersion
  fileError.value = ''
  try {
    const contents = await file.text()
    if (version !== readVersion)
      return
    text.value = contents
  }
  catch {
    if (version === readVersion)
      fileError.value = '文件读取失败，请重新选择'
  }
})

function updateText(value: string) {
  text.value = value
  readVersion += 1
  fileError.value = ''
}

function updateKind(row: TokenImportRow, event: Event) {
  const value = (event.target as HTMLSelectElement).value as TokenImportKind
  emit('setRowKind', { id: row.id, kind: value })
}
</script>

<template>
  <BaseFormItem :label="label" required :error="fileError || undefined">
    <template v-if="uploadable" #extra>
      <BaseButton size="sm" :disabled="disabled" @click="openFile()">
        <template #icon>
          <Upload class="size-3.5" aria-hidden="true" />
        </template>
        上传文件
      </BaseButton>
    </template>
    <BaseTextarea
      :model-value="text"
      :aria-label="label"
      :rows="9"
      :placeholder="placeholder"
      :disabled="disabled"
      @update:model-value="updateText"
    />
    <template v-if="rows">
      <div class="mt-3 flex flex-wrap items-center justify-between gap-2 rounded-md border border-cp-border bg-cp-bg-muted px-3 py-2 text-xs text-cp-text-secondary">
        <span>待导入 {{ rows.filter(row => row.status === 'pending').length }} · 成功 {{ rows.filter(row => row.status === 'success').length }} · 失败 {{ rows.filter(row => row.status === 'failed').length }} · 还需要你处理 {{ rows.filter(row => row.status === 'needs_action' || row.kind === 'unknown' || row.status === 'duplicate').length }} 行</span>
        <div class="flex flex-wrap gap-2">
          <BaseButton v-if="rows.some(row => row.kind === 'unknown')" size="sm" variant="secondary" :disabled="disabled" @click="emit('setUnknownKind', 'rt')">
            未识别设为 RT
          </BaseButton>
          <BaseButton v-if="rows.some(row => row.kind === 'unknown')" size="sm" variant="secondary" :disabled="disabled" @click="emit('setUnknownKind', 'at')">
            未识别设为 AT
          </BaseButton>
          <BaseButton v-if="rows.some(row => row.status === 'failed' || row.status === 'needs_action')" size="sm" variant="secondary" :disabled="disabled" @click="emit('retryFailed')">
            <template #icon>
              <RefreshCw class="size-3.5" aria-hidden="true" />
            </template>
            重试失败项
          </BaseButton>
          <BaseButton v-if="rows.some(row => row.status === 'failed' || row.status === 'needs_action')" size="sm" variant="secondary" :disabled="disabled" @click="emit('copyFailed')">
            <template #icon>
              <Copy class="size-3.5" aria-hidden="true" />
            </template>
            复制失败清单
          </BaseButton>
        </div>
      </div>
      <div class="mt-2 max-h-72 overflow-auto rounded-md border border-cp-border">
        <table class="w-full min-w-[640px] text-left text-xs">
          <thead class="sticky top-0 bg-cp-bg-muted text-cp-text-secondary">
            <tr>
              <th class="px-3 py-2">
                行号
              </th><th class="px-3 py-2">
                类型
              </th><th class="px-3 py-2">
                凭据
              </th><th class="px-3 py-2">
                状态
              </th><th class="px-3 py-2">
                结果
              </th><th class="px-3 py-2 text-right">
                操作
              </th>
            </tr>
          </thead>
          <tbody>
            <tr v-for="row in rows" :key="row.id" class="border-t border-cp-border align-middle">
              <td class="px-3 py-2 text-cp-text-secondary">
                {{ row.index }}
              </td>
              <td class="px-3 py-2">
                <select :aria-label="`第 ${row.index} 行类型`" class="rounded border border-cp-border bg-cp-bg px-1.5 py-1" :value="row.kind" :disabled="disabled || row.status === 'success' || row.status === 'importing'" @change="updateKind(row, $event)">
                  <option value="at">
                    AT
                  </option><option value="rt">
                    RT
                  </option><option value="unknown">
                    未识别
                  </option><option value="unsupported">
                    不支持
                  </option>
                </select>
              </td>
              <td class="max-w-48 truncate px-3 py-2 font-mono text-cp-text" :title="row.credential ? maskToken(row.credential) : '无法提取凭据'">
                {{ row.credential ? maskToken(row.credential) : '无法提取凭据' }}
              </td>
              <td class="px-3 py-2">
                <span :class="row.status === 'success' ? 'text-cp-success' : row.status === 'failed' || row.status === 'needs_action' || row.kind === 'unknown' || row.status === 'duplicate' ? 'text-cp-error' : row.status === 'importing' ? 'text-cp-warning' : 'text-cp-text-secondary'">{{ row.status === 'pending' ? '待导入' : row.status === 'importing' ? (row.retryAttempt ? `导入中（第 ${row.retryAttempt} 次重试）` : '导入中') : row.status === 'success' ? '成功' : row.status === 'duplicate' ? '重复' : row.status === 'needs_action' ? '需要处理' : '失败' }}</span>
              </td>
              <td class="max-w-64 truncate px-3 py-2 text-cp-text-secondary">
                {{ row.error || row.result || (row.kind === 'unknown' ? '无法识别' : '') }}
              </td>
              <td class="px-3 py-2 text-right">
                <div class="inline-flex gap-1">
                  <BaseButton v-if="row.status === 'failed' || row.status === 'needs_action'" size="sm" variant="ghost" :disabled="disabled" title="重试该行" @click="emit('retryRow', row.id)">
                    <RefreshCw class="size-3.5" aria-hidden="true" />
                  </BaseButton>
                  <BaseButton size="sm" variant="ghost" :disabled="disabled || row.status === 'importing'" title="移除该行" @click="emit('removeRow', row.id)">
                    <Trash2 class="size-3.5" aria-hidden="true" />
                  </BaseButton>
                </div>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </template>
  </BaseFormItem>
</template>
