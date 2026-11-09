<template>
  <div class="log-viewer">
    <div class="log-header">
      <h2>日志查看</h2>
      <div class="version-info" v-if="versionInfo">
        <span class="version-badge">{{ versionInfo.name }} v{{ versionInfo.version }}</span>
        <span class="version-detail">{{ versionInfo.build_profile }} · {{ versionInfo.rustc }}</span>
        <!-- PDF 引擎版本：系统 mutool -v + （编入时）内置 mupdf -->
        <span class="version-pdf" v-if="pdfInfo">
          <span
            class="version-chip"
            :class="{
              'chip-muted': !pdfInfo.mutool?.available,
              'chip-active': pdfInfo.engine?.active === 'mutool',
            }"
            :title="mutoolTitle"
          >
            mutool {{ mutoolShort }}
            <template v-if="pdfInfo.engine?.active === 'mutool'">（生效中）</template>
          </span>
          <span
            class="version-chip"
            v-if="pdfInfo.builtin_mupdf?.available"
            :class="{ 'chip-active': pdfInfo.engine?.active === 'mupdf' }"
            :title="mupdfTitle"
          >
            内置 mupdf {{ pdfInfo.builtin_mupdf.version }}
            <template v-if="pdfInfo.engine?.active === 'mupdf'">（生效中）</template>
          </span>
        </span>
        <span class="version-date">构建于 {{ versionInfo.build_time }}</span>
      </div>
      <span
        class="log-file"
        v-if="logFile"
        :title="`页面只显示进程内最近 ${capacity} 条；更早的历史日志请直接查看该文件`"
      >📄 {{ logFile }}</span>
    </div>

    <!-- 过滤栏 -->
    <div class="log-filters">
      <div class="filter-row">
        <select v-model="levelFilter" class="filter-select" @change="reload">
          <option v-for="item in LOG_LEVELS" :key="item.value" :value="item.value">
            {{ item.label }}
          </option>
        </select>
        <input
          v-model="keywordFilter"
          type="text"
          class="filter-input"
          placeholder="在当前列表中搜索（即时过滤）..."
        />
        <label class="filter-check" title="把 /assist/api/admin/logs 自身的请求日志也显示出来（默认隐藏，避免自己刷屏）">
          <input type="checkbox" v-model="includeSelf" @change="reload" />
          含本页请求
        </label>
        <button class="btn btn-outline" @click="reload" :disabled="loading">
          {{ loading ? '加载中...' : '刷新' }}
        </button>
        <button
          class="btn btn-outline"
          :class="{ 'btn-active': autoRefresh }"
          :title="`自动模式：每 ${AUTO_REFRESH_MS / 1000} 秒增量拉取新增日志（页面隐藏时暂停）`"
          @click="toggleAutoRefresh"
        >
          {{ autoRefresh ? '⏸ 暂停' : '▶ 自动' }}
        </button>
        <button class="btn btn-outline" @click="clearLogs" title="清空当前列表（服务端缓冲不受影响）">
          🗑 清空
        </button>
        <span class="log-count" v-if="visibleLogs.length > 0">
          显示 {{ visibleLogs.length }} 条
          <span v-if="keywordFilter" class="log-hint">（共 {{ logs.length }} 条）</span>
          <span v-if="cappedHint" class="log-hint" title="列表只保留最近的日志，更早的请用「刷新」重新载入">
            （超出上限仅保留最近 {{ MAX_ENTRIES }} 条）
          </span>
          <span v-if="lostTotal > 0" class="log-hint" title="生产日志的速度超过拉取速度，且已超出服务端缓冲跨度">
            （中间有 {{ lostTotal }} 条被丢弃）
          </span>
        </span>
        <span class="log-status">
          <span v-if="loadError" class="log-error">{{ loadError }}</span>
          <span v-else-if="lastUpdated">最后更新 {{ lastUpdated }}</span>
          <span v-if="autoRefresh" class="log-live-dot" title="自动轮询中">●</span>
        </span>
      </div>
    </div>

    <!-- 日志列表 -->
    <div class="log-list" ref="logListRef">
      <div v-if="loading" class="log-loading">加载中...</div>
      <div v-else-if="visibleLogs.length === 0" class="log-empty">
        {{ logs.length === 0 ? '暂无日志（等待新的日志输出...）' : '当前关键词下没有匹配的日志' }}
      </div>
      <div
        v-for="(entry, i) in visibleLogs"
        :key="entry.seq ?? `i-${i}`"
        class="log-entry"
        :class="'level-' + (entry.level || 'info').toLowerCase()"
      >
        <div class="log-entry-main" @click="toggleExpand(entry)">
          <span class="log-timestamp">{{ formatTimestamp(entry.timestamp) }}</span>
          <span class="log-target" v-if="entry.target">{{ entry.target }}</span>
          <span class="log-arrow">→</span>
          <span class="log-request" v-if="requestPath(entry)">
            <span class="log-method" v-if="stringField(entry, 'method')" :class="'method-' + methodClass(entry)">
              {{ stringField(entry, 'method') }}
            </span>
            <span class="log-path" :title="fullPath(entry)">
              {{ requestPath(entry) }}<template v-if="stringField(entry, 'query')">?{{ stringField(entry, 'query') }}</template>
            </span>
          </span>
          <span class="log-message">{{ entry.fields?.message || entry.message }}</span>
          <span class="log-expand-icon">{{ isExpanded(entry) ? '▼' : '▶' }}</span>
        </div>
        <!-- 展开详情：原始 JSON -->
        <div v-if="isExpanded(entry)" class="log-detail">
          <div class="log-detail-actions">
            <button class="btn btn-sm" @click.stop="openDetail(entry)">⤢ 大图查看</button>
          </div>
          <pre class="detail-json">{{ formatJson(asRecord(entry)) }}</pre>
        </div>
      </div>
    </div>
    <!-- JSON 详情模态框 -->
    <div v-if="detailEntry" class="json-modal-overlay" @click.self="closeDetail">
      <div class="json-modal">
        <div class="json-modal-header">
          <span class="json-modal-title">
            {{ detailEntry.level?.toUpperCase() || 'INFO' }} -
            {{ messageOf(detailEntry)?.slice(0, 80) }}
          </span>
          <div class="json-modal-actions">
            <button class="btn-copy" @click="copyJson" :title="copied ? '已复制' : '复制 JSON'">
              {{ copied ? '✓' : '📋' }}
            </button>
            <button class="json-modal-close" @click="closeDetail">×</button>
          </div>
        </div>
        <pre class="json-modal-body">{{ formatJson(asRecord(detailEntry)) }}</pre>
        <div class="json-modal-footer">
          <button class="btn btn-sm" @click="closeDetail">关闭</button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref, onMounted, onUnmounted, nextTick } from 'vue'
