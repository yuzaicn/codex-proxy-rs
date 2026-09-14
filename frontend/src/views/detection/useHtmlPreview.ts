import { reactive } from 'vue'

import { getDetectionRecordHtmlUrl } from '@/api'

export type PreviewStatus = 'idle' | 'loading' | 'rendering' | 'ready' | 'missing' | 'error' | 'timeout'

export interface PreviewState {
  status: PreviewStatus
  srcdoc?: string
  controller?: AbortController
  renderTimeout?: number
}

const PREVIEW_FETCH_TIMEOUT_MS = 8_000
const PREVIEW_RENDER_TIMEOUT_MS = 4_000

export function previewDocument(html: string) {
  const cspMeta = '<meta http-equiv="Content-Security-Policy" content="sandbox allow-scripts">'
  if (/<head\b[^>]*>/i.test(html))
    return html.replace(/<head\b[^>]*>/i, match => `${match}${cspMeta}`)
  return `<!doctype html><html><head>${cspMeta}</head><body>${html}</body></html>`
}

// 列表缩略图与详情弹窗共用同一套「先取后渲染」状态机：
// 先 fetch 正文再投到 sandbox iframe 的 srcdoc，绝不把接口的错误正文当页面渲染。
export function useHtmlPreview() {
  const states = reactive<Record<number, PreviewState>>({})

  function getState(recordId: number): PreviewState {
    return states[recordId] ?? (states[recordId] = { status: 'idle' })
  }

  function clearRenderTimeout(state: PreviewState) {
    if (state.renderTimeout !== undefined) {
      window.clearTimeout(state.renderTimeout)
      state.renderTimeout = undefined
    }
  }

  function cancel(recordId: number) {
    const state = getState(recordId)
    if (state.status !== 'loading' && state.status !== 'rendering')
      return
    clearRenderTimeout(state)
    state.controller?.abort()
    state.controller = undefined
    state.srcdoc = undefined
    state.status = 'idle'
  }

  async function load(recordId: number) {
    const state = getState(recordId)
    if (state.status === 'loading' || state.status === 'rendering')
      return

    const controller = new AbortController()
    state.controller = controller
    state.srcdoc = undefined
    state.status = 'loading'
    const fetchTimeout = window.setTimeout(() => {
      if (state.status === 'loading') {
        state.status = 'timeout'
        controller.abort()
      }
    }, PREVIEW_FETCH_TIMEOUT_MS)

    try {
      const response = await fetch(getDetectionRecordHtmlUrl(recordId), {
        credentials: 'same-origin',
        signal: controller.signal,
      })
      // 404 / 204 都表示这条记录没有 html_content：不是「加载失败」，由调用方给占位态。
      if (response.status === 404 || response.status === 204) {
        state.status = 'missing'
        return
      }
      if (!response.ok) {
        state.status = 'error'
        return
      }

      const html = await response.text()
      if (!html.trim()) {
        state.status = 'missing'
        return
      }

      state.srcdoc = previewDocument(html)
      state.status = 'rendering'
      state.renderTimeout = window.setTimeout(() => {
        if (state.status === 'rendering')
          state.status = 'timeout'
      }, PREVIEW_RENDER_TIMEOUT_MS)
    }
    catch {
      if (!controller.signal.aborted)
        state.status = 'error'
    }
    finally {
      window.clearTimeout(fetchTimeout)
      if (state.controller === controller)
        state.controller = undefined
    }
  }

  function markRendered(recordId: number) {
    const state = getState(recordId)
    if (state.status !== 'rendering')
      return
    clearRenderTimeout(state)
    state.status = 'ready'
  }

  function markRenderFailed(recordId: number) {
    const state = getState(recordId)
    if (state.status !== 'rendering')
      return
    clearRenderTimeout(state)
    state.status = 'error'
  }

  function cancelAll() {
    for (const recordId of Object.keys(states))
      cancel(Number(recordId))
  }

  return { states, getState, load, cancel, cancelAll, markRendered, markRenderFailed }
}
