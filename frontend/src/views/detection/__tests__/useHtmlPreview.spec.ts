import { beforeEach, describe, expect, it, vi } from 'vitest'

import { previewDocument } from '../useHtmlPreview'

// vitest.config.ts 把本目录的单测固定在 node 环境（不引 Vue 插件、不引 SFC），
// 组合式函数里的 window.setTimeout 只能手动补一个最小实现。
beforeEach(() => {
  vi.stubGlobal('window', { setTimeout: globalThis.setTimeout, clearTimeout: globalThis.clearTimeout })
})

describe('检测响应正文的沙箱投影', () => {
  it('无 head 的片段包一层带 CSP 的完整文档', () => {
    const doc = previewDocument('<p>hello</p>')
    expect(doc).toContain('Content-Security-Policy')
    expect(doc).toContain('sandbox allow-scripts')
    expect(doc).toContain('<p>hello</p>')
  })

  it('已有 head 时把 CSP 插进 head 而不重建文档', () => {
    const doc = previewDocument('<html><head><title>t</title></head><body>x</body></html>')
    expect(doc.match(/<head[^>]*>/gi)).toHaveLength(1)
    expect(doc.indexOf('Content-Security-Policy')).toBeLessThan(doc.indexOf('<title>t</title>'))
  })

  it('不得放宽沙箱：投影只声明 allow-scripts，不出现 allow-same-origin', () => {
    expect(previewDocument('<p>x</p>')).not.toContain('allow-same-origin')
  })
})

describe('fetch 失败态不落入 ready', () => {
  it('404 记录被判为 missing 而不是渲染错误正文', async () => {
    const { useHtmlPreview } = await import('../useHtmlPreview')
    vi.stubGlobal('fetch', vi.fn(async () => new Response(
      JSON.stringify({ code: 40401, message: '检测记录不存在或没有 HTML 内容', data: null }),
      { status: 404, headers: { 'content-type': 'application/json' } },
    )))

    const preview = useHtmlPreview()
    await preview.load({ id: 7, detector: 'legacy' })

    const state = preview.getState(7)
    expect(state.status).toBe('missing')
    expect(state.srcdoc).toBeUndefined()
  })

  it('正文取回后进入 rendering 并持有投影后的 srcdoc', async () => {
    const { useHtmlPreview } = await import('../useHtmlPreview')
    vi.stubGlobal('fetch', vi.fn(async () => new Response('<p>ok</p>', { status: 200 })))

    const preview = useHtmlPreview()
    await preview.load({ id: 8, detector: 'legacy' })

    const state = preview.getState(8)
    expect(state.status).toBe('rendering')
    expect(state.srcdoc).toContain('<p>ok</p>')
  })

  it('fingerprint 行直接进入不支持回放占位且不发请求', async () => {
    const { useHtmlPreview } = await import('../useHtmlPreview')
    const fetchMock = vi.fn()
    vi.stubGlobal('fetch', fetchMock)

    const preview = useHtmlPreview()
    await preview.load({ id: 9, detector: 'fingerprint' })

    expect(preview.getState(9)).toMatchObject({ status: 'unavailable' })
    expect(fetchMock).not.toHaveBeenCalled()
  })
})
