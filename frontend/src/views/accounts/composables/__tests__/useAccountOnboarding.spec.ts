import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/error'
import { useAccountOnboarding } from '../useAccountOnboarding'

const { importAccountsMock } = vi.hoisted(() => ({ importAccountsMock: vi.fn() }))

vi.mock('@/api', () => ({
  completeAccountOAuth: vi.fn(),
  importAccounts: importAccountsMock,
  startAccountOAuth: vi.fn(),
}))

// BaseToast 的入口会加载 SFC，node 环境必须整体替换。
vi.mock('@/components/base/BaseToast', () => ({
  toast: { success: vi.fn(), error: vi.fn(), warning: vi.fn(), info: vi.fn() },
}))

const RISKY_COPY = '上游执行结果未知，该账号可能已导入。请先刷新账号列表确认，未导入再重试；反复重试可能使该 Refresh Token 永久失效'
const SAFE_CONFLICT_COPY = '上游执行结果未知，账号可能已导入；重试是安全的（同账号会更新，不会重复创建）'

function setup(tokens = 'rt.token-under-test') {
  const onboarding = useAccountOnboarding({ reload: vi.fn(async () => {}) })
  onboarding.createForm.value.provider = 'openai'
  onboarding.createForm.value.mode = 'auto'
  onboarding.appendTokenText(tokens)
  return onboarding
}

// 用假定时器吃掉 1s/2s/4s 退避，让重试路径在测试里同步收敛。
async function runImport(action: Promise<unknown>) {
  await vi.runAllTimersAsync()
  await action
}

function successResponse(count = 1) {
  return { importedCount: count, accountIds: Array.from({ length: count }, (_, i) => `acc-${i}`) }
}

describe('importTokenRow 自动重试判定', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    importAccountsMock.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('502 + code 50202 只发 1 次请求，行置 failed 并显示风险文案', async () => {
    importAccountsMock.mockRejectedValue(new ApiError('上游执行结果未知', 502, 50202, undefined, 'api'))
    const onboarding = setup()
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(1)
    const row = onboarding.tokenRows.value[0]
    expect(row.status).toBe('failed')
    expect(row.error).toBe(RISKY_COPY)
    expect(row.error).not.toContain('重试是安全的')
  })

  it('本地超时（kind=timeout, status 0）只发 1 次请求，行置 failed 并显示风险文案', async () => {
    importAccountsMock.mockRejectedValue(new ApiError('请求超时，请稍后重试', 0, undefined, undefined, 'timeout'))
    const onboarding = setup()
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(1)
    const row = onboarding.tokenRows.value[0]
    expect(row.status).toBe('failed')
    expect(row.error).toBe(RISKY_COPY)
  })

  it.each([
    ['断网（kind=network, status 0）', new ApiError('网络连接失败，请检查网络后重试', 0, undefined, undefined, 'network')],
    ['408 请求超时', new ApiError('请求超时，请稍后重试', 408, undefined, undefined, 'http')],
  ])('%s 只发 1 次请求并显示风险文案', async (_label, error) => {
    importAccountsMock.mockRejectedValue(error)
    const onboarding = setup()
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(1)
    const row = onboarding.tokenRows.value[0]
    expect(row.status).toBe('failed')
    expect(row.error).toBe(RISKY_COPY)
  })

  it('503（非 50202）仍自动重试，第 2 次成功', async () => {
    importAccountsMock
      .mockRejectedValueOnce(new ApiError('服务暂不可用，请稍后重试', 503, 50301, undefined, 'api'))
      .mockResolvedValueOnce(successResponse())
    const onboarding = setup()
    // 全部成功会关闭弹窗并清空 tokenRows，先抓住行引用（行对象为原地变更）。
    const row = onboarding.tokenRows.value[0]
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(2)
    expect(row.status).toBe('success')
  })

  it('无 code 的裸 502 不属于风险桶，仍自动重试', async () => {
    importAccountsMock
      .mockRejectedValueOnce(new ApiError('上游服务请求失败', 502, undefined, undefined, 'http'))
      .mockResolvedValueOnce(successResponse())
    const onboarding = setup()
    const row = onboarding.tokenRows.value[0]
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(2)
    expect(row.status).toBe('success')
  })

  it('500（非 50202）重试到上限后置 failed，文案透传后端 message', async () => {
    importAccountsMock.mockRejectedValue(new ApiError('服务内部错误', 500, 50001, undefined, 'api'))
    const onboarding = setup()
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(4)
    const row = onboarding.tokenRows.value[0]
    expect(row.status).toBe('failed')
    expect(row.error).toBe('服务内部错误')
  })

  it('409 + 40901 行为与文案均不变：自动重试到上限，保留原「重试是安全的」文案', async () => {
    importAccountsMock.mockRejectedValue(new ApiError('资源状态冲突', 409, 40901, undefined, 'api'))
    const onboarding = setup()
    await runImport(onboarding.handleCreate())

    expect(importAccountsMock).toHaveBeenCalledTimes(4)
    const row = onboarding.tokenRows.value[0]
    expect(row.status).toBe('failed')
    expect(row.error).toBe(SAFE_CONFLICT_COPY)
  })
})

describe('手动重试入口', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    importAccountsMock.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('retryImportRow 对风险桶失败行发起单次重试', async () => {
    importAccountsMock.mockRejectedValueOnce(new ApiError('上游执行结果未知', 502, 50202, undefined, 'api'))
    const onboarding = setup()
    await runImport(onboarding.handleCreate())
    expect(importAccountsMock).toHaveBeenCalledTimes(1)

    importAccountsMock.mockResolvedValueOnce(successResponse())
    await runImport(onboarding.retryImportRow(onboarding.tokenRows.value[0].id))

    expect(importAccountsMock).toHaveBeenCalledTimes(2)
    expect(onboarding.tokenRows.value[0].status).toBe('success')
  })

  it('retryFailedImports 覆盖全部失败行', async () => {
    importAccountsMock.mockRejectedValue(new ApiError('请求超时，请稍后重试', 0, undefined, undefined, 'timeout'))
    const onboarding = setup('rt.first-token\nrt.second-token')
    // 全部成功会关闭弹窗并清空 tokenRows，先抓住行引用（行对象为原地变更）。
    const rows = [...onboarding.tokenRows.value]
    await runImport(onboarding.handleCreate())
    expect(importAccountsMock).toHaveBeenCalledTimes(2)
    expect(rows.every(row => row.status === 'failed')).toBe(true)

    importAccountsMock.mockReset()
    importAccountsMock.mockResolvedValue(successResponse())
    await runImport(onboarding.retryFailedImports())

    expect(importAccountsMock).toHaveBeenCalledTimes(2)
    expect(rows).toHaveLength(2)
    expect(rows.every(row => row.status === 'success')).toBe(true)
  })
})
