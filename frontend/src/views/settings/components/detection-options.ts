import type { ReasoningEffort } from '@/api'
import type { SelectOption } from '@/components/base/BaseSelect.vue'

export const REASONING_EFFORT_OPTIONS = [
  { value: 'auto', label: '自动', description: '按模型目录选择支持的最高档位' },
  { value: 'none', label: 'None', description: '关闭推理' },
  { value: 'minimal', label: 'Minimal', description: '最少推理' },
  { value: 'low', label: 'Low', description: '较低推理强度' },
  { value: 'medium', label: 'Medium', description: '平衡速度与推理深度' },
  { value: 'high', label: 'High', description: '较高推理强度' },
  { value: 'xhigh', label: 'Xhigh', description: '极高推理强度' },
  { value: 'max', label: 'Max', description: '模型支持的最大推理强度' },
] satisfies Array<SelectOption & { value: ReasoningEffort }>

const reasoningEfforts = new Set<ReasoningEffort>(REASONING_EFFORT_OPTIONS.map(option => option.value))

export function normalizeReasoningEffort(value: unknown): ReasoningEffort {
  return typeof value === 'string' && reasoningEfforts.has(value as ReasoningEffort)
    ? value as ReasoningEffort
    : 'auto'
}
