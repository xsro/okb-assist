import { apiGet, apiPut, apiPost } from './client'
import type {
  ServiceConfig,
  SystemConfig,
  ServiceStatus,
  ConnectionTestResult
} from '@/types/config'

// ── 配置读写 ────────────────────────────────────────────

/** 获取服务配置 */
export function getServiceConfig() {
  return apiGet<ServiceConfig>('/assist/api/config')
}

/** 更新服务配置 */
export function updateServiceConfig(config: ServiceConfig) {
  return apiPut<{ message: string }>('/assist/api/config', config)
}

/** 重载配置 */
export function reloadConfig() {
  return apiPost<{ message: string }>('/assist/api/config/reload')
}

/** 获取系统配置（脱敏） */
export function getSystemConfig() {
  return apiGet<SystemConfig>('/assist/api/config/system')
}

// ── 连接测试 ────────────────────────────────────────────

/** 测试服务连接 */
export function testConnection(service: 'mineru' | 'ollama' | 'qdrant') {
  return apiPost<ConnectionTestResult>(
    '/assist/api/config/test-connection',
    { service }
  )
}

// ── 服务状态 ────────────────────────────────────────────

/** 获取所有服务状态 */
export function getAllServiceStatus() {
  return apiGet<ServiceStatus>('/assist/api/admin/services')
}