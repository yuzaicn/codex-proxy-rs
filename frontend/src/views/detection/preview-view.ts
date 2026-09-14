import type { PreviewState } from './useHtmlPreview'
import type { DetectionRecord } from '@/api'
import { emptyResponseNotice } from './presentation'

// 详情弹窗「响应效果」区块的渲染判定，抽成纯函数以便逐态断言（组合式函数与模板只负责执行）。
export type ResponsePreviewView
  = | { kind: 'spinner', text: string }
    | { kind: 'iframe', srcdoc: string }
    | { kind: 'notice', title: string, description: string }
    | { kind: 'error', retryable: true }

export function responsePreviewView(
  record: DetectionRecord,
  state: PreviewState,
): ResponsePreviewView {
  switch (state.status) {
    case 'rendering':
    case 'ready':
      // srcdoc 由 previewDocument 投影，沙箱约束在 iframe 上固定为 allow-scripts。
      return state.srcdoc !== undefined
        ? { kind: 'iframe', srcdoc: state.srcdoc }
        : { kind: 'spinner', text: '正在渲染响应内容…' }
    case 'loading':
      return { kind: 'spinner', text: '正在加载响应内容…' }
    case 'timeout':
      return {
        kind: 'notice',
        title: '响应内容渲染超时',
        description: '响应正文已取回但渲染超时，可关闭弹窗后重试。',
      }
    case 'missing': {
      // 先取后渲染：接口 404 / 空正文一律给占位，绝不把接口的错误 JSON 当页面渲染。
      const notice = emptyResponseNotice(record)
      return { kind: 'notice', title: notice.title, description: notice.description }
    }
    case 'error':
      return { kind: 'error', retryable: true }
    default:
      return { kind: 'spinner', text: '正在加载响应内容…' }
  }
}
