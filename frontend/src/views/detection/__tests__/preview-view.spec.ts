import type { DetectionRecord } from '@/api'
import { describe, expect, it } from 'vitest'

import { responsePreviewView } from '../preview-view'

// 线上实测（2026-09-14，2244 条中 337 条）失败留痕的 reasoning_content 就是这段形状。
const failedProbe: DetectionRecord = {
  id: 1001,
  detection_round_id: 'round-1',
  account_id: 'acct-failed',
  checked_at: '2026-09-14T12:00:00Z',
  degraded: false,
  scheduling_suspended: false,
  reasoning_content: '探测失败（reasoning_effort=max）：request deadline exceeded',
  prompt_used: '请用一句话描述这张图片',
  matched_phrases: [],
}

const normalRecord: DetectionRecord = {
  id: 1002,
  detection_round_id: 'round-1',
  account_id: 'acct-normal',
  checked_at: '2026-09-14T12:00:01Z',
  degraded: false,
  scheduling_suspended: false,
  reasoning_content: '用户要求描述图片，我照做了。',
  prompt_used: '请用一句话描述这张图片',
  matched_phrases: [],
}

const projectedHtml = '<!doctype html><html><head><meta http-equiv="Content-Security-Policy" content="sandbox allow-scripts"></head><body><p>一只猫</p></body></html>'

describe('详情弹窗「响应效果」区块的状态判定', () => {
  it('正常记录渲染 srcdoc，不落占位', () => {
    const view = responsePreviewView(normalRecord, { status: 'ready', srcdoc: projectedHtml })
    expect(view.kind).toBe('iframe')
    if (view.kind === 'iframe')
      expect(view.srcdoc).toContain('一只猫')
  })

  it('探测失败记录（接口 404）只给占位，永远不产出 iframe', () => {
    const view = responsePreviewView(failedProbe, { status: 'missing' })
    expect(view.kind).toBe('notice')
    if (view.kind === 'notice') {
      expect(view.title).toContain('探测失败')
      expect(`${view.title}${view.description}`).not.toContain('40401')
      expect(`${view.title}${view.description}`).not.toContain('检测记录不存在')
    }
  })

  it('带响应正文的正常记录不会被误判成空响应占位', () => {
    const view = responsePreviewView(normalRecord, { status: 'missing', srcdoc: projectedHtml })
    expect(view.kind).toBe('notice')
    if (view.kind === 'notice') {
      expect(view.title).toContain('没有')
      expect(view.title).not.toContain('探测失败')
    }
  })

  it('加载中与渲染中都是转圈，不空白', () => {
    expect(responsePreviewView(normalRecord, { status: 'loading' })).toEqual({ kind: 'spinner', text: '正在加载响应内容…' })
    // rendering 但 srcdoc 尚未投影出来时也只给转圈，不落空白 iframe。
    expect(responsePreviewView(normalRecord, { status: 'rendering' })).toEqual({ kind: 'spinner', text: '正在渲染响应内容…' })
  })

  it('超时给出可重试的说明而不是空白', () => {
    const view = responsePreviewView(normalRecord, { status: 'timeout' })
    expect(view.kind).toBe('notice')
    if (view.kind === 'notice')
      expect(view.title).toContain('超时')
  })

  it('请求失败给出告警态并允许重试', () => {
    expect(responsePreviewView(normalRecord, { status: 'error' })).toEqual({ kind: 'error', retryable: true })
  })

  it('idle 起步态等同加载中，弹窗不会出现空白区块', () => {
    expect(responsePreviewView(normalRecord, { status: 'idle' }).kind).toBe('spinner')
  })
})
