import request from '../request'

export interface DetectionAccountScope {
  all?: boolean
  account_ids?: string[]
}

export interface DetectionConfig {
  enabled: boolean
  account_scope: DetectionAccountScope
  interval_secs: number
  model: string
}

export interface DetectionRound {
  id: string
  started_at?: string
  completed_at?: string | null
  status?: string
  [key: string]: unknown
}

export interface DetectionRecord {
  id: number
  detection_round_id?: string
  account_id?: string
  status?: string
  [key: string]: unknown
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

export function getDetectionRecordHtmlUrl(id: number): string {
  return `/api/admin/detection/records/${id}/html`
}
