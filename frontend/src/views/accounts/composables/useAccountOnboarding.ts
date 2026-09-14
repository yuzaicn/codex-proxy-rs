import type { getAccounts } from '@/api'

import { computed, ref, shallowRef, watch } from 'vue'
import {
  completeAccountOAuth,
  importAccounts,
  startAccountOAuth,
} from '@/api'
import { ApiError } from '@/api/request'
import { toast } from '@/components/base/BaseToast'
import { useAsyncAction } from '@/composables/useAsyncAction'
import { errorMessage } from '@/utils/async'
import { isRecord } from '@/utils/object'
import { formatProviderLabel, isSupportedProvider } from '@/utils/providers'
import { accountImportSettings, accountProxyError, emptyAccountCreateForm } from '../components/AccountCreateModal/model'

type AccountRow = Awaited<ReturnType<typeof getAccounts>>['items'][number]
type ImportProvider = 'openai' | 'xai'

interface MixedImportDocument {
  provider: ImportProvider
  document: Record<string, unknown>
}

type OpenAiTokenImportMode = 'access_token' | 'refresh_token'

const MAX_TOKEN_IMPORT_COUNT = 200
export const TOKEN_IMPORT_CONCURRENCY = 4

export type TokenImportKind = 'at' | 'rt' | 'unknown' | 'unsupported'
export type TokenImportStatus = 'pending' | 'importing' | 'success' | 'failed' | 'needs_action' | 'duplicate'
export type TokenImportOutcome = 'created' | 'updated' | 'imported'
export interface TokenImportRow {
  id: string
  index: number
  raw: string
  credential: string
  kind: TokenImportKind
  status: TokenImportStatus
  result: string
  error: string
  retryAttempt: number
  entry: Record<string, unknown> | null
  outcome: TokenImportOutcome | null
  name: string
  email: string
  accountId: string
  proxyUrl: string
  proxies: Record<string, unknown>[]
}

// 汇总条与完成提示共用的口径：失败=自动重试耗尽或风险桶（结果未知）单次失败，均可手动重试；还需要你处理=需要处理/未识别/重复。
export function tokenImportSummary(rows: TokenImportRow[]) {
  return {
    pending: rows.filter(row => row.status === 'pending').length,
    success: rows.filter(row => row.status === 'success').length,
    created: rows.filter(row => row.status === 'success' && row.outcome === 'created').length,
    updated: rows.filter(row => row.status === 'success' && row.outcome === 'updated').length,
    imported: rows.filter(row => row.status === 'success' && row.outcome === 'imported').length,
    failed: rows.filter(row => row.status === 'failed').length,
    needsAction: rows.filter(row => row.status === 'needs_action').length,
    actionNeeded: rows.filter(row => row.status === 'needs_action' || row.status === 'duplicate' || row.kind === 'unknown').length,
  }
}

