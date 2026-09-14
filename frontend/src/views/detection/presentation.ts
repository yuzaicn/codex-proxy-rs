export function normalizeMatchedPhrases(value: unknown): string[] {
  if (!Array.isArray(value))
    return []

  return value.flatMap(phrase => typeof phrase === 'string' && phrase.trim() ? [phrase.trim()] : [])
}

// 探测失败留痕的固定前缀，与后端 workers/intelligence_detection.rs 的失败分支同源。
// 前置的退档提示（`…；探测失败（…）`）不参与匹配，只在这些开头的字符内找。
const PROBE_FAILURE_MARKER = '探测失败（'
const PROBE_FAILURE_SCAN_CHARS = 240

export function isProbeFailureReasoning(reasoning: string | null | undefined): boolean {
  const text = reasoning?.trim()
  if (!text)
    return false
  return text.slice(0, PROBE_FAILURE_SCAN_CHARS).includes(PROBE_FAILURE_MARKER)
}

// 详情弹窗「响应效果」区块四种非就绪态的判别文案。
// 空响应的成因按证据强度排序：失败留痕原文 > 成功但正文为空 > 无从判别。
export function emptyResponseNotice(record: { reasoning_content?: string | null, degraded: boolean, matched_phrases?: string[] | null }) {
  if (isProbeFailureReasoning(record.reasoning_content)) {
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