import { getLogs, LOG_LEVELS } from '@/api/admin'
import { useRequireToken } from '@/composables/useRequireToken'
import { apiGet } from '@/api/client'
import type { LogEntry, LogLevel } from '@/api/admin'

interface PdfVersionInfo {
  engine?: {
    configured: string
    active: string
    label: string
  }
  mutool?: {
    path: string
    available: boolean
    version: string | null
    error: string | null
  }
  builtin_mupdf?: {
    available: boolean
    version: string
  }
}

interface VersionInfo {
  name: string
  version: string
  build_time: string
  build_profile: string
  rustc: string
  target: string
  backend: string
  /** PDF 引擎版本：系统 `mutool -v` + （编入时）内置 mupdf 版本 */
  pdf?: PdfVersionInfo
}

const { requireToken } = useRequireToken()

/**
 * 自动轮询间隔（毫秒）。
 *
 * 后端只提供「进程内环形缓冲」的增量接口（不读日志文件），每次请求都是纯内存操作，
 * 因此 1 秒一次的开销可忽略。
 */
const AUTO_REFRESH_MS = 1000
/** 列表最多保留的条数：超出后从最旧的开始丢，避免长时间开页把 DOM 撑爆。 */
const MAX_ENTRIES = 2000
/** 单次请求的条数上限：命中更多时后端置 `truncated`，前端立刻再拉一次追平。 */
const PAGE_LIMIT = 500

