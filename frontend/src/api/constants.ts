export const API_BASE_URL = import.meta.env.DEV ? '/dev' : ''
export const API_TIMEOUT_MS = 30_000
// 导入单行一次 token 交换的后端预算为 30s（TOKEN_REQUEST_TIMEOUT）；前端超时必须大于它，
// 让后端已分类的 42902/50301/50202 抵达前端，而不是被本地 timeout 掩盖后误入风险桶。
export const ACCOUNT_IMPORT_TIMEOUT_MS = 40_000
