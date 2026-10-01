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
      >
        <div class="log-entry-main" @click="toggleExpand(i)">
          <span class="log-timestamp">{{ formatTimestamp(entry.timestamp) }}</span>
          <span class="log-target" v-if="entry.target">{{ entry.target }}</span>
          <span class="log-arrow">→</span>
          <span class="log-message">{{ entry.fields?.message || entry.message }}</span>
          <span class="log-expand-icon">{{ expandedIndex === i ? '▼' : '▶' }}</span>
        </div>
        <!-- 展开详情：原始 JSON -->
        <div v-if="expandedIndex === i" class="log-detail">
          <pre class="detail-json">{{ formatJson(entry) }}</pre>
        </div>
      </div>
    </div>
    <!-- JSON 详情模态框 -->
    <div v-if="detailEntry" class="json-modal-overlay" @click.self="closeDetail">
      <div class="json-modal">
        <div class="json-modal-header">
          <span class="json-modal-title">
            {{ detailEntry.level?.toUpperCase() || 'INFO' }} -
            {{ detailEntry.message?.slice(0, 80) }}
          </span>
          <div class="json-modal-actions">
            <button class="btn-copy" @click="copyJson" :title="copied ? '已复制' : '复制 JSON'">
              {{ copied ? '✓' : '📋' }}
            </button>
            <button class="json-modal-close" @click="closeDetail">×</button>
          </div>
        </div>
        <pre class="json-modal-body">{{ formatJson(detailEntry) }}</pre>
        <div class="json-modal-footer">
          <button class="btn btn-sm" @click="closeDetail">关闭</button>
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
const detailEntry = ref<LogEntry | null>(null)
const expandedIndex = ref<number | null>(null)
const copied = ref(false)
let refreshTimer: ReturnType<typeof setInterval> | null = null

function toggleExpand(i: number) {
  expandedIndex.value = expandedIndex.value === i ? null : i
}

function openDetail(entry: LogEntry) {
  detailEntry.value = entry
}

function closeDetail() {
  detailEntry.value = null
  copied.value = false
}

function formatJson(obj: Record<string, unknown>): string {
  return JSON.stringify(obj, null, 2)
}

function copyJson() {
  if (!detailEntry.value) return
  navigator.clipboard.writeText(formatJson(detailEntry.value))
  copied.value = true
  setTimeout(() => { copied.value = false }, 1500)
}

function formatTimestamp(ts: string): string {
  if (!ts) return ''
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
  border-bottom: 1px solid #2a2a2a;
}

.log-entry:last-child {
  border-bottom: none;
}

.log-entry-main {
  display: flex;
  gap: 8px;
  padding: 3px 12px;
  align-items: baseline;
  color: #d4d4d4;
  cursor: pointer;
  transition: background 0.1s;
  flex-wrap: wrap;
}

.log-entry-main:hover {
  background: #2a2a2a;
}

.log-expand-icon {
  flex-shrink: 0;
  color: #666;
  font-size: 10px;
  width: 14px;
  text-align: center;
  margin-left: auto;
}

/* ── JSON 详情模态框 ──────────────────────────────────── */
.json-modal-overlay {
  position: fixed;
  inset: 0;
  background: rgba(0, 0, 0, 0.5);
  z-index: 1000;
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 20px;
}

.json-modal {
  background: #1e1e1e;
  border: 1px solid #333;
  border-radius: 12px;
  width: 90vw;
  max-width: 800px;
  max-height: 80vh;
  display: flex;
  flex-direction: column;
  box-shadow: 0 16px 48px rgba(0, 0, 0, 0.4);
}

.json-modal-header {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 12px 16px;
  border-bottom: 1px solid #333;
  gap: 12px;
}

.json-modal-title {
  font-size: 14px;
  font-weight: 600;
  color: #d4d4d4;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  flex: 1;
}

.json-modal-actions {
  display: flex;
  align-items: center;
  gap: 8px;
  flex-shrink: 0;
}

.btn-copy {
  background: none;
  border: 1px solid #444;
  color: #888;
  font-size: 14px;
  cursor: pointer;
  padding: 4px 8px;
  border-radius: 4px;
  line-height: 1;
  min-width: 36px;
  min-height: 32px;
}
.btn-copy:hover {
  background: #333;
  color: #fff;
  border-color: #666;
}

.json-modal-close {
  background: none;
  border: none;
  color: #888;
  font-size: 22px;
  cursor: pointer;
  padding: 0 4px;
  line-height: 1;
  border-radius: 4px;
  min-width: 36px;
  min-height: 36px;
}
.json-modal-close:hover {
  background: #333;
  color: #fff;
}

.json-modal-body {
  flex: 1;
  overflow: auto;
  padding: 16px;
  margin: 0;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'Consolas', monospace;
  font-size: 12px;
  line-height: 1.6;
  color: #ce9178;
  white-space: pre;
  tab-size: 2;
}

.json-modal-footer {
  display: flex;
  justify-content: flex-end;
  padding: 10px 16px;
  border-top: 1px solid #333;
}

/* 级别着色 */
.log-entry.level-error .log-entry-main {
  background: rgba(244, 67, 54, 0.08);
}
.log-entry.level-error .log-entry-main:hover {
  background: rgba(244, 67, 54, 0.15);
}
.log-entry.level-warn .log-entry-main {
  background: rgba(255, 152, 0, 0.06);
}
.log-entry.level-warn .log-entry-main:hover {
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

.log-arrow {
  flex-shrink: 0;
  color: #888;
  font-size: 11px;
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
}

/* ── 展开详情 ─────────────────────────────────────────── */
.log-detail {
  padding: 8px 12px 8px 12px;
  background: #252526;
  border-top: 1px solid #333;
  font-size: 12px;
}

.detail-json {
  margin: 0;
  padding: 8px;
  background: #1e1e1e;
  border-radius: 4px;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'Consolas', monospace;
  font-size: 11px;
  line-height: 1.5;
  color: #ce9178;
  white-space: pre-wrap;
  word-break: break-all;
  overflow-x: auto;
  max-height: 400px;
  overflow-y: auto;
}

@media (max-width: 768px) {
  .filter-row {
    flex-direction: column;
    align-items: stretch;
  }
  .filter-input {
    min-width: 0;
  }
  .log-entry-main {
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
  .log-detail {
    padding-left: 12px;
  }
  .detail-json {
    font-size: 10px;
  }
}
</style>