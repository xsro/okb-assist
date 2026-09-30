<template>
  <div class="log-viewer">
    <div class="log-header">
      <h2>日志查看</h2>
      <div class="version-info" v-if="versionInfo">
        <span class="version-badge">{{ versionInfo.name }} v{{ versionInfo.version }}</span>
        <span class="version-detail">{{ versionInfo.build_profile }} · {{ versionInfo.rustc }}</span>
        <span class="version-date">构建于 {{ versionInfo.build_time }}</span>
      </div>
      <span class="log-file" v-if="logFile">📄 {{ logFile }}</span>
    </div>

    <!-- 过滤栏 -->
    <div class="log-filters">
      <div class="filter-row">
        <select v-model="levelFilter" class="filter-select" @change="loadLogs">
          <option value="">所有级别</option>
          <option value="trace">TRACE</option>
          <option value="debug">DEBUG</option>
          <option value="info">INFO</option>
          <option value="warn">WARN</option>
          <option value="error">ERROR</option>
        </select>
        <input
          v-model="keywordFilter"
          type="text"
          class="filter-input"
          placeholder="搜索日志内容..."
          @keyup.enter="loadLogs"
        />
        <select v-model="lineCount" class="filter-select filter-lines" @change="loadLogs">
          <option :value="50">50 条</option>
          <option :value="100">100 条</option>
          <option :value="200">200 条</option>
          <option :value="500">500 条</option>
          <option :value="1000">1000 条</option>
        </select>
        <button class="btn btn-outline" @click="loadLogs" :disabled="loading">
          {{ loading ? '加载中...' : '刷新' }}
        </button>
        <button class="btn btn-outline" @click="autoRefresh = !autoRefresh">
          {{ autoRefresh ? '⏸ 暂停' : '▶ 自动' }}
        </button>
        <span class="log-count" v-if="totalLogs > 0">
          共 {{ totalLogs }} 条，显示 {{ displayedLogs }} 条
        </span>
      </div>
    </div>

    <!-- 日志列表 -->
    <div class="log-list" ref="logListRef">
      <div v-if="loading" class="log-loading">加载中...</div>
      <div v-else-if="logs.length === 0" class="log-empty">
        暂无日志
      </div>
      <div
        v-for="(entry, i) in logs"
        :key="i"
        class="log-entry"
        :class="'level-' + (entry.level || 'info').toLowerCase()"
        @click="toggleExpand(i)"
      >
        <span class="log-level-badge">{{ (entry.level || 'INFO').toUpperCase() }}</span>
        <span class="log-timestamp">{{ formatTimestamp(entry.timestamp) }}</span>
        <span class="log-target" v-if="entry.target">{{ entry.target }}</span>
        <span class="log-message">{{ entry.message }}</span>
        <span class="log-location" v-if="entry.file && entry.line">
          <small>{{ entry.file }}:{{ entry.line }}</small>
        </span>
        <!-- 展开详情（请求信息） -->
        <div v-if="expandedIndex === i && hasRequestFields(entry)" class="log-detail">
          <div class="detail-row" v-if="entry.method && entry.uri">
            <span class="detail-label">请求</span>
            <span class="detail-value">{{ entry.method }} {{ entry.uri }}</span>
          </div>
          <div class="detail-row" v-if="entry.status">
            <span class="detail-label">状态</span>
            <span class="detail-value">{{ entry.status }}</span>
          </div>
          <div class="detail-row" v-if="entry.client_ip">
            <span class="detail-label">来源 IP</span>
            <span class="detail-value">{{ entry.client_ip }}</span>
          </div>
          <div class="detail-row" v-if="entry.x_forwarded_for">
            <span class="detail-label">X-Forwarded-For</span>
            <span class="detail-value">{{ entry.x_forwarded_for }}</span>
          </div>
          <div class="detail-row" v-if="entry.x_real_ip">
            <span class="detail-label">X-Real-IP</span>
            <span class="detail-value">{{ entry.x_real_ip }}</span>
          </div>
          <div class="detail-row" v-if="entry.user_agent">
            <span class="detail-label">User-Agent</span>
            <span class="detail-value detail-user-agent">{{ entry.user_agent }}</span>
          </div>
          <div class="detail-row" v-if="entry.referer">
            <span class="detail-label">Referer</span>
            <span class="detail-value">{{ entry.referer }}</span>
          </div>
          <div class="detail-row" v-if="entry.origin">
            <span class="detail-label">Origin</span>
            <span class="detail-value">{{ entry.origin }}</span>
          </div>
          <div class="detail-row" v-if="entry.content_type">
            <span class="detail-label">Content-Type</span>
            <span class="detail-value">{{ entry.content_type }}</span>
          </div>
          <div class="detail-row" v-if="entry.resp_content_type">
            <span class="detail-label">响应类型</span>
            <span class="detail-value">{{ entry.resp_content_type }}</span>
          </div>
          <div class="detail-row" v-if="entry.resp_content_length">
            <span class="detail-label">响应大小</span>
            <span class="detail-value">{{ entry.resp_content_length }} bytes</span>
          </div>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted } from 'vue'