export function useAccountOnboarding(options: {
  reload: () => Promise<unknown>
  accountById?: (accountId: string) => Pick<AccountRow, 'email' | 'name'> | undefined
}) {
  const createModalOpen = shallowRef(false)
  const reauthorizingAccount = shallowRef<AccountRow | null>(null)
  const creatingAccountAction = useAsyncAction()
  const authorizingOAuthAction = useAsyncAction()
  const creatingAccount = creatingAccountAction.loading
  const authorizingOAuth = authorizingOAuthAction.loading
  const createForm = ref(emptyAccountCreateForm())
  const tokenRows = ref<TokenImportRow[]>([])
  const tokenImportNotice = ref('')
  let tokenRowSequence = 0

  const showCreateModal = computed({
    get: () => createModalOpen.value,
    set: (value: boolean) => {
      createModalOpen.value = value
      if (!value) {
        reauthorizingAccount.value = null
        createForm.value = emptyAccountCreateForm()
        tokenRows.value = []
        tokenImportNotice.value = ''
        tokenRowSequence = 0
      }
    },
  })

  async function handleCreate() {
    if (createForm.value.mode === 'oauth') {
      await completeOAuth()
      return
    }
    if (creatingAccount.value)
      return

    if (createForm.value.provider === 'openai' && createForm.value.mode === 'auto') {
      await creatingAccountAction.run(
        async () => {
          const message = await importOpenAiTokenRows()
          if (tokenRows.value.some(row => row.status === 'failed' || row.status === 'needs_action'))
            toast.error(message)
          else
            toast.success(message)
        },
        { errorText: '导入失败' },
      )
      return
    }

    await creatingAccountAction.run(
      async () => {
        const proxyError = accountProxyError(createForm.value)
        if (proxyError)
          throw new Error(proxyError)
        const message = createForm.value.provider === 'batch'
          ? await importMixedAccountDocument()
          : await importAccountDocument()
        await finishCreate(message)
      },
      { errorText: '导入失败' },
    )
  }

  async function handleAuthorizeOAuth() {
    if (authorizingOAuth.value)
      return

    await authorizingOAuthAction.run(
      async () => {
        const input = newAccountInput()
        const account = reauthorizingAccount.value
        const proxyError = accountProxyError(createForm.value)
        if (!account && proxyError)
          throw new Error(proxyError)
        const result = await startAccountOAuth({
          ...input,
          outboundProxyId: !account && createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
          ...(account
            ? {
                accountId: account.id,
              }
            : {}),
        })

        createForm.value = {
          ...createForm.value,
          oauthFlowId: result.flowId,
          oauthAuthUrl: result.authorizationUrl,
          oauthCallback: '',
        }
        toast.success('授权链接已生成')
      },
      { errorText: '授权链接生成失败' },
    )
  }

  async function completeOAuth() {
    if (creatingAccount.value)
      return

    await creatingAccountAction.run(
      async () => {
        if (!createForm.value.oauthFlowId)
          throw new Error('请先生成授权链接')

        const callbackUrl = createForm.value.oauthCallback.trim()
        if (!callbackUrl) {
          throw new Error(createForm.value.provider === 'xai'
            ? '请粘贴 OAuth 回调地址、含 code 和 state 的查询字符串或授权码'
            : '请粘贴 OAuth 回调地址')
        }
        await completeAccountOAuth({
          provider: createForm.value.provider,
          flowId: createForm.value.oauthFlowId,
          callbackUrl,
          settings: reauthorizingAccount.value ? undefined : accountImportSettings(createForm.value),
        })
        await finishCreate(
          reauthorizingAccount.value
            ? '账号重新授权成功'
            : createForm.value.provider === 'xai'
              ? 'xAI OAuth 账号已添加'
              : 'OpenAI OAuth 账号已添加',
        )
      },
      {
        errorText: reauthorizingAccount.value ? '重新授权失败' : 'OAuth 授权导入失败',
      },
    )
  }

  function openCreateAccount() {
    reauthorizingAccount.value = null
    createForm.value = emptyAccountCreateForm()
    tokenRows.value = []
    tokenImportNotice.value = ''
    tokenRowSequence = 0
    showCreateModal.value = true
  }

  function openReauthorizeAccount(account: AccountRow) {
    if (account.provider !== 'openai' && account.provider !== 'xai')
      return
    reauthorizingAccount.value = account
    createForm.value = {
      ...emptyAccountCreateForm(),
      provider: account.provider,
      step: 'import',
      mode: 'oauth',
    }
    showCreateModal.value = true
    void handleAuthorizeOAuth()
  }

  function newAccountInput() {
    const account = reauthorizingAccount.value
    return {
      provider: createForm.value.provider,
      name: account?.name || account?.email || `${createForm.value.provider} OAuth`,
    }
  }

  async function importAccountDocument() {
    const provider = requireImportProvider(createForm.value.provider)
    const mode = createForm.value.mode
    if (mode === 'oauth')
      throw new Error('请选择凭据导入方式')
    if (provider === 'openai' && mode === 'auto')
      return importOpenAiTokenRows()
    const documents = accountImportDocuments(provider, mode, createForm.value.importTexts[mode])
    let importedCount = 0
    for (const entry of documents) {
      const result = await importAccounts({
        provider,
        settings: accountImportSettings(createForm.value),
        outboundProxyId: createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
        data: entry.document,
      })
      importedCount += result.importedCount
    }
    return `${formatProviderLabel(provider)} 账号已导入 ${importedCount} 个`
  }

  async function importOpenAiTokenRows() {
    const rows = tokenRows.value
    if (rows.length === 0)
      throw new Error('请至少粘贴一个凭据')
    const candidates = rows.filter(row => row.status === 'pending' || row.status === 'failed' || row.status === 'needs_action')
      .filter(row => row.entry && row.kind !== 'unknown' && row.kind !== 'unsupported' && row.status !== 'duplicate')
    if (candidates.length === 0)
      throw new Error('没有可导入的凭据')
    await runTokenImport(candidates)
    await options.reload()
    hydrateTokenImportResults(candidates)
    const summary = tokenImportSummary(rows)
    if (summary.failed > 0 || summary.needsAction > 0) {
      const parts = [tokenImportSuccessMessage(summary)]
      if (summary.failed > 0)
        parts.push(`失败 ${summary.failed} 行`)
      if (summary.actionNeeded > 0)
        parts.push(`还需要你处理 ${summary.actionNeeded} 行`)
      return parts.join('，')
    }
    showCreateModal.value = false
    return tokenImportSuccessMessage(summary)
  }

  function appendTokenText(value: string) {
    if (createForm.value.provider !== 'openai' || createForm.value.mode !== 'auto')
      return
    const entries = parseOpenAiTokenEntries(value)
    if (entries.length === 0)
      return

    const available = MAX_TOKEN_IMPORT_COUNT - tokenRows.value.length
    if (available <= 0) {
      tokenImportNotice.value = `最多保留 ${MAX_TOKEN_IMPORT_COUNT} 行，未加入本次内容`
      return
    }

    const accepted = entries.slice(0, available)
    if (accepted.length < entries.length) {
      tokenImportNotice.value = `最多保留 ${MAX_TOKEN_IMPORT_COUNT} 行，本次仅加入 ${accepted.length} 行`
    }
    else {
      tokenImportNotice.value = ''
    }

    const startIndex = tokenRows.value.length
    tokenRows.value.push(...accepted.map((item, offset) => createTokenImportRow(
      item,
      startIndex + offset + 1,
      `token-row-${++tokenRowSequence}`,
    )))
    reconcileTokenRows()
  }

  function reconcileTokenRows() {
    const seen = new Map<string, number>()
    tokenRows.value.forEach((row, offset) => {
      row.index = offset + 1
      if (!row.credential)
        return
      const prior = seen.get(row.credential)
      if (prior !== undefined) {
        if (row.status !== 'success' && row.status !== 'importing') {
          row.status = 'duplicate'
          row.error = `重复（第 ${prior} 行）`
        }
      }
      else {
        seen.set(row.credential, row.index)
        if (row.status === 'duplicate') {
          row.status = 'pending'
          row.error = ''
        }
      }
    })
  }

  async function runTokenImport(rows: TokenImportRow[]) {
    let cursor = 0
    async function worker() {
      while (cursor < rows.length) {
        const row = rows[cursor++]
        await importTokenRow(row)
      }
    }
    await Promise.all(Array.from({ length: Math.min(TOKEN_IMPORT_CONCURRENCY, rows.length) }, () => worker()))
  }

  async function importTokenRow(row: TokenImportRow) {
    row.error = ''
    row.result = ''
    row.outcome = null
    row.accountId = ''
    row.retryAttempt = 0
    for (let attempt = 0; attempt <= 3; attempt++) {
      row.status = 'importing'
      row.retryAttempt = attempt
      try {
        const data = row.proxies.length > 0
          ? { accounts: [row.entry], proxies: row.proxies }
          : { accounts: [row.entry] }
        const result = await importAccounts({
          provider: 'openai',
          settings: accountImportSettings(createForm.value),
          outboundProxyId: createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
          data,
        })
        const failure = importResponseFailure(result)
        if (failure) {
          if (failure.retryable && attempt < 3) {
            await wait(1000 * 2 ** attempt)
            continue
          }
          row.status = failure.retryable ? 'failed' : 'needs_action'
          row.error = failure.message
        }
        else if (result.importedCount > 0) {
          row.status = 'success'
          row.outcome = importOutcome(result)
          row.accountId = result.accountIds[0] || ''
          row.result = tokenImportOutcomeLabel(row.outcome)
        }
        else {
          row.status = 'failed'
          row.error = '导入失败'
        }
        return
      }
      catch (error) {
        if (!isRetryableImportError(error) || attempt === 3) {
          row.status = isNeedsActionImportError(error) ? 'needs_action' : 'failed'
          row.error = isNeedsActionImportError(error)
            ? '凭据无效或已失效，请检查后更换'
            : importFailureMessage(error)
          return
        }
        await wait(1000 * 2 ** attempt)
      }
    }
  }

  async function retryImportRow(id: string) {
    const row = tokenRows.value.find(item => item.id === id)
    if (!row || !['failed', 'needs_action'].includes(row.status) || creatingAccount.value)
      return
    await creatingAccountAction.run(async () => {
      await runTokenImport([row])
      if (row.status === 'success') {
        await options.reload()
        hydrateTokenImportResults([row])
        toast.success(tokenImportSuccessMessage(tokenImportSummary([row])))
      }
    }, { errorText: '重试失败' })
  }

  async function retryFailedImports() {
    if (creatingAccount.value)
      return
    await creatingAccountAction.run(async () => {
      await runTokenImport(tokenRows.value.filter(row => row.status === 'failed' || row.status === 'needs_action'))
      await options.reload()
      hydrateTokenImportResults(tokenRows.value)
      if (tokenRows.value.every(row => row.status !== 'failed' && row.status !== 'needs_action')) {
        const message = tokenImportSuccessMessage(tokenImportSummary(tokenRows.value))
        showCreateModal.value = false
        toast.success(message)
      }
    }, { errorText: '重试失败项失败' })
  }

  async function copyFailedImports() {
    const text = tokenRows.value.filter(row => row.status === 'failed' || row.status === 'needs_action').map(row => row.raw).join('\n')
    try {
      await navigator.clipboard.writeText(text)
      toast.success('失败清单已复制')
    }
    catch {
      toast.error('复制失败清单失败')
    }
  }

  async function importMixedAccountDocument() {
    const documents = parseMixedImportDocuments(parseImportJson(createForm.value.importTexts.json))
    let importedCount = 0
    const failures: string[] = []

    for (const entry of documents) {
      try {
        const result = await importAccounts({
          provider: entry.provider,
          settings: accountImportSettings(createForm.value),
          outboundProxyId: createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
          data: entry.document,
        })
        importedCount += result.importedCount
      }
      catch (error) {
        const message = errorMessage(error, '导入失败')
        failures.push(`${formatProviderLabel(entry.provider, 'OpenAI')}：${message}`)
      }
    }

    if (importedCount === 0) {
      throw new Error(failures.length > 0 ? `批量导入失败：${failures.join('、')}` : '批量文件没有可导入的账号')
    }

    if (failures.length > 0) {
      return `已导入 ${importedCount} 个账号，${failures.length} 个文档失败：${failures.join('、')}`
    }
    return `已导入 ${importedCount} 个账号`
  }

  async function finishCreate(message: string) {
    showCreateModal.value = false
    await options.reload()
    toast.success(message)
  }

  function hydrateTokenImportResults(rows: TokenImportRow[]) {
    for (const row of rows) {
      if (row.status !== 'success' || !row.outcome)
        continue
      const account = row.accountId ? options.accountById?.(row.accountId) : undefined
      const identity = account?.email?.trim() || account?.name.trim()
      const outcome = tokenImportOutcomeLabel(row.outcome)
      row.result = identity ? `${outcome} · ${identity}` : outcome
    }
  }

  watch(
    () => createForm.value.provider,
    () => {
      tokenRows.value = []
      tokenImportNotice.value = ''
      tokenRowSequence = 0
      createForm.value = {
        ...createForm.value,
        mode: createForm.value.provider === 'batch' ? 'json' : 'oauth',
        importTexts: { auto: '', access_token: '', refresh_token: '', json: '' },
        oauthFlowId: '',
        oauthAuthUrl: '',
        oauthCallback: '',
      }
    },
    { flush: 'sync' },
  )

  watch(
    () => createForm.value.mode,
    (mode) => {
      if (mode !== 'auto') {
        tokenRows.value = []
        tokenImportNotice.value = ''
        tokenRowSequence = 0
      }
    },
    { flush: 'sync' },
  )

  watch(
    [
      () => createForm.value.proxyMode,
      () => createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : '',
    ],
    () => {
      createForm.value.oauthFlowId = ''
      createForm.value.oauthAuthUrl = ''
      createForm.value.oauthCallback = ''
    },
    { flush: 'sync' },
  )

  return {
    showCreateModal,
    reauthorizingAccount,
    creatingAccount,
    authorizingOAuth,
    createForm,
    tokenRows,
    tokenImportNotice,
    appendTokenText,
    handleCreate,
    handleAuthorizeOAuth,
    openCreateAccount,
    openReauthorizeAccount,
    retryImportRow,
    retryFailedImports,
    copyFailedImports,
    setTokenRowKind: (id: string, kind: TokenImportKind) => {
      const row = tokenRows.value.find(item => item.id === id)
      if (row && row.status !== 'success' && row.status !== 'importing') {
        row.kind = kind
        row.entry = row.credential && (kind === 'at' || kind === 'rt') ? { [kind === 'rt' ? 'refreshToken' : 'accessToken']: row.credential } : null
      }
    },
    removeTokenRow: (id: string) => {
      tokenRows.value = tokenRows.value.filter(row => row.id !== id)
      reconcileTokenRows()
    },
    setUnknownKind: (kind: TokenImportKind) => {
      tokenRows.value.forEach((row) => {
        if (row.kind === 'unknown') {
          row.kind = kind
          row.entry = row.credential && (kind === 'at' || kind === 'rt') ? { [kind === 'rt' ? 'refreshToken' : 'accessToken']: row.credential } : null
        }
      })
    },
  }
}

function importFailureMessage(error: unknown) {
  if (isRiskyAmbiguousImportError(error))
    return '上游执行结果未知，该账号可能已导入。请先刷新账号列表确认，未导入再重试；反复重试可能使该 Refresh Token 永久失效'
  if (isAmbiguousImportError(error))
    return '上游执行结果未知，账号可能已导入；重试是安全的（同账号会更新，不会重复创建）'
  return errorMessage(error, '导入失败')
}

function importResponseFailure(result: { failures?: Array<{ code: string, retryable: boolean, message: string }> }) {
  const failure = result.failures?.[0]
  if (!failure)
    return null
  return failure
}

function importOutcome(result: { createdCount?: number, updatedCount?: number }): TokenImportOutcome {
  if (result.createdCount === 1)
    return 'created'
  if (result.updatedCount === 1)
    return 'updated'
  return 'imported'
}

function tokenImportOutcomeLabel(outcome: TokenImportOutcome) {
  if (outcome === 'created')
    return '新增'
  if (outcome === 'updated')
    return '更新'
  return '已导入'
}

function tokenImportSuccessMessage(summary: ReturnType<typeof tokenImportSummary>) {
  const parts: string[] = []
  if (summary.created > 0 || summary.updated > 0) {
    parts.push(`新增 ${summary.created} 个`)
    parts.push(`更新 ${summary.updated} 个`)
  }
  if (summary.imported > 0)
    parts.push(`已导入 ${summary.imported} 个`)
  return parts.length > 0 ? parts.join('，') : `已导入 ${summary.success} 个`
}

// 风险桶：真 ambiguous —— 上游可能已消费该凭据但结果未送达（50202），或本地拿不到请求结果
// （超时 / 断网 / 408 / status 0）。自动重放同一 Refresh Token 会撞 refresh_token_reused 导致永久失效，
// 因此这类错误只允许用户手动重试。40901 不在此桶：conflict 语义下同账号更新基本成立。
function isRiskyAmbiguousImportError(error: unknown) {
  if (!(error instanceof ApiError))
    return false
  return error.code === 50202 || error.status === 0 || error.status === 408 || error.kind === 'timeout' || error.kind === 'network'
}

function isAmbiguousImportError(error: unknown) {
  if (!(error instanceof ApiError))
    return false
  return error.code === 40901 || isRiskyAmbiguousImportError(error)
}

// 自动重试只保留「上游明确拒绝且重放安全」的失败：(5xx 且非 50202) 或 409；风险桶一律单次即失败。
function isRetryableImportError(error: unknown) {
  if (!(error instanceof ApiError))
    return false
  if (isRiskyAmbiguousImportError(error))
    return false
  return error.status >= 500 || error.status === 409
}

function isNeedsActionImportError(error: unknown) {
  return error instanceof ApiError && (error.status === 400 || error.status === 404)
}

function wait(ms: number) {
  return new Promise(resolve => setTimeout(resolve, ms))
}

