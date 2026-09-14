import { describe, expect, it } from 'vitest'

import { normalizeReasoningEffort, REASONING_EFFORT_OPTIONS } from '../detection-options'

describe('降智检测推理档位', () => {
  it('按契约提供 auto 到 max 的完整档位集合', () => {
    expect(REASONING_EFFORT_OPTIONS.map(option => option.value)).toEqual([
      'auto',
      'none',
      'minimal',
      'low',
      'medium',
      'high',
      'xhigh',
      'max',
    ])
  })

  it('兼容旧响应或未知档位并退回 auto', () => {
    expect(normalizeReasoningEffort(undefined)).toBe('auto')
    expect(normalizeReasoningEffort('unknown')).toBe('auto')
    expect(normalizeReasoningEffort('xhigh')).toBe('xhigh')
  })
})