const logs = ref<LogEntry[]>([])
const loading = ref(false)
/** 服务端等级阈值（「该等级及以上」） */
const levelFilter = ref<LogLevel>('info')
/** 关键词只在当前列表里过滤，即时生效，不触发请求 */
const keywordFilter = ref('')
/** 自动模式（定时增量拉取）；默认开启 */
const autoRefresh = ref(true)
/** 增量游标：上次响应的 `next_seq` */
const since = ref(0)
const lastUpdated = ref('')
const loadError = ref('')
/** 日志文件路径（后端只回显路径，供提示历史日志位置） */
const logFile = ref('')
/** 服务端环形缓冲容量 */
const capacity = ref(2000)
/** 累计被服务端环形缓冲丢弃的条数 */
const lostTotal = ref(0)
/** 是否显示「本接口自身请求」的日志（服务端过滤，改动需重载） */
const includeSelf = ref(false)
/** 是否因列表超过 MAX_ENTRIES 丢了最旧的条目 */
const cappedHint = ref(false)
const logListRef = ref<HTMLElement | null>(null)
const versionInfo = ref<VersionInfo | null>(null)
const detailEntry = ref<LogEntry | null>(null)
/** 展开的是哪一条：按日志 `seq` 记录，追加新日志时不会错位或自动折叠 */
const expandedSeq = ref<number | null>(null)
const copied = ref(false)
let pollTimer: ReturnType<typeof setInterval> | null = null
/** 防抖：上一次请求未返回时不叠加新请求 */
let inFlight = false

/** 关键词过滤后的展示列表（纯客户端过滤） */
const visibleLogs = computed(() => {
  const kw = keywordFilter.value.trim().toLowerCase()
  if (!kw) return logs.value
  return logs.value.filter((entry) => JSON.stringify(entry).toLowerCase().includes(kw))
})

/** PDF 引擎版本信息（日志页「系统版本」的一部分） */
const pdfInfo = computed<PdfVersionInfo | null>(() => versionInfo.value?.pdf ?? null)

/** 只取 `mutool version 1.28.5` 里的版本号，拿不到就退回可用状态文案 */
const mutoolShort = computed(() => {
  const mutool = pdfInfo.value?.mutool
  if (!mutool?.available) return '不可用'
  const matched = mutool.version?.match(/\d+(?:\.\d+)+/)
  return matched ? matched[0] : mutool.version || '版本未知'
})

const mutoolTitle = computed(() => {
  const mutool = pdfInfo.value?.mutool
  if (!mutool) return ''
  if (!mutool.available) {
    return `系统 mutool 不可用：${mutool.error || '未知原因'}（路径：${mutool.path}）`
  }
  return `系统 mutool：${mutool.version}\n路径：${mutool.path}`
})

const mupdfTitle = computed(() => {
  const builtin = pdfInfo.value?.builtin_mupdf
  if (!builtin?.available) return ''
  const state = pdfInfo.value?.engine?.active === 'mupdf' ? '当前生效' : '当前未启用'
  return `内置 mupdf（Cargo.toml 依赖版本 ${builtin.version}）：${state}`
})

/** 取 `fields` 里的字符串字段（请求类日志才有 method / path / query） */
function stringField(entry: LogEntry, name: string): string {
  const fields = entry.fields as Record<string, unknown> | undefined
  const value = fields?.[name]
  return typeof value === 'string' ? value : ''
}

/** 请求路径：请求开始/错误中间件写 `fields.path`，TraceLayer 的响应日志写 `fields.uri` */
function requestPath(entry: LogEntry): string {
  return stringField(entry, 'path') || stringField(entry, 'uri')
}

/** 完整请求目标（含查询串），用于 hover 提示 */
function fullPath(entry: LogEntry): string {
  const path = requestPath(entry)
  const query = stringField(entry, 'query')
  return query ? `${path}?${query}` : path
}

/** 方法名 → 着色用的类名 */
function methodClass(entry: LogEntry): string {
  return stringField(entry, 'method').toLowerCase() || 'other'
}

function messageOf(entry: LogEntry): string {
  const nested = (entry.fields as Record<string, unknown> | undefined)?.message
  return typeof nested === 'string' ? nested : String(entry.message ?? '')
}

/** LogEntry 带索引签名，转成纯对象给 JSON.stringify / 模态框用 */
function asRecord(entry: LogEntry): Record<string, unknown> {
  return entry as unknown as Record<string, unknown>
}

