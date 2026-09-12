<script setup lang="ts">
import type { ResetDetectionAccountScope } from '@/api'

import { Clock3, CreditCard, Save, ShieldCheck } from '@lucide/vue'

import BaseButton from '@/components/base/BaseButton.vue'
import BaseCard from '@/components/base/BaseCard.vue'
import BaseFormItem from '@/components/base/BaseForm/FormItem.vue'
import BaseForm from '@/components/base/BaseForm/index.vue'
import BaseInput from '@/components/base/BaseInput.vue'
import BaseSelect from '@/components/base/BaseSelect.vue'
import BaseSwitch from '@/components/base/BaseSwitch.vue'

defineProps<{
  loading: boolean
  saving: boolean
  error: string
}>()

const emit = defineEmits<{
  save: []
}>()

const enabled = defineModel<boolean>('enabled', { required: true })
const interval = defineModel<string>('interval', { required: true })
const intervalUnit = defineModel<'seconds' | 'hours'>('intervalUnit', { required: true })
const accountScope = defineModel<ResetDetectionAccountScope>('accountScope', { required: true })
const autoUseResetCard = defineModel<boolean>('autoUseResetCard', { required: true })

const INTERVAL_UNIT_OPTIONS = [
  { label: '秒', value: 'seconds' },
  { label: '小时', value: 'hours' },
]

const ACCOUNT_SCOPE_OPTIONS = [
  { label: '全部账号（不含错误）', value: 'all_non_error' },
  { label: '正常账号', value: 'normal' },
  { label: '受限账号', value: 'limited' },
]
</script>

<template>
  <BaseCard title="重置检测" description="按账号状态轮询检测可用的主动额度重置卡">
    <template #actions>
      <BaseButton variant="primary" :loading="saving" :disabled="loading" @click="emit('save')">
        <template #icon>
          <Save class="size-4" />
        </template>
        {{ saving ? '保存中...' : '保存' }}
      </BaseButton>
    </template>

    <div class="grid gap-4">
      <p
        v-if="error"
        class="m-0 rounded-cp bg-cp-error-container px-3.5 py-3 text-cp-sm font-emphasis text-cp-error-on-container"
      >
        {{ error }}
      </p>

      <BaseForm class="max-w-6xl sm:grid-cols-2">
        <div class="col-span-full flex min-h-6 items-center justify-between gap-3">
          <span class="text-cp leading-none font-medium text-cp-text-secondary">重置检测服务</span>
          <BaseSwitch v-model="enabled" label="启用重置检测" show-label :disabled="loading" />
        </div>

        <BaseFormItem label="轮询周期" description="后端按秒保存；小于 30 秒无法提交">
          <div class="flex min-w-0 gap-2">
            <BaseInput
              v-model="interval"
              aria-label="轮询周期"
              type="number"
              min="1"
              class="min-w-0 flex-1"
              :disabled="!enabled || loading"
            >
              <template #prefix>
                <Clock3 class="size-4" />
              </template>
            </BaseInput>
            <BaseSelect
              v-model="intervalUnit"
              aria-label="轮询周期单位"
              class="w-28 shrink-0"
              :options="INTERVAL_UNIT_OPTIONS"
              :disabled="!enabled || loading"
            />
          </div>
        </BaseFormItem>

        <BaseFormItem label="检测范围" description="选择需要轮询检测的账号状态">
          <BaseSelect
            v-model="accountScope"
            aria-label="检测范围"
            :options="ACCOUNT_SCOPE_OPTIONS"
            :disabled="!enabled || loading"
          />
        </BaseFormItem>

        <div class="col-span-full flex flex-col gap-2 rounded-cp bg-cp-fill-quaternary px-3.5 py-3">
          <div class="flex min-h-6 items-center justify-between gap-3">
            <div class="flex min-w-0 items-center gap-2.5">
              <CreditCard class="size-4 shrink-0 text-cp-primary-text" />
              <span class="text-cp leading-none font-medium text-cp-text-secondary">自动使用重置卡</span>
            </div>
            <BaseSwitch
              v-model="autoUseResetCard"
              label="自动使用重置卡"
              show-label
              :disabled="!enabled || loading"
            />
          </div>
          <p class="m-0 pl-6 text-cp-xs leading-[1.4] font-emphasis text-cp-warning-text">
            仅对额度耗尽的 OpenAI 账号自动消费；429 限流账号不消费（重置卡不能解除限流冷却）。消费不可撤销，请确认后再启用。
          </p>
        </div>

        <p class="col-span-full m-0 flex items-center gap-2 text-cp-xs font-emphasis text-cp-text-quaternary">
          <ShieldCheck class="size-3.5 shrink-0" />
          关闭重置检测时会保留轮询周期、检测范围与自动使用设置，重新开启后无需重复配置。
        </p>
      </BaseForm>
    </div>
  </BaseCard>
</template>