export function maskToken(value: string) {
  if (value.length <= 14)
    return `${value.slice(0, 4)}…${value.slice(-3)}`
  return `${value.slice(0, 8)}…${value.slice(-6)}`
}

export function parseOpenAiTokenRows(value: string): TokenImportRow[] {
  return parseOpenAiTokenEntries(value)
    .slice(0, MAX_TOKEN_IMPORT_COUNT)
    .map((item, index) => createTokenImportRow(item, index + 1, `token-row-${index + 1}`))
    .map((row, _index, rows) => {
      const prior = rows.find(candidate => candidate.credential && candidate.credential === row.credential && candidate.index < row.index)
      if (prior) {
        row.status = 'duplicate'
        row.error = `重复（第 ${prior.index} 行）`
      }
      return row
    })
}

function parseOpenAiTokenEntries(value: string): Array<{ raw: string, value: unknown }> {
  const trimmed = value.trim()
  if (!trimmed)
    return []
  const parsed = parseJsonEntries(trimmed)
  return parsed.length > 0
    ? parsed
    : value.split(/\r?\n/).map(line => line.trim()).filter(Boolean).map(raw => ({ raw, value: raw }))
}

interface ParsedTokenEntry { raw: string, value: unknown, proxies?: Record<string, unknown>[] }

