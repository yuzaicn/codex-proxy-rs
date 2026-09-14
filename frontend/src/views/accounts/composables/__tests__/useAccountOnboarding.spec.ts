import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { ApiError } from '@/api/error'
import { parseOpenAiTokenRows, tokenImportSummary, useAccountOnboarding } from '../useAccountOnboarding'

const { importAccountsMock, toastSuccessMock } = vi.hoisted(() => ({
  importAccountsMock: vi.fn(),
  toastSuccessMock: vi.fn(),
}))

vi.mock('@/api', () => ({
  completeAccountOAuth: vi.fn(),
  importAccounts: importAccountsMock,
  startAccountOAuth: vi.fn(),
}))

// BaseToast 的入口会加载 SFC，node 环境必须整体替换。
vi.mock('@/components/base/BaseToast', () => ({
  toast: { success: toastSuccessMock, error: vi.fn(), warning: vi.fn(), info: vi.fn() },
}))

const RISKY_COPY = '上游执行结果未知，该账号可能已导入。请先刷新账号列表确认，未导入再重试；反复重试可能使该 Refresh Token 永久失效'
const SAFE_CONFLICT_COPY = '上游执行结果未知，账号可能已导入；重试是安全的（同账号会更新，不会重复创建）'

function setup(
  tokens = 'rt.token-under-test',
  options: {
    accounts?: Record<string, { email: string | null, name: string }>
    reload?: () => Promise<unknown>
  } = {},
) {
  const onboarding = useAccountOnboarding({
    reload: options.reload || vi.fn(async () => {}),
    accountById: accountId => options.accounts?.[accountId],
  })
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
    toastSuccessMock.mockReset()
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

describe('sub2api 导出解析', () => {
  it('CLIProxyAPI codex 单文件可直接识别，proxy_url 归一化且展示身份', () => {
    const rows = parseOpenAiTokenRows(JSON.stringify({
      type: 'codex', access_token: 'at-codex', refresh_token: 'rt-codex', id_token: 'e30.eyJzdWIiOiIxIn0.sig',
      email: 'codex@example.test', account_id: 'acct-codex', proxy_url: 'http://proxy.test:8080',
    }))
    expect(rows[0]).toMatchObject({ kind: 'at', status: 'pending', email: 'codex@example.test', accountId: 'acct-codex', proxyUrl: 'http://proxy.test:8080' })
    expect(rows[0].entry).toMatchObject({ outbound_proxy_url: 'http://proxy.test:8080' })
  })

  it('导入 CLIProxyAPI 行时发送 outbound_proxy_url', async () => {
    importAccountsMock.mockResolvedValue(successResponse())
    const onboarding = setup(JSON.stringify({ type: 'codex', access_token: 'at-proxy', proxy_url: 'socks5://proxy.test:1080' }))
    await onboarding.handleCreate()
    expect(importAccountsMock).toHaveBeenCalledWith(expect.objectContaining({
      data: { accounts: [expect.objectContaining({ outbound_proxy_url: 'socks5://proxy.test:1080' })] },
    }))
  })

  it('CLIProxyAPI gemini/claude/antigravity 类型被拦截', () => {
    const rows = parseOpenAiTokenRows(JSON.stringify([
      { type: 'gemini', access_token: 'at-gemini' },
      { type: 'claude', access_token: 'at-claude' },
      { type: 'antigravity', access_token: 'at-antigravity' },
    ]))
    expect(rows.map(row => row.error)).toEqual([
      'CLIProxyAPI gemini 凭据，无法导入',
      'CLIProxyAPI claude 凭据，无法导入',
      'CLIProxyAPI antigravity 凭据，无法导入',
    ])
    expect(rows.every(row => row.kind === 'unsupported' && row.status === 'needs_action')).toBe(true)
  })

  it('连续裸对象与数组均可拆分为多行', () => {
    const objectText = '{"type":"codex","access_token":"at-one"}\n{"type":"codex","access_token":"at-two"}'
    expect(parseOpenAiTokenRows(objectText).map(row => row.credential)).toEqual(['at-one', 'at-two'])
    expect(parseOpenAiTokenRows('[{"type":"codex","access_token":"at-three"},{"type":"codex","access_token":"at-four"}]')).toHaveLength(2)
  })

  it('从 credentials 提取 OAuth 凭据，保留 name/email 与匹配的 proxy', () => {
    const rows = parseOpenAiTokenRows(JSON.stringify({
      type: 'sub2api-data',
      proxies: [{ proxy_key: 'proxy-a', protocol: 'socks5', host: 'proxy.test', port: 1080 }],
      accounts: [{
        name: '账号一',
        platform: 'openai',
        proxy_key: 'proxy-a',
        credentials: { access_token: 'at-sub2api', email: 'one@example.test' },
      }],
    }))

    expect(rows).toHaveLength(1)
    expect(rows[0]).toMatchObject({
      credential: 'at-sub2api',
      kind: 'at',
      name: '账号一',
      email: 'one@example.test',
      status: 'pending',
      proxies: [{ proxy_key: 'proxy-a' }],
    })
  })

  it('标记非 OpenAI、API Key、缺凭据与缺代理的行并给出原因', () => {
    const rows = parseOpenAiTokenRows(JSON.stringify({
      proxies: [],
      accounts: [
        { platform: 'anthropic', credentials: { access_token: 'ignored' } },
        { platform: 'openai', credentials: { api_key: 'sk-test' } },
        { platform: 'openai', name: 'empty' },
        { platform: 'openai', proxy_key: 'missing', credentials: { refresh_token: 'rt-sub2api' } },
      ],
    }))

    expect(rows.map(row => row.error)).toEqual([
      '非 OpenAI 平台，无法导入',
      'API Key 不是 OAuth 凭据',
      '既无 Access Token 也无 Refresh Token',
      '找不到 proxy_key 对应的代理，无法导入',
    ])
    expect(rows.every(row => row.status === 'needs_action' && row.kind === 'unsupported')).toBe(true)
  })

  it('保持 200 行截断与重复凭据判定', () => {
    const entries = Array.from({ length: 201 }, (_, index) => ({ access_token: `at-${index}` }))
    const rows = parseOpenAiTokenRows(JSON.stringify({ accounts: [...entries, { access_token: 'at-0' }] }))
    expect(rows).toHaveLength(200)
    expect(rows[0].status).toBe('pending')
    expect(rows.filter(row => row.status === 'duplicate')).toHaveLength(0)
  })
})

describe('手动重试入口', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    importAccountsMock.mockReset()
    toastSuccessMock.mockReset()
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

describe('逐行导入结果与汇总', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    importAccountsMock.mockReset()
    toastSuccessMock.mockReset()
  })

  afterEach(() => {
    vi.useRealTimers()
  })

  it('createdCount/updatedCount 映射为新增/更新，reload 后按返回 accountIds 回显账号', async () => {
    importAccountsMock
      .mockResolvedValueOnce({ ...successResponse(), createdCount: 1, accountIds: ['created-account'] })
      .mockResolvedValueOnce({ ...successResponse(), updatedCount: 1, accountIds: ['existing-account'] })
    const accounts: Record<string, { email: string | null, name: string }> = {}
    const reload = vi.fn(async () => {
      Object.assign(accounts, {
        'created-account': { email: 'created@example.com', name: '新账号' },
        'existing-account': { email: null, name: '已有账号' },
      })
    })
    const onboarding = setup('rt.created-token\nrt.updated-token', {
      reload,
      accounts,
    })
    const rows = [...onboarding.tokenRows.value]

    await runImport(onboarding.handleCreate())

    expect(reload).toHaveBeenCalledTimes(1)
    expect(rows.map(row => ({ outcome: row.outcome, accountId: row.accountId, result: row.result }))).toEqual([
      { outcome: 'created', accountId: 'created-account', result: '新增 · created@example.com' },
      { outcome: 'updated', accountId: 'existing-account', result: '更新 · 已有账号' },
    ])
    expect(tokenImportSummary(rows)).toMatchObject({ success: 2, created: 1, updated: 1, imported: 0 })
    expect(toastSuccessMock).toHaveBeenCalledWith('新增 1 个，更新 1 个')
  })

  it('旧后端缺省计数字段时回退为已导入，并与汇总和完成提示同源', async () => {
    importAccountsMock.mockResolvedValueOnce({ ...successResponse(), accountIds: ['legacy-account'] })
    const onboarding = setup('rt.legacy-token', {
      accounts: { 'legacy-account': { email: 'legacy@example.com', name: '旧账号' } },
    })
    const row = onboarding.tokenRows.value[0]

    await runImport(onboarding.handleCreate())

    expect(row.outcome).toBe('imported')
    expect(row.result).toBe('已导入 · legacy@example.com')
    expect(tokenImportSummary([row])).toMatchObject({ success: 1, created: 0, updated: 0, imported: 1 })
    expect(toastSuccessMock).toHaveBeenCalledWith('已导入 1 个')
  })

  it('仅更新时完成提示仍明确显示新增 0 个、更新 1 个', async () => {
    importAccountsMock.mockResolvedValueOnce({ ...successResponse(), updatedCount: 1 })
    const onboarding = setup()

    await runImport(onboarding.handleCreate())

    expect(toastSuccessMock).toHaveBeenCalledWith('新增 0 个，更新 1 个')
  })

  it('200 响应含 failures 时优先判失败，不读取成功计数', async () => {
    importAccountsMock.mockResolvedValueOnce({
      ...successResponse(),
      createdCount: 1,
      failures: [{ index: 0, code: 'refresh_rejected', retryable: false, message: '令牌已失效' }],
    })
    const onboarding = setup()

    await runImport(onboarding.handleCreate())

    expect(onboarding.tokenRows.value[0]).toMatchObject({
      status: 'needs_action',
      outcome: null,
      result: '',
      error: '令牌已失效',
    })
  })
})

describe('导入行边界', () => {
  it('最多保留 200 行', () => {
    const tokens = Array.from({ length: 201 }, (_, index) => `rt.token-${index}`).join('\n')

    expect(parseOpenAiTokenRows(tokens)).toHaveLength(200)
  })

  it('重复凭据保留展示但标记为重复', () => {
    const rows = parseOpenAiTokenRows('rt.same-token\nrt.other-token\nrt.same-token')

    expect(rows.map(row => row.status)).toEqual(['pending', 'pending', 'duplicate'])
    expect(rows[2].error).toBe('重复（第 1 行）')
  })
})
