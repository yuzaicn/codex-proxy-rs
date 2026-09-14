import { describe, expect, it } from 'vitest'

import { emptyResponseNotice, isProbeFailureReasoning, normalizeMatchedPhrases } from '../presentation'

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

describe('探测失败留痕的判别', () => {
  it('识别后端失败分支写入的固定前缀', () => {
    expect(isProbeFailureReasoning('探测失败（reasoning_effort=max）：request deadline exceeded')).toBe(true)
  })

  it('识别带退档提示的失败留痕', () => {
    const reasoning = 'reasoning_effort=xhigh 被上游拒绝，本轮已自动退档到 high；探测失败（reasoning_effort=high）：upstream 5xx'
    expect(isProbeFailureReasoning(reasoning)).toBe(true)
  })

  it('只认开头的失败留痕，正文深处提到同名词组不算', () => {
    const reasoning = `这是一段很长的正常思考过程。${'内容。'.repeat(200)}最后提到探测失败（reasoning_effort=max）这个说法。`
    expect(isProbeFailureReasoning(reasoning)).toBe(false)
  })

  it('空值与无内容一律判否', () => {
    expect(isProbeFailureReasoning(null)).toBe(false)
    expect(isProbeFailureReasoning(undefined)).toBe(false)
    expect(isProbeFailureReasoning('   ')).toBe(false)
  })
})

describe('详情弹窗空响应占位文案', () => {
  it('失败留痕给出「探测失败」而非「加载失败」', () => {
    const notice = emptyResponseNotice({
      degraded: false,
      matched_phrases: [],
      reasoning_content: '探测失败（reasoning_effort=max）：request deadline exceeded',
    })
    expect(notice.title).toContain('探测失败')
    expect(notice.title).not.toContain('加载失败')
    expect(notice.description).not.toContain('加载失败')
  })

  it('成功探测但无正文时不给失败结论', () => {
    const notice = emptyResponseNotice({ degraded: false, matched_phrases: [], reasoning_content: '普通思考过程' })
    expect(notice.title).not.toContain('探测失败')
  })

  it('探测失败但命中判词为空时，只按「无正文」解释，不误报降智', () => {
    const notice = emptyResponseNotice({ degraded: false, matched_phrases: [], reasoning_content: '没有留痕文本' })
    expect(notice.title).not.toContain('探测失败')
    expect(notice.description).not.toContain('加载失败')
  })

  it('命中判词的记录与失败留痕区分开', () => {
    const notice = emptyResponseNotice({ degraded: true, matched_phrases: ['内联svg'], reasoning_content: '普通思考过程' })
    expect(notice.title).not.toContain('探测失败')
    expect(notice.description).not.toContain('加载失败')
  })
})