import { getLogs } from '@/api/admin'
import { useRequireToken } from '@/composables/useRequireToken'
import { apiGet } from '@/api/client'
import type { LogEntry } from '@/api/admin'

interface VersionInfo {
  name: string
  version: string
  build_time: string
  build_profile: string
  rustc: string
  target: string
  backend: string
}

const { requireToken } = useRequireToken()

const logs = ref<LogEntry[]>([])
const loading = ref(false)
const levelFilter = ref('')
const keywordFilter = ref('')
const lineCount = ref(100)
const totalLogs = ref(0)
const displayedLogs = ref(0)
const logFile = ref('')
const autoRefresh = ref(false)
const logListRef = ref<HTMLElement | null>(null)
const versionInfo = ref<VersionInfo | null>(null)
const expandedIndex = ref<number | null>(null)
let refreshTimer: ReturnType<typeof setInterval> | null = null

function toggleExpand(i: number) {
  expandedIndex.value = expandedIndex.value === i ? null : i
}

function hasRequestFields(entry: LogEntry): boolean {
  return !!(entry.client_ip || entry.method || entry.uri || entry.user_agent)
}

function formatTimestamp(ts: string): string {
  if (!ts) return ''
  // JSONL 格式: 2024-01-15T10:30:00.123456Z
  try {
    const d = new Date(ts)
    if (isNaN(d.getTime())) return ts
    return d.toLocaleString('zh-CN', {
      hour12: false,
      year: 'numeric',
      month: '2-digit',
      day: '2-digit',
      hour: '2-digit',
      minute: '2-digit',
      second: '2-digit'
    })
  } catch {
    return ts
  }
}

async function loadLogs() {
  if (!requireToken()) return
  loading.value = true
  try {
    const res = await getLogs({
      lines: lineCount.value,
      level: levelFilter.value || undefined,
      q: keywordFilter.value || undefined
    })
    logs.value = res.entries
    totalLogs.value = res.total
    displayedLogs.value = res.displayed
    logFile.value = res.file
  } catch {
    logs.value = []
    totalLogs.value = 0
    displayedLogs.value = 0
  } finally {
    loading.value = false
  }
}

onMounted(async () => {
  loadLogs()
  // 加载版本信息
  try {
    const info = await apiGet<VersionInfo>('/assist/api/admin/')
    versionInfo.value = info
  } catch { /* ignore */ }
})

onUnmounted(() => {
  if (refreshTimer) {
    clearInterval(refreshTimer)
    refreshTimer = null
  }
})
</script>

<style scoped>
.log-viewer {
  padding: 0;
}

.log-header {
  display: flex;
  align-items: baseline;
  gap: 16px;
  margin-bottom: 16px;
  flex-wrap: wrap;
}

.version-info {
  display: flex;
  align-items: center;
  gap: 8px;
  font-size: 12px;
  flex-wrap: wrap;
}

.version-badge {
  background: var(--primary);
  color: #fff;
  padding: 2px 8px;
  border-radius: 4px;
  font-weight: 600;
  font-size: 11px;
}

