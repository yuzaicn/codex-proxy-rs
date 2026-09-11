<script setup lang="ts">
import type { Account, DetectionConfig } from '@/api'

import { RefreshCw, Save } from '@lucide/vue'
import { computed, reactive, ref, watch } from 'vue'

import { getAccounts, getDetectionConfig, updateDetectionConfig } from '@/api'
import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseCheckbox from '@/components/base/BaseCheckbox.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'
import { toast } from '@/components/base/BaseToast'
import { useSettingsStore } from '@/stores/modules/settings'
import { errorMessage } from '@/utils/async'

const props = defineProps<{
  active?: boolean
}>()
const settingsStore = useSettingsStore()

const loading = ref(false)
const saving = ref(false)
const error = ref('')
const accounts = ref<Account[]>([])
const accountsLoading = ref(false)
const form = reactive({
  enabled: false,
  allAccounts: true,
  accountIds: [] as string[],
  intervalSecs: '3600',
  model: '',
})

const selectedCount = computed(() => form.accountIds.length)
const canSave = computed(() => !loading.value && !saving.value)

function applyConfig(config: DetectionConfig) {
  const scope = config.account_scope ?? {}
  form.enabled = Boolean(config.enabled)
  form.allAccounts = scope.all === true || !Array.isArray(scope.account_ids)
  form.accountIds = Array.isArray(scope.account_ids) ? [...scope.account_ids] : []
  form.intervalSecs = Number.isFinite(config.interval_secs) ? String(config.interval_secs) : '3600'
  form.model = config.model ?? ''
}

async function loadAccounts() {
  accountsLoading.value = true
  try {
    const result = await getAccounts({ page: 1, pageSize: 1000 })
    accounts.value = result.items
  }
  catch (cause: unknown) {
    toast.error(errorMessage(cause, '账号列表加载失败'))
  }
  finally {
    accountsLoading.value = false
  }
}

async function load() {
  if (loading.value)
    return
  loading.value = true
  error.value = ''
  try {
    const config = await getDetectionConfig()
    applyConfig(config)
  }
  catch (cause: unknown) {
    error.value = errorMessage(cause, '降智检测配置加载失败')
    toast.error(error.value)
  }
  finally {
    loading.value = false
  }
}

function reverseSelection() {
  const selected = new Set(form.accountIds)
  form.accountIds = accounts.value
    .map(account => account.id)
    .filter(accountId => !selected.has(accountId))
}

function selectAllAccounts() {
  form.accountIds = accounts.value.map(account => account.id)
}

function toggleAccount(accountId: string, selected: boolean) {
  const next = new Set(form.accountIds)
  if (selected)
    next.add(accountId)
  else
    next.delete(accountId)
  form.accountIds = [...next]
}

function buildPayload(): DetectionConfig | null {
  const intervalSecs = Number(form.intervalSecs)
  if (!Number.isInteger(intervalSecs) || intervalSecs < 60 || intervalSecs > 86400) {
    toast.warning('检测间隔需为 60–86400 秒的整数')
    return null
  }
  const model = form.model.trim()
  if (form.enabled && !model) {
    toast.warning('请输入检测模型')
    return null
  }

  return {
    enabled: form.enabled,
    account_scope: form.allAccounts ? { all: true } : { account_ids: [...form.accountIds] },
    interval_secs: intervalSecs,
    model,
  }
}

async function save() {
  if (!canSave.value)
    return
  const payload = buildPayload()
  if (!payload)
    return

  saving.value = true
  try {
    const result = await updateDetectionConfig(payload)
    applyConfig(result)
    settingsStore.applyDetectionConfig(result)
    toast.success('降智检测配置已保存')
  }
  catch (cause: unknown) {
    error.value = errorMessage(cause, '降智检测配置保存失败')
    toast.error(error.value)
    await load()
  }
  finally {
    saving.value = false
  }
}

defineExpose({ load })

watch(
  () => props.active,
  (active) => {
    if (active) {
      void load()
      void loadAccounts()
    }
  },
  { immediate: true },
)
</script>

<template>
  <BaseCard title="降智检测" description="定期检测账号响应质量，识别可能的能力降级">
    <template #actions>
      <BaseButton variant="primary" :loading="saving" :disabled="!canSave" @click="save">
        <template #icon>
          <Save class="size-4" />
        </template>
        {{ saving ? '保存中...' : '保存' }}
      </BaseButton>
    </template>

    <div class="grid gap-4">
      <p v-if="error" class="m-0 rounded-cp bg-cp-error-container px-3.5 py-3 text-cp-sm font-emphasis text-cp-error-on-container">
        {{ error }}
      </p>

      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <div class="col-span-full flex min-h-6 items-center justify-between gap-3">
          <span class="text-cp leading-none font-medium text-cp-text-secondary">检测服务</span>
          <BaseSwitch v-model="form.enabled" label="启用降智检测" show-label :disabled="loading" />
        </div>

        <BaseFormItem label="检测账号范围" description="可选择全部账号，或反选后仅检测指定账号">
          <div class="grid gap-3">
            <BaseSwitch v-model="form.allAccounts" label="全部账号" show-label :disabled="!form.enabled || loading" />
            <div v-if="!form.allAccounts" class="grid gap-3">
              <div class="flex flex-wrap items-center gap-2">
                <BaseButton size="sm" variant="ghost" :disabled="!form.enabled || accountsLoading || accounts.length === 0" @click="selectAllAccounts">
                  全选
                </BaseButton>
                <BaseButton size="sm" variant="ghost" :disabled="!form.enabled || accountsLoading || accounts.length === 0" @click="reverseSelection">
                  <template #icon>
                    <RefreshCw class="size-3.5" />
                  </template>
                  反选
                </BaseButton>
                <span class="text-cp-xs font-emphasis text-cp-text-quaternary">已选 {{ selectedCount }} 个</span>
              </div>
              <div v-if="accounts.length > 0" class="grid gap-2 sm:grid-cols-2">
                <div
                  v-for="account in accounts"
                  :key="account.id"
                  class="flex min-h-11 items-center rounded-cp bg-cp-fill-quaternary px-3.5 py-2.5"
                >
                  <BaseCheckbox
                    :model-value="form.accountIds.includes(account.id)"
                    :label="account.email || account.name || account.id"
                    show-label
                    :disabled="!form.enabled || loading || accountsLoading"
                    @update:model-value="toggleAccount(account.id, $event)"
                  />
                </div>
              </div>
              <p v-else class="m-0 rounded-cp bg-cp-fill-quaternary px-3.5 py-3 text-cp-sm font-emphasis text-cp-text-quaternary">
                {{ accountsLoading ? '正在加载账号...' : '暂无可选账号。' }}
              </p>
            </div>
          </div>
        </BaseFormItem>

        <BaseFormItem label="检测间隔（秒）" description="范围 60–86400 秒">
          <BaseInput
            v-model="form.intervalSecs"
            aria-label="检测间隔（秒）"
            type="number"
            min="60"
            max="86400"
            :disabled="!form.enabled || loading"
          />
        </BaseFormItem>

        <BaseFormItem label="检测模型" description="用于检测请求的模型名称">
          <BaseInput v-model="form.model" aria-label="检测模型" placeholder="例如 gpt-5" :disabled="!form.enabled || loading" />
        </BaseFormItem>
      </BaseForm>
    </div>
  </BaseCard>
</template>
