import type { DetectionRecord } from '@/api'
import { describe, expect, it } from 'vitest'

import {
  detectionEvidencePresentation,
  detectionVerdictPresentation,
  emptyResponseNotice,
  isProbeFailureReasoning,
  normalizeMatchedPhrases,
} from '../presentation'

const legacyRecord: DetectionRecord = {
  id: 1,
  detection_round_id: 'round-legacy',
  account_id: 'acct-legacy',
  checked_at: '2026-09-20T08:00:00Z',
  degraded: true,
  verdict: 'legacy',
  detector: 'legacy',
  scheduling_suspended: true,
  reasoning_content: '普通思考过程',
  matched_phrases: ['内联svg'],
}

const fingerprintRecord: DetectionRecord = {
  id: 2,
  detection_round_id: 'round-fingerprint',
  account_id: 'acct-fingerprint',
  checked_at: '2026-09-20T09:00:00Z',
  degraded: false,
  verdict: 'undetermined',
  detector: 'fingerprint',
  scheduling_suspended: false,
  margin: 0.1254,
  claimed_model: 'gpt-5.6-sol',
  rank_one: 'gpt-5.6-terra',
  bank_id: 'modeltrace-bank-20260912',
  probe_payload: [
    { challenge_id: 'query-01', ok: true, parsed_count: 300 },
    { challenge_id: 'query-02', ok: false, parsed_count: 0, note: '上游超时' },
    { challenge_id: 'query-03', ok: true, parsed_count: 298 },
  ],
}

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
    expect(isProbeFailureReasoning({ detector: 'legacy', reasoning_content: '探测失败（reasoning_effort=max）：request deadline exceeded' })).toBe(true)
  })

  it('识别带退档提示的失败留痕', () => {
    const reasoning = 'reasoning_effort=xhigh 被上游拒绝，本轮已自动退档到 high；探测失败（reasoning_effort=high）：upstream 5xx'
    expect(isProbeFailureReasoning({ detector: 'legacy', reasoning_content: reasoning })).toBe(true)
  })

  it('只认开头的失败留痕，正文深处提到同名词组不算', () => {
    const reasoning = `这是一段很长的正常思考过程。${'内容。'.repeat(200)}最后提到探测失败（reasoning_effort=max）这个说法。`
    expect(isProbeFailureReasoning({ detector: 'legacy', reasoning_content: reasoning })).toBe(false)
  })

  it('空值与无内容一律判否', () => {
    expect(isProbeFailureReasoning({ detector: 'legacy', reasoning_content: null })).toBe(false)
    expect(isProbeFailureReasoning({ detector: 'legacy', reasoning_content: undefined })).toBe(false)
    expect(isProbeFailureReasoning({ detector: 'legacy', reasoning_content: '   ' })).toBe(false)
  })

  it('fingerprint 行不再解析 legacy 失败前缀', () => {
    expect(isProbeFailureReasoning({
      detector: 'fingerprint',
      reasoning_content: '探测失败（reasoning_effort=max）：旧格式文本',
    })).toBe(false)
  })
})

describe('legacy 与 fingerprint 双态展示', () => {
  it('legacy 行保持原结论与命中判词', () => {
    expect(detectionVerdictPresentation(legacyRecord)).toMatchObject({ label: '降智', tone: 'error' })
    expect(detectionEvidencePresentation(legacyRecord)).toEqual({
      kind: 'legacy',
      matchedPhrases: ['内联svg'],
    })
  })

  it('fingerprint 行展示身份结论、margin、模型对照与三条挑战', () => {
    expect(detectionVerdictPresentation(fingerprintRecord)).toMatchObject({ label: '证据不足', tone: 'warning' })
    expect(detectionEvidencePresentation(fingerprintRecord)).toEqual({
      kind: 'fingerprint',
      margin: '0.125',
      claimedModel: 'gpt-5.6-sol',
      rankOneModel: 'gpt-5.6-terra',
      bankId: 'modeltrace-bank-20260912',
      challenges: fingerprintRecord.probe_payload,
    })
  })

  it.each([
    ['normal', '身份一致', 'success'],
    ['degraded', '身份不一致', 'error'],
    ['undetermined', '证据不足', 'warning'],
    ['unverifiable', '无法归因', 'neutral'],
  ] as const)('fingerprint verdict %s 映射为独立结论', (verdict, label, tone) => {
    expect(detectionVerdictPresentation({
      degraded: verdict === 'degraded',
      detector: 'fingerprint',
      verdict,
    })).toMatchObject({ label, tone })
  })
})

describe('详情弹窗空响应占位文案', () => {
  it('失败留痕给出「探测失败」而非「加载失败」', () => {
    const notice = emptyResponseNotice({
      degraded: false,
      detector: 'legacy',
      matched_phrases: [],
      reasoning_content: '探测失败（reasoning_effort=max）：request deadline exceeded',
    })
    expect(notice.title).toContain('探测失败')
    expect(notice.title).not.toContain('加载失败')
    expect(notice.description).not.toContain('加载失败')
  })

  it('成功探测但无正文时不给失败结论', () => {
    const notice = emptyResponseNotice({ degraded: false, detector: 'legacy', matched_phrases: [], reasoning_content: '普通思考过程' })
    expect(notice.title).not.toContain('探测失败')
  })

  it('探测失败但命中判词为空时，只按「无正文」解释，不误报降智', () => {
    const notice = emptyResponseNotice({ degraded: false, detector: 'legacy', matched_phrases: [], reasoning_content: '没有留痕文本' })
    expect(notice.title).not.toContain('探测失败')
    expect(notice.description).not.toContain('加载失败')
  })

  it('命中判词的记录与失败留痕区分开', () => {
    const notice = emptyResponseNotice({ degraded: true, detector: 'legacy', matched_phrases: ['内联svg'], reasoning_content: '普通思考过程' })
    expect(notice.title).not.toContain('探测失败')
    expect(notice.description).not.toContain('加载失败')
  })
})
