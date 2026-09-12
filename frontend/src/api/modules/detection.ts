import request from '../request'

export interface DetectionConfig {
  enabled: boolean
  account_scope: { all: true } | { account_ids: string[] }
  interval_secs: number
  model: string
}

export interface DetectionRound {
  detection_round_id: string
  checked_at: string
  degraded_count: number
  normal_count: number
}

export interface DetectionRecord {
  id: number
  detection_round_id: string
  account_id: string
  account_email?: string | null
  account_name?: string | null
  account_plan_type?: string | null
  account_plan_type_display?: string | null
  checked_at: string
  degraded: boolean
  scheduling_suspended: boolean
  // 触发判定的思考过程正文；0007 迁移前的旧记录不回填，读到 null/缺失属正常。
  reasoning_content?: string | null
}

export function getDetectionConfig() {
  return request<DetectionConfig>({
    url: '/api/admin/detection/config',
    method: 'GET',
  })
}

export function updateDetectionConfig(data: DetectionConfig) {
  return request<DetectionConfig>({
    url: '/api/admin/detection/config',
    method: 'POST',
    data,
  })
}

export function getDetectionRounds() {
  return request<DetectionRound[]>({
    url: '/api/admin/detection/records/rounds',
    method: 'GET',
  })
}

export function getDetectionRecords(roundId: string) {
  return request<DetectionRecord[]>({
    url: '/api/admin/detection/records',
    method: 'GET',
    params: { detection_round_id: roundId },
  })
}

// 管理端路由冻结为静态路径 + 查询参数（架构测试禁止路径参数），与后端契约一致。
export function getDetectionRecordHtmlUrl(id: number) {
  return `/api/admin/detection/records/html?id=${id}`
}