.version-detail,
.version-date {
  color: var(--text-secondary);
  font-size: 11px;
}

.log-file {
  font-size: 12px;
  color: var(--text-secondary);
  background: var(--bg);
  padding: 2px 8px;
  border-radius: 4px;
  margin-left: auto;
}

.log-filters {
  margin-bottom: 12px;
}

.filter-row {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: center;
}

.filter-select {
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  background: var(--bg-white);
  color: var(--text);
  outline: none;
  cursor: pointer;
  min-height: 34px;
}

.filter-select:focus {
  border-color: var(--primary);
}

.filter-input {
  flex: 1;
  min-width: 160px;
  padding: 6px 12px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 13px;
  outline: none;
  min-height: 34px;
}

.filter-input:focus {
  border-color: var(--primary);
  box-shadow: 0 0 0 2px rgba(25, 118, 210, 0.12);
}

.filter-lines {
  width: 100px;
}

.log-count {
  font-size: 12px;
  color: var(--text-secondary);
  margin-left: auto;
}

/* ── 日志列表 ──────────────────────────────────────────── */
.log-list {
  background: #1e1e1e;
  border: 1px solid var(--border);
  border-radius: 8px;
  max-height: calc(100vh - 240px);
  overflow-y: auto;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'Consolas', monospace;
  font-size: 12px;
  line-height: 1.7;
}

.log-loading,
.log-empty {
  padding: 24px;
  text-align: center;
  color: #888;
}

.log-entry {
  display: flex;
  gap: 8px;
  padding: 3px 12px;
  border-bottom: 1px solid #2a2a2a;
  align-items: baseline;
  color: #d4d4d4;
  cursor: pointer;
  transition: background 0.1s;
  flex-wrap: wrap;
}

.log-entry:last-child {
  border-bottom: none;
}

.log-entry:hover {
  background: #2a2a2a;
}

/* 展开详情面板 */
.log-detail {
  width: 100%;
  padding: 6px 0 2px 56px;
  font-size: 11px;
  display: flex;
  flex-direction: column;
  gap: 2px;
}

.detail-row {
  display: flex;
  gap: 8px;
  line-height: 1.6;
}

.detail-label {
  color: #569cd6;
  flex-shrink: 0;
  min-width: 100px;
}

.detail-value {
  color: #ce9178;
  word-break: break-all;
}

.detail-user-agent {
  color: #6a9955;
  font-size: 10px;
}

/* 级别着色 */
.log-entry.level-error {
  background: rgba(244, 67, 54, 0.08);
}
.log-entry.level-error:hover {
  background: rgba(244, 67, 54, 0.15);
}
.log-entry.level-warn {
  background: rgba(255, 152, 0, 0.06);
}
.log-entry.level-warn:hover {
  background: rgba(255, 152, 0, 0.12);
}

.log-level-badge {
  flex-shrink: 0;
  width: 48px;
  font-weight: 700;
  font-size: 11px;
  text-align: center;
  padding: 0 4px;
}

.level-error .log-level-badge { color: #f44336; }
.level-warn .log-level-badge { color: #ff9800; }
.level-info .log-level-badge { color: #4fc3f7; }
.level-debug .log-level-badge { color: #81c784; }
.level-trace .log-level-badge { color: #888; }

.log-timestamp {
  flex-shrink: 0;
  color: #888;
  font-size: 11px;
  min-width: 150px;
}

.log-target {
  flex-shrink: 0;
  color: #6a9955;
  font-size: 11px;
  max-width: 200px;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.log-message {
  flex: 1;
  word-break: break-word;
  white-space: pre-wrap;
}

.log-location {
  flex-shrink: 0;
  color: #569cd6;
  font-size: 11px;
  margin-left: auto;
}

@media (max-width: 768px) {
  .filter-row {
    flex-direction: column;
    align-items: stretch;
  }
  .filter-input {
    min-width: 0;
  }
  .log-entry {
    flex-wrap: wrap;
    gap: 2px 8px;
  }
  .log-timestamp {
    min-width: 0;
  }
  .log-target {
    max-width: 120px;
  }
  .log-count {
    margin-left: 0;
  }
}
</style>