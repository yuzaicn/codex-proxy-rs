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
}

export function useAccountOnboarding(options: {
  reload: () => Promise<unknown>
}) {
  const createModalOpen = shallowRef(false)
  const reauthorizingAccount = shallowRef<AccountRow | null>(null)
  const creatingAccountAction = useAsyncAction()
  const authorizingOAuthAction = useAsyncAction()
  const creatingAccount = creatingAccountAction.loading
  const authorizingOAuth = authorizingOAuthAction.loading
  const createForm = ref(emptyAccountCreateForm())
  const tokenRows = ref<TokenImportRow[]>([])

  const showCreateModal = computed({
    get: () => createModalOpen.value,
    set: (value: boolean) => {
      createModalOpen.value = value
      if (!value) {
        reauthorizingAccount.value = null
        createForm.value = emptyAccountCreateForm()
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
          if (tokenRows.value.some(row => row.status === 'failed'))
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
    const failures = rows.filter(row => row.status === 'failed' || row.status === 'needs_action')
    if (failures.length > 0)
      return `已导入 ${rows.filter(row => row.status === 'success').length} 个账号，${failures.length} 行失败`
    showCreateModal.value = false
    return `已导入 ${rows.filter(row => row.status === 'success').length} 个账号`
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
    row.retryAttempt = 0
    for (let attempt = 0; attempt <= 3; attempt++) {
      row.status = 'importing'
      row.retryAttempt = attempt
      try {
        const result = await importAccounts({
          provider: 'openai',
          settings: accountImportSettings(createForm.value),
          outboundProxyId: createForm.value.proxyMode === 'proxy' ? createForm.value.proxyId.trim() : undefined,
          data: { accounts: [row.entry] },
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
          row.result = '已导入'
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
    await runTokenImport([row])
    if (tokenRows.value.find(item => item.id === id)?.status === 'success') {
      await options.reload()
      toast.success('凭据已导入')
    }
  }

  async function retryFailedImports() {
    if (creatingAccount.value)
      return
    await creatingAccountAction.run(async () => {
      await runTokenImport(tokenRows.value.filter(row => row.status === 'failed' || row.status === 'needs_action'))
      await options.reload()
      if (tokenRows.value.every(row => row.status !== 'failed' && row.status !== 'needs_action')) {
        showCreateModal.value = false
        toast.success('失败项已全部导入')
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

  watch(
    () => createForm.value.provider,
    () => {
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
    () => createForm.value.mode === 'auto' ? createForm.value.importTexts.auto : '',
    (value) => { tokenRows.value = parseOpenAiTokenRows(value) },
    { immediate: true, flush: 'sync' },
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
      const seen = new Set<string>()
      tokenRows.value.forEach((row) => {
        if (!row.credential)
          return
        if (seen.has(row.credential)) {
          row.status = 'duplicate'
          row.error = '重复凭据'
        }
        else {
          seen.add(row.credential)
          if (row.status === 'duplicate') {
            row.status = 'pending'
            row.error = ''
          }
        }
      })
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

function isAmbiguousImportError(error: unknown) {
  if (!(error instanceof ApiError))
    return false
  return error.code === 50202 || error.code === 40901 || error.status === 0 || error.status === 408 || error.kind === 'timeout' || error.kind === 'network'
}

function isRetryableImportError(error: unknown) {
  if (!(error instanceof ApiError))
    return false
  return error.status >= 500 || error.status === 409 || error.status === 0 || error.status === 408 || error.kind === 'timeout' || error.kind === 'network'
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
  const trimmed = value.trim()
  if (!trimmed)
    return []
  const parsed = parseJsonEntries(trimmed)
  const entries = parsed.length > 0 ? parsed : value.split(/\r?\n/).map(line => line.trim()).filter(Boolean).map(raw => ({ raw, value: raw }))
  const rows: TokenImportRow[] = []
  entries.slice(0, MAX_TOKEN_IMPORT_COUNT).forEach((item, index) => {
    const obj = isRecord(item.value) ? item.value : null
    const credential = obj ? credentialFromObject(obj) : String(item.value)
    const kind = obj ? credentialKind(obj) : tokenKind(credential)
    rows.push({ id: `${index}-${credential || item.raw}`, index: index + 1, raw: item.raw, credential, kind, status: 'pending', result: '', error: '', retryAttempt: 0, entry: obj || (credential && (kind === 'at' || kind === 'rt') ? { [kind === 'rt' ? 'refreshToken' : 'accessToken']: credential } : null) })
  })
  const seen = new Map<string, number>()
  rows.forEach((row) => {
    if (!row.credential)
      return
    const prior = seen.get(row.credential)
    if (prior !== undefined) {
      row.status = 'duplicate'
      row.error = `重复（第 ${prior} 行）`
    }
    else {
      seen.set(row.credential, row.index)
    }
  })
  return rows
}

function parseJsonEntries(value: string): Array<{ raw: string, value: unknown }> {
  try {
    const parsed = JSON.parse(value)
    return flattenJson(parsed, value)
  }
  catch { /* JSONL and token lines are handled below. */ }
  const entries: Array<{ raw: string, value: unknown }> = []
  let group = ''
  let depth = 0
  for (const line of value.split(/\r?\n/).map(item => item.trim()).filter(Boolean)) {
    const startsJson = line.startsWith('{') || line.startsWith('[')
    if (!group && !startsJson) {
      try {
        entries.push({ raw: line, value: JSON.parse(line) })
      }
      catch {
        continue
      }
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

function flattenJson(value: unknown, raw: string): Array<{ raw: string, value: unknown }> {
  if (Array.isArray(value))
    return value.flatMap(item => flattenJson(item, JSON.stringify(item)))
  if (isRecord(value) && Array.isArray(value.accounts))
    return value.accounts.flatMap(item => flattenJson(item, JSON.stringify(item)))
  if (isRecord(value) && Array.isArray(value.documents)) {
    return value.documents.flatMap((item) => {
      if (!isRecord(item) || !('document' in item) || item.provider !== 'openai')
        return [{ raw: JSON.stringify(item), value: { __unsupported: true } }]
      return flattenJson(item.document, JSON.stringify(item.document))
    })
  }
  return [{ raw, value }]
}

function credentialFromObject(value: Record<string, unknown>) {
  for (const key of ['accessToken', 'access_token', 'personal_access_token', 'personalAccessToken', 'refreshToken', 'refresh_token']) {
    if (typeof value[key] === 'string' && value[key].trim())
      return value[key].trim()
  }
  return ''
}

function credentialKind(value: Record<string, unknown>): TokenImportKind {
  const at = ['accessToken', 'access_token', 'personal_access_token', 'personalAccessToken'].some(key => typeof value[key] === 'string' && value[key].trim())
  const rt = ['refreshToken', 'refresh_token'].some(key => typeof value[key] === 'string' && value[key].trim())
  return at ? 'at' : rt ? 'rt' : 'unsupported'
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