function createTokenImportRow(
  item: ParsedTokenEntry,
  index: number,
  id: string,
): TokenImportRow {
  const obj = isRecord(item.value) ? item.value : null
  const normalized = obj ? normalizeImportEntry(obj) : null
  const credential = normalized ? credentialFromObject(normalized) : String(item.value)
  let kind = normalized ? credentialKind(normalized) : tokenKind(credential)
  let reason = normalized ? importabilityReason(normalized, credential, kind) : ''
  if (normalized && typeof normalized.proxy_key === 'string' && normalized.proxy_key.trim() && (!item.proxies?.length || !item.proxies[0].proxy_key)) {
    kind = 'unsupported'
    reason = '找不到 proxy_key 对应的代理，无法导入'
  }
  if (normalized && reason && !isOpenAiImportEntry(normalized))
    kind = 'unsupported'
  if (obj && reason === 'API Key 不是 OAuth 凭据')
    kind = 'unsupported'
  return {
    id,
    index,
    raw: item.raw,
    credential,
    kind,
    status: reason ? 'needs_action' : 'pending',
    result: '',
    error: reason,
    retryAttempt: 0,
    entry: normalized || (credential && (kind === 'at' || kind === 'rt')
      ? { [kind === 'rt' ? 'refreshToken' : 'accessToken']: credential }
      : null),
    outcome: null,
    name: normalized ? stringField(normalized, ['name', 'label']) : '',
    email: normalized ? stringField(normalized, ['email', 'userEmail']) || nestedStringField(normalized, ['credentials'], ['email', 'userEmail']) : '',
    accountId: normalized ? stringField(normalized, ['account_id', 'accountId']) : '',
    proxyUrl: normalized ? stringField(normalized, ['outbound_proxy_url', 'outboundProxyUrl', 'proxy_url']) : '',
    proxies: item.proxies || [],
  }
}

function parseJsonEntries(value: string): ParsedTokenEntry[] {
  try {
    const parsed = JSON.parse(value)
    return flattenJson(parsed, value)
  }
  catch { /* JSONL and token lines are handled below. */ }
  const entries: ParsedTokenEntry[] = []
  let group = ''
  let depth = 0
  for (const line of value.split(/\r?\n/).map(item => item.trim()).filter(Boolean)) {
    const startsJson = line.startsWith('{') || line.startsWith('[')
    if (!group && !startsJson) {
      entries.push({ raw: line, value: line })
      continue
    }
    group = group ? `${group}\n${line}` : line
    depth += [...line].filter(char => char === '{' || char === '[').length
    depth -= [...line].filter(char => char === '}' || char === ']').length
    if (depth <= 0) {
      try {
        entries.push(...flattenJson(JSON.parse(group), group))
      }
      catch {
        entries.push({ raw: group, value: { __unsupported: true } })
      }
      group = ''
      depth = 0
    }
  }
  if (group)
    entries.push({ raw: group, value: { __unsupported: true } })
  return entries
}

function flattenJson(value: unknown, raw: string, inheritedProxies: Record<string, unknown>[] = []): ParsedTokenEntry[] {
  if (Array.isArray(value))
    return value.flatMap(item => flattenJson(item, JSON.stringify(item), inheritedProxies))
  if (isRecord(value) && Array.isArray(value.accounts)) {
    const proxies = Array.isArray(value.proxies) ? value.proxies.filter(isRecord) : inheritedProxies
    return value.accounts.flatMap(item => {
      if (!isRecord(item))
        return flattenJson(item, JSON.stringify(item), proxies)
      const key = typeof item.proxy_key === 'string' ? item.proxy_key : ''
      const proxy = key ? proxies.find(candidate => candidate.proxy_key === key) : undefined
      return flattenJson(item, JSON.stringify(item), key ? (proxy ? [proxy] : [{}]) : [])
    })
  }
  if (isRecord(value) && Array.isArray(value.documents)) {
    return value.documents.flatMap((item) => {
      if (!isRecord(item) || !('document' in item) || item.provider !== 'openai')
        return [{ raw: JSON.stringify(item), value: { __unsupported: true } }]
      return flattenJson(item.document, JSON.stringify(item.document), [])
    })
  }
  return [{ raw, value, proxies: inheritedProxies }]
}

function credentialFromObject(value: Record<string, unknown>) {
  for (const key of ['accessToken', 'access_token', 'personal_access_token', 'personalAccessToken', 'refreshToken', 'refresh_token']) {
    if (typeof value[key] === 'string' && value[key].trim())
      return value[key].trim()
  }
  const credentials = isRecord(value.credentials) ? value.credentials : null
  if (credentials) {
    for (const key of ['accessToken', 'access_token', 'personal_access_token', 'personalAccessToken', 'refreshToken', 'refresh_token']) {
      if (typeof credentials[key] === 'string' && credentials[key].trim())
        return credentials[key].trim()
    }
  }
  return ''
}

function credentialKind(value: Record<string, unknown>): TokenImportKind {
  const at = ['accessToken', 'access_token', 'personal_access_token', 'personalAccessToken'].some(key => typeof value[key] === 'string' && value[key].trim())
  const rt = ['refreshToken', 'refresh_token'].some(key => typeof value[key] === 'string' && value[key].trim())
  const credentials = isRecord(value.credentials) ? value.credentials : null
  const nestedAt = credentials && ['accessToken', 'access_token', 'personal_access_token', 'personalAccessToken'].some(key => typeof credentials[key] === 'string' && credentials[key].trim())
  const nestedRt = credentials && ['refreshToken', 'refresh_token'].some(key => typeof credentials[key] === 'string' && credentials[key].trim())
  return at || nestedAt ? 'at' : rt || nestedRt ? 'rt' : 'unsupported'
}

function stringField(value: Record<string, unknown>, keys: string[]) {
  for (const key of keys) {
    if (typeof value[key] === 'string' && value[key].trim())
      return value[key].trim()
  }
  return ''
}

function nestedStringField(value: Record<string, unknown>, parents: string[], keys: string[]) {
  const nested = parents.reduce<unknown>((current, key) => isRecord(current) ? current[key] : undefined, value)
  return isRecord(nested) ? stringField(nested, keys) : ''
}

function importabilityReason(value: Record<string, unknown>, credential: string, kind: TokenImportKind) {
  if (!isOpenAiImportEntry(value)) {
    const type = stringField(value, ['type']).toLowerCase()
    if (type)
      return `CLIProxyAPI ${type} 凭据，无法导入`
    return '非 OpenAI 平台，无法导入'
  }
  const hasApiKey = Boolean(stringField(value, ['apiKey', 'api_key', 'key']) || (isRecord(value.credentials) && stringField(value.credentials, ['apiKey', 'api_key', 'key'])))
  if (hasApiKey && !credential)
    return 'API Key 不是 OAuth 凭据'
  if (!credential || kind === 'unsupported')
    return '既无 Access Token 也无 Refresh Token'
  return ''
}