function isExpanded(entry: LogEntry): boolean {
  return entry.seq != null && expandedSeq.value === entry.seq
}

function toggleExpand(entry: LogEntry) {
  expandedSeq.value = isExpanded(entry) ? null : (entry.seq ?? null)
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
  navigator.clipboard.writeText(formatJson(asRecord(detailEntry.value)))
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

/** 用户是否已经在列表底部（在底部才自动跟随最新日志，否则保持阅读位置） */
function isAtBottom(): boolean {
  const el = logListRef.value
  if (!el) return true
  return el.scrollHeight - el.scrollTop - el.clientHeight < 48
}

function scrollToBottom() {
  nextTick(() => {
    const el = logListRef.value
    if (el) el.scrollTop = el.scrollHeight
  })
}

/** 列表超过上限时丢掉最旧的 */
function capLogs() {
  if (logs.value.length > MAX_ENTRIES) {
    logs.value.splice(0, logs.value.length - MAX_ENTRIES)
    cappedHint.value = true
  }
}

/**
 * 拉取日志。
 *
 * 服务端数据源是进程内环形缓冲（最近 N 条，进程重启即清空），接口是纯增量的：
 * - `reset = true`：游标归零，重新从缓冲区里最早的记录开始（刷新 / 换等级）；
 * - 否则只取 `since` 之后的新记录并追加到列表。
 */
async function loadLogs(opts: { reset?: boolean } = {}) {
  if (!requireToken()) return
  if (inFlight) return
  const reset = opts.reset === true
  inFlight = true
  if (reset) loading.value = true
  const stick = isAtBottom()
  let needMore = false
  try {
    const res = await getLogs({
      since: reset ? 0 : since.value,
      level: levelFilter.value,
      limit: PAGE_LIMIT,
      includeSelf: includeSelf.value
    })
    if (reset) {
      logs.value = []
      lostTotal.value = 0
      cappedHint.value = false
      expandedSeq.value = null
    }
    if (res.entries.length) {
      logs.value.push(...res.entries)
      capLogs()
      // 展开状态按 `seq` 记录，追加新日志时保持不变（用户能继续看展开的那条）
      if (stick) scrollToBottom()
    }
    if (res.lost > 0) lostTotal.value += res.lost
    since.value = res.next_seq ?? since.value
    logFile.value = res.file || logFile.value
    capacity.value = res.capacity || capacity.value
    lastUpdated.value = new Date().toLocaleTimeString('zh-CN', { hour12: false })
    loadError.value = ''
    // 命中数超过单次上限：立刻再拉一次把落后的部分追平
    needMore = res.truncated === true
  } catch (err) {
    // 增量失败保留已有日志（下一轮自动重试）；重置失败才清空
    if (reset) {
      logs.value = []
      lostTotal.value = 0
      cappedHint.value = false
    }
    const status = (err as { response?: { status?: number } })?.response?.status
    if (status === 401 || status === 403) {
      // 凭证失效 / 权限不足：继续每秒重试没有意义，停掉轮询等用户处理
      autoRefresh.value = false
      stopPolling()
      loadError.value =
        status === 401 ? '登录状态已失效，请重新登录' : '权限不足（仅管理员可查看日志）'
    } else {
      loadError.value = '日志加载失败'
    }
  } finally {
    loading.value = false
    inFlight = false
  }
  if (needMore) void loadLogs()
}

/** 手动刷新 / 切换等级：从缓冲区最早处重新载入 */
function reload() {
  since.value = 0
  void loadLogs({ reset: true })
}

/** 只清空页面上的列表（不改变游标，之后只显示新日志） */
function clearLogs() {
  logs.value = []
  expandedSeq.value = null
  lostTotal.value = 0
  cappedHint.value = false
}

function stopPolling() {
  if (pollTimer) {
    clearInterval(pollTimer)
    pollTimer = null
  }
}

function startPolling() {
  stopPolling()
  if (!autoRefresh.value) return
  pollTimer = setInterval(() => {
    // 页面隐藏时暂停拉取，回前台由 visibilitychange 立即补一次
    if (document.visibilityState === 'hidden' || inFlight) return
    void loadLogs()
  }, AUTO_REFRESH_MS)
}

function toggleAutoRefresh() {
  autoRefresh.value = !autoRefresh.value
  if (autoRefresh.value) {
    startPolling()
    void loadLogs()
  } else {
    stopPolling()
  }
}

/** 从后台切回前台：立即补一次增量，再恢复定时器 */
function onVisibilityChange() {
  if (document.visibilityState === 'visible' && autoRefresh.value) {
    void loadLogs()
    startPolling()
  }
}

onMounted(async () => {
  void loadLogs({ reset: true })
  startPolling()
  document.addEventListener('visibilitychange', onVisibilityChange)
  try {
    const info = await apiGet<VersionInfo>('/assist/api/admin/')
    versionInfo.value = info
  } catch { /* ignore */ }
})

onUnmounted(() => {
  stopPolling()
  document.removeEventListener('visibilitychange', onVisibilityChange)
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

/* PDF 引擎版本小标签（mutool / 内置 mupdf） */
.version-pdf {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.version-chip {
  font-size: 11px;
  padding: 1px 6px;
  border: 1px solid var(--border);
  border-radius: 4px;
  color: var(--text-secondary);
  background: var(--bg-white);
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'Consolas', monospace;
  cursor: help;
}

.version-chip.chip-active {
  border-color: var(--primary);
  color: var(--primary);
}

.version-chip.chip-muted {
  color: var(--text-secondary);
  border-style: dashed;
  opacity: 0.85;
}

.log-file {
  font-size: 12px;
  color: var(--text-secondary);
  background: var(--bg);
  padding: 2px 8px;
  border-radius: 4px;
  font-family: 'SF Mono', 'Cascadia Code', 'Fira Code', 'Consolas', monospace;
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

.filter-check {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  cursor: pointer;
  min-height: 34px;
}

.filter-check input {
  cursor: pointer;
}

.log-count {
  font-size: 12px;
  color: var(--text-secondary);
  margin-left: auto;
}

/* 最后更新时间 / 加载失败 / 自动模式指示灯 */
.log-status {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  font-size: 12px;
  color: var(--text-secondary);
}

.log-error {
  color: var(--error, #d33);
}

.log-live-dot {
  color: #2ecc71;
  font-size: 10px;
  animation: log-pulse 1.6s ease-in-out infinite;
}

@keyframes log-pulse {
  0%, 100% { opacity: 1; }
  50% { opacity: 0.25; }
}

/* 自动模式开启时高亮按钮 */
.btn-active {
  border-color: var(--primary, #2ecc71);
  color: var(--primary, #2ecc71);
}

/* 列表超限 / 丢弃日志提示 */
.log-hint {
  color: var(--warning, #c98a00);
  margin-left: 4px;
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

/* 请求类日志：METHOD + PATH（消息里只有「请求开始」，靠这两个字段看是哪次调用） */
.log-request {
  flex-shrink: 1;
  min-width: 0;
  display: inline-flex;
  align-items: baseline;
  gap: 6px;
  max-width: 46%;
}

.log-method {
  flex-shrink: 0;
  font-size: 10px;
  font-weight: 700;
  letter-spacing: 0.3px;
  padding: 0 5px;
  border-radius: 3px;
  background: #333;
  color: #9cdcfe;
  line-height: 1.6;
}

.method-get { color: #4fc3f7; }
.method-post { color: #c3e88d; }
.method-put { color: #ffcb6b; }
.method-patch { color: #f78c6c; }
.method-delete { color: #f07178; }
.method-options,
.method-head { color: #888; }

.log-path {
  flex: 1 1 auto;
  min-width: 0;
  color: #9cdcfe;
  font-size: 11px;
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
}

/* ── 展开详情 ─────────────────────────────────────────── */
.log-detail {
  padding: 8px 12px 8px 12px;
  background: #252526;
  border-top: 1px solid #333;
  font-size: 12px;
}

.log-detail-actions {
  display: flex;
  justify-content: flex-end;
  margin-bottom: 6px;
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
  .filter-check {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  cursor: pointer;
  min-height: 34px;
}

.filter-check input {
  cursor: pointer;
}

.log-count {
    margin-left: 0;
  }
  .log-request {
    max-width: 100%;
    flex: 1 1 100%;
  }
  .log-detail {
    padding-left: 12px;
  }
  .detail-json {
    font-size: 10px;
  }
}
</style>
