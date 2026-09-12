import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { importAccounts } from '@/api'
import { ApiError } from '@/api/request'
import { useAccountOnboarding } from './useAccountOnboarding'

vi.mock('@/api', () => ({
  completeAccountOAuth: vi.fn(),
  importAccounts: vi.fn(),
  startAccountOAuth: vi.fn(),
}))

vi.mock('@/components/base/BaseToast', () => ({
  toast: {
    success: vi.fn(),
    error: vi.fn(),
  },
}))

const importAccountsMock = vi.mocked(importAccounts)

// 50201 / 50202 在后端同为 HTTP 502，只有业务错误码能区分两者。
function upstreamResultUnknownError() {
  return new ApiError('上游执行结果未知，请刷新状态后再决定是否重试', 502, 50202)
}

function badGatewayError() {
  return new ApiError('上游服务请求失败', 502, 50201)
}

function networkError() {
  return new ApiError('网络连接失败，请检查网络后重试', 0, undefined, undefined, 'network')
}

function setupOnboarding() {
  const onboarding = useAccountOnboarding({ reload: vi.fn().mockResolvedValue(undefined) })
  onboarding.createForm.value.provider = 'openai'
  onboarding.createForm.value.mode = 'auto'
  onboarding.appendTokenText('rt.vitest-fake-refresh-token-1')
  return onboarding
}

function firstRow(onboarding: ReturnType<typeof setupOnboarding>) {
  const row = onboarding.tokenRows.value[0]
  if (!row)
    throw new Error('expected a token row')
  return row
}

async function runWithTimers(task: Promise<unknown>) {
  await vi.runAllTimersAsync()
  await task
}

describe('importTokenRow retry bucketing', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })

  afterEach(() => {
    vi.useRealTimers()
    vi.clearAllMocks()
  })

  it('50202 上游执行结果未知：不自动重试，行状态置 needs_action', async () => {
    importAccountsMock.mockRejectedValue(upstreamResultUnknownError())
    const onboarding = setupOnboarding()

    await runWithTimers(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(1)
    const row = firstRow(onboarding)
    expect(row.status).toBe('needs_action')
    expect(row.retryAttempt).toBe(0)
    expect(row.error).toContain('可能已经导入成功')
    expect(row.error).toContain('刷新账号列表')
    expect(row.error).toContain('refresh token 失效')
    expect(row.error).not.toContain('重试是安全的')
  })

  it('50202 手动重试仍是单次请求，不放大', async () => {
    importAccountsMock.mockRejectedValue(upstreamResultUnknownError())
    const onboarding = setupOnboarding()
    await runWithTimers(onboarding.handleCreate())
    importAccountsMock.mockClear()

    await runWithTimers(onboarding.retryImportRow(firstRow(onboarding).id))

    expect(importAccountsMock).toHaveBeenCalledTimes(1)
    expect(firstRow(onboarding).status).toBe('needs_action')
  })

  it('50201 上游服务请求失败：仍自动重试并可成功', async () => {
    importAccountsMock
      .mockRejectedValueOnce(badGatewayError())
      .mockRejectedValueOnce(badGatewayError())
      .mockResolvedValueOnce({ importedCount: 1, accountIds: ['account-1'] })
    const onboarding = setupOnboarding()
    // 全部成功会关闭弹窗并清空 tokenRows，先持有行引用再断言。
    const row = firstRow(onboarding)

    await runWithTimers(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(3)
    expect(row.status).toBe('success')
    expect(row.result).toBe('已导入')
  })

  it('50201 持续失败：重试上限保持 4 次请求，终态 failed 且文案不变', async () => {
    importAccountsMock.mockRejectedValue(badGatewayError())
    const onboarding = setupOnboarding()

    await runWithTimers(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(4)
    const row = firstRow(onboarding)
    expect(row.status).toBe('failed')
    expect(row.error).toBe('上游服务请求失败')
  })

  it('无业务码的网络错误：维持原自动重试判定', async () => {
    importAccountsMock.mockRejectedValue(networkError())
    const onboarding = setupOnboarding()

    await runWithTimers(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(4)
    expect(firstRow(onboarding).status).toBe('failed')
  })
})
