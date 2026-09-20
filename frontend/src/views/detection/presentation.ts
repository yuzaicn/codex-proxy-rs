import type { DetectionProbeResult, DetectionRecord, DetectionVerdict } from '@/api'

export function normalizeMatchedPhrases(value: unknown): string[] {
  if (!Array.isArray(value))
    return []

  return value.flatMap(phrase => typeof phrase === 'string' && phrase.trim() ? [phrase.trim()] : [])
}

// 探测失败留痕的固定前缀，与后端 workers/intelligence_detection.rs 的失败分支同源。
// 前置的退档提示（`…；探测失败（…）`）不参与匹配，只在这些开头的字符内找。
const PROBE_FAILURE_MARKER = '探测失败（'
const PROBE_FAILURE_SCAN_CHARS = 240

export function isLegacyRecord(record: Pick<DetectionRecord, 'detector'>): boolean {
  return record.detector !== 'fingerprint'
}

export function isProbeFailureReasoning(record: Pick<DetectionRecord, 'detector' | 'reasoning_content'>): boolean {
  if (!isLegacyRecord(record))
    return false

  const text = record.reasoning_content?.trim()
  if (!text)
    return false
  return text.slice(0, PROBE_FAILURE_SCAN_CHARS).includes(PROBE_FAILURE_MARKER)
}

export type DetectionVerdictTone = 'success' | 'error' | 'warning' | 'neutral'

export interface DetectionVerdictPresentation {
  verdict: DetectionVerdict
  label: string
  assistiveLabel: string
  tone: DetectionVerdictTone
}

export function detectionVerdictPresentation(
  record: Pick<DetectionRecord, 'degraded' | 'detector' | 'verdict'>,
): DetectionVerdictPresentation {
  if (isLegacyRecord(record)) {
    return record.degraded
      ? { verdict: 'legacy', label: '降智', assistiveLabel: '该账号判定为降智', tone: 'error' }
      : { verdict: 'legacy', label: '不降智', assistiveLabel: '该账号判定为不降智', tone: 'success' }
  }

  switch (record.verdict) {
    case 'normal':
      return { verdict: record.verdict, label: '身份一致', assistiveLabel: '模型身份一致', tone: 'success' }
    case 'degraded':
      return { verdict: record.verdict, label: '身份不一致', assistiveLabel: '模型身份不一致', tone: 'error' }
    case 'undetermined':
      return { verdict: record.verdict, label: '证据不足', assistiveLabel: '模型身份一致性证据不足', tone: 'warning' }
    case 'unverifiable':
    default:
      return { verdict: record.verdict, label: '无法归因', assistiveLabel: '本轮无法归因模型身份', tone: 'neutral' }
  }
}

export interface FingerprintEvidencePresentation {
  kind: 'fingerprint'
  margin: string
  claimedModel: string
  rankOneModel: string
  bankId: string
  challenges: DetectionProbeResult[]
}

export interface LegacyEvidencePresentation {
  kind: 'legacy'
  matchedPhrases: string[]
}

export function detectionEvidencePresentation(record: DetectionRecord): FingerprintEvidencePresentation | LegacyEvidencePresentation {
  if (isLegacyRecord(record)) {
    return {
      kind: 'legacy',
      matchedPhrases: normalizeMatchedPhrases(record.matched_phrases),
    }
  }

  const challenges = Array.isArray(record.probe_payload)
    ? record.probe_payload.filter((item): item is DetectionProbeResult => (
        item !== null
        && typeof item === 'object'
        && typeof item.challenge_id === 'string'
        && typeof item.ok === 'boolean'
        && typeof item.parsed_count === 'number'
      ))
    : []

  return {
    kind: 'fingerprint',
    margin: typeof record.margin === 'number' && Number.isFinite(record.margin)
      ? record.margin.toFixed(3)
      : '未生成',
    claimedModel: record.claimed_model?.trim() || '未记录',
    rankOneModel: record.rank_one?.trim() || '未返回',
    bankId: record.bank_id?.trim() || '未记录',
    challenges,
  }
}

// 详情弹窗「响应效果」区块四种非就绪态的判别文案。
// 空响应的成因按证据强度排序：失败留痕原文 > 成功但正文为空 > 无从判别。
export function emptyResponseNotice(record: Pick<DetectionRecord, 'degraded' | 'detector'> & Partial<Pick<DetectionRecord, 'matched_phrases' | 'reasoning_content'>>) {
  if (!isLegacyRecord(record)) {
    return {
      title: '本代检测不产出 HTML 回放',
      description: '指纹检测只保存身份归因结果与挑战状态，请在上方查看本轮证据。',
    }
  }

  if (isProbeFailureReasoning(record)) {
    return {
      title: '本次探测失败，没有响应内容',
      description: '该账号这一轮没有产出模型响应，记录只保留了失败原因（见上方「思考过程」）。页面没有出错，这只是探测本身没拿到响应。',
    }
  }

  const matched = normalizeMatchedPhrases(record.matched_phrases)
  if (!record.degraded && matched.length === 0) {
    return {
      title: '该记录没有响应内容',
      description: '这次探测没有留下可渲染的响应原文，判定结论仍以上方「命中判词」与列表结论为准。',
    }
  }

  return {
    title: '该记录没有保存响应内容',
    description: '这条记录没有可渲染的响应原文，可能是旧数据或响应为空。',
  }
}
