import { describe, expect, it } from 'vitest'

import { normalizeMatchedPhrases } from '../presentation'

describe('降智检测命中判词展示', () => {
  it('保留后端返回顺序并清理空标签', () => {
    expect(normalizeMatchedPhrases(['内联svg', ' inline与svg间隔≤10字符 ', '', null])).toEqual([
      '内联svg',
      'inline与svg间隔≤10字符',
    ])
  })

  it('兼容旧记录缺少 matched_phrases', () => {
    expect(normalizeMatchedPhrases(undefined)).toEqual([])
    expect(normalizeMatchedPhrases(null)).toEqual([])
  })
})