function isOpenAiImportEntry(value: Record<string, unknown>) {
  const platform = stringField(value, ['platform', 'provider']).toLowerCase()
  if (platform)
    return platform === 'openai' || platform === 'codex'
  const type = stringField(value, ['type']).toLowerCase()
  return !type || type === 'openai' || type === 'codex' || type === 'oauth'
}

function normalizeImportEntry(value: Record<string, unknown>) {
  if (typeof value.proxy_url !== 'string' || !value.proxy_url.trim() || value.outbound_proxy_url !== undefined || value.outboundProxyUrl !== undefined)
    return value
  return { ...value, outbound_proxy_url: value.proxy_url }
}

function tokenKind(value: string): TokenImportKind {
  if (value.startsWith('at-') || value.startsWith('eyJ'))
    return 'at'
  if (value.startsWith('rt.'))
    return 'rt'
  return 'unknown'
}

function parseImportJson(value: string) {
  try {
    return JSON.parse(value)
  }
  catch {
    throw new Error('JSON 格式不正确')
  }
}

function requireImportProvider(value: string): ImportProvider {
  if (isSupportedProvider(value))
    return value
  throw new Error('请选择要导入的账号平台')
}

function accountImportDocuments(
  provider: ImportProvider,
  mode: string,
  value: string,
): MixedImportDocument[] {
  if (provider === 'openai' && isOpenAiTokenImportMode(mode)) {
    return [{
      provider,
      document: parseOpenAiTokenImport(value, mode),
    }]
  }
  return providerImportDocuments(parseImportJson(value), provider)
}

function parseOpenAiTokenImport(value: string, mode: OpenAiTokenImportMode) {
  const tokens = value
    .split(/\r?\n/)
    .map(token => token.trim())
    .filter(Boolean)
  const label = mode === 'access_token' ? 'Access Token' : 'Refresh Token'

  if (tokens.length === 0)
    throw new Error(`请至少粘贴一个 ${label}`)
  if (tokens.length > MAX_TOKEN_IMPORT_COUNT)
    throw new Error(`单次最多导入 ${MAX_TOKEN_IMPORT_COUNT} 个 ${label}`)

  const credentialKey = mode === 'access_token' ? 'accessToken' : 'refreshToken'
  return {
    accounts: tokens.map(token => ({ [credentialKey]: token })),
  }
}

function isOpenAiTokenImportMode(value: string): value is OpenAiTokenImportMode {
  return value === 'access_token' || value === 'refresh_token'
}

function providerImportDocuments(value: unknown, provider: ImportProvider): MixedImportDocument[] {
  if (isRecord(value) && Array.isArray(value.documents)) {
    const documents = parseMixedImportDocuments(value)
      .filter(entry => entry.provider === provider)
    if (documents.length === 0) {
      const label = formatProviderLabel(provider)
      throw new Error(`批量导入文件不包含 ${label} 账号文档`)
    }
    return documents
  }
  if (!isRecord(value))
    throw new Error('导入文件必须是 JSON object')
  return [{ provider, document: value }]
}

function parseMixedImportDocuments(value: unknown): MixedImportDocument[] {
  if (!isRecord(value) || !Array.isArray(value.documents))
    throw new Error('批量导入文件必须是 CPR 多平台导出文件')

  const documents: MixedImportDocument[] = []
  for (const entry of value.documents) {
    if (!isRecord(entry))
      throw new Error('批量导入文件包含无效的 Provider 文档')
    const provider = entry.provider
    if (!isSupportedProvider(provider))
      throw new Error('批量导入文件包含无效的 Provider 文档')
    if (!isRecord(entry.document))
      throw new Error('批量导入文件包含无效的 Provider 文档')
    documents.push({ provider, document: entry.document })
  }

  if (documents.length === 0)
    throw new Error('批量文件没有可导入的账号文档')
  return documents
}
