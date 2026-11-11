<template>
  <div class="admin-view">
    <h2>监控面板</h2>

    <!-- 统计卡片 -->
    <div class="stats-grid">
      <div class="stat-card">
        <span class="stat-value">{{ stats?.total_documents || 0 }}</span>
        <span class="stat-label">总文档数</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.status_counts?.uploaded || 0 }}</span>
        <span class="stat-label">已上传</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.status_counts?.parsing || 0 }}</span>
        <span class="stat-label">解析中</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.status_counts?.markdown_done || 0 }}</span>
        <span class="stat-label">解析完成</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ stats?.error_count || 0 }}</span>
        <span class="stat-label">错误</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ formatSize(stats?.total_size || 0) }}</span>
        <span class="stat-label">总大小</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ formatSize(stats?.pdf_size || 0) }}</span>
        <span class="stat-label">PDF 大小</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ formatSize(stats?.md_size || 0) }}</span>
        <span class="stat-label">Markdown 大小</span>
      </div>
      <div class="stat-card">
        <span class="stat-value">{{ formatSize(stats?.zip_size || 0) }}</span>
        <span class="stat-label">资源包大小</span>
      </div>
    </div>

    <!-- 文件构成（按文件事实列聚合，零磁盘 IO） -->
    <div class="section">
      <div class="section-header">
        <h3>文件构成</h3>
        <button class="btn btn-sm btn-outline" @click="load" :disabled="false">
          刷新
        </button>
      </div>

      <div class="stats-grid">
        <router-link
          v-for="card in fileCards"
          :key="card.key"
          class="stat-card file-card"
          :class="`file-${card.key}`"
          :to="card.to"
          :title="card.hint"
        >
          <span class="stat-value">{{ card.value }}</span>
          <span class="stat-label">{{ card.label }}</span>
        </router-link>
      </div>

      <div class="health-link-row">
        <router-link class="btn btn-sm btn-outline" :to="pdfOnlyLink">
          查看「只有 PDF、没有 Markdown」的文献（{{ fileCounts.pdf_only }}）→
        </router-link>
        <span class="health-link-hint">
          点击卡片同样可跳转到文献列表并按文件状态筛选（`pdf` / `md` / `zip` = present / absent）
        </span>
      </div>
    </div>

    <!-- PDF 健康 -->
    <div class="section">
      <div class="section-header">
        <h3>PDF 健康</h3>
        <button class="btn btn-sm btn-outline" @click="loadPdfHealth" :disabled="healthLoading">
          刷新
        </button>
      </div>

      <div class="stats-grid">
        <div
          v-for="card in healthCards"
          :key="card.key"
          class="stat-card health-card"
          :class="`health-${card.key}`"
        >
          <span class="stat-value">{{ card.value }}</span>
          <span class="stat-label">{{ card.label }}</span>
        </div>
      </div>

      <div class="health-link-row">
        <router-link class="btn btn-sm btn-outline" to="/assist?pdf_health=warning,error,missing">
          查看异常 PDF 文献（{{ health?.problem_count ?? 0 }}）→
        </router-link>
        <span class="health-link-hint">
          在文献列表中按「PDF 健康 = warning / error / missing」筛选
        </span>
      </div>
    </div>

    <!-- 维护操作 -->
    <div class="section">
      <h3>维护操作</h3>
      <div class="action-buttons">
        <button
          class="btn btn-outline"
          title="只解析「有 PDF 但还没有 Markdown」的文档；没有 PDF 的记录会被跳过"
          @click="batchParse"
        >批量解析有 PDF 无 Markdown</button>
        <button
          class="btn btn-outline"
          title="只处理已有 Markdown 的文档；只识别 Markdown 里的「引用格式 / 引用本文 / DOI」，没有线索的会跳过（几毫秒一篇）"
          @click="batchExtractHeuristic"
        >批量启发式抽取元数据</button>
        <button
          class="btn btn-outline"
          title="只为 PDF/Markdown/资源包 大小为空（NULL）的文档补写大小，不动已有数据"
          @click="syncMissingFileSizes"
        >补充缺失的大小信息</button>
        <button
          class="btn btn-outline"
          title="只检查从未做过健康检查（pdf_health 为空）的文档"
          @click="checkMissingPdfHealth"
        >补充缺失的 PDF 健康信息</button>
        <button
          class="btn btn-outline"
          title="只为没有 file_hash 的文档计算哈希"
          @click="recalcMissingHashes"
        >补充缺失的哈希信息</button>
        <br>
        <button class="btn btn-outline" @click="doSyncFileSizes">同步全部文件大小</button>
        <button class="btn btn-outline" @click="checkPdfHealthBatch">检查全部 PDF 健康</button>
        <button class="btn btn-outline" @click="recalcHashes">重算全部哈希</button>
        <button class="btn btn-outline" @click="dedup">文献去重</button>
        <button class="btn btn-outline" @click="goDuplicates">手动去重</button>
      </div>
    </div>

    <!-- 流水线监控 -->
    <div class="section monitor-section">
      <div class="section-header">
        <h3>流水线监控</h3>
        <button class="btn btn-sm btn-outline" @click="refresh" :disabled="loading">
          刷新
        </button>
      </div>

      <!-- 队列概览 -->
      <div class="stats-grid">
        <div class="stat-card">
          <span class="stat-value">{{ queue.max_concurrent_tasks }}</span>
          <span class="stat-label">最大并发</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">{{ queue.running_tasks }}</span>
          <span class="stat-label">运行中</span>
        </div>
        <div class="stat-card">
          <span class="stat-value">{{ queue.available_slots }}</span>
          <span class="stat-label">可用槽位</span>
        </div>
      </div>

      <!-- 活跃任务 -->
      <div class="subsection">
        <h4>活跃任务</h4>
        <div class="table-wrap">
          <table class="doc-table">
            <thead>
              <tr>
                <th>文档 ID</th>
                <th>标题</th>
                <th>任务类型</th>
                <th>解析工具</th>
                <th>解析进度</th>
                <th>状态</th>
                <th>开始时间</th>
              </tr>
            </thead>
            <tbody>
              <tr v-for="task in activeTasks" :key="task.doc_id">
                <td>{{ task.doc_id }}</td>
                <td>{{ task.doc_title }}</td>
                <td>{{ taskTypeLabel(task.task_type) }}</td>
                <td>
                  <span v-if="task.mineru_config_name" class="tool-tag">
                    {{ task.mineru_config_name }}
                  </span>
                  <span v-else class="text-muted">-</span>
                </td>
                <td>
                  <div v-if="hasTaskProgress(task)" class="parse-progress">
                    <div class="progress-mini-bar-wrap">
                      <div
                        class="progress-mini-bar"
                        :style="{ width: (task.extracted_pages! / task.total_pages! * 100) + '%' }"
                      ></div>
                    </div>
                    <span class="progress-text">{{ task.extracted_pages }}/{{ task.total_pages }}{{ task.task_type === 'parse' ? ' 页' : ' 条' }}</span>
                  </div>
                  <span v-else class="text-muted">-</span>
                </td>
                <td><StatusBadge :status="task.status" /></td>
                <td>{{ formatTime(task.started_at) }}</td>
              </tr>
            </tbody>
          </table>
          <div v-if="activeTasks.length === 0" class="empty-hint">
            暂无活跃任务
          </div>
        </div>
      </div>

      <!-- 批量进度 -->
      <div class="subsection">
        <h4>批量进度</h4>
        <div v-if="batchProgress && batchProgress.active" class="batch-progress">
          <div class="progress-header">
            <span class="stage">{{ batchProgress.stage }}</span>
            <span v-if="batchProgress.vector_db_id" class="target-db">
              目标库: {{ batchProgress.vector_db_id }}
            </span>
          </div>
          <div class="progress-bar-wrap">
            <div
              class="progress-bar"
              :style="{ width: progressPercent + '%' }"
            ></div>
          </div>
          <div class="progress-stats">
            <span>总文档: {{ batchProgress.total }}</span>
            <span>已处理: {{ batchProgress.processed }}</span>
            <span>失败: {{ batchProgress.errors }}</span>
            <span v-if="batchProgress.total_batches > 0">
              批次: {{ batchProgress.current_batch }} / {{ batchProgress.total_batches }}
            </span>
          </div>
        </div>
        <div v-else class="empty-hint">
          无批量任务运行
        </div>
      </div>
    </div>


  </div>
</template>

<script setup lang="ts">
import { ref, onMounted, onUnmounted, computed } from 'vue'
import { useRouter } from 'vue-router'
import {
  getStats,
  getPdfHealthOverview,
  recalculateHashes,
  checkPdfHealth,
  deduplicateDocuments,
  syncFileSizes,
} from '@/api/admin'
import { startBatchParse, startBatchExtract } from '@/api/pipeline'
import { usePipelineStore } from '@/stores/pipeline'
import { useToast } from '@/composables/useToast'
import StatusBadge from '@/components/StatusBadge.vue'
import type { ActiveTask } from '@/types/pipeline'
import type { PdfHealthOverview, SystemStats } from '@/api/admin'

const { showToast, showSuccess, showInfo, showError } = useToast()
const router = useRouter()
const pipelineStore = usePipelineStore()

const stats = ref<SystemStats | null>( null)
const loading = computed(() => pipelineStore.loading)
const queue = computed(() => pipelineStore.queue)
const activeTasks = computed(() => pipelineStore.activeTasks)
const batchProgress = computed(() => pipelineStore.batchProgress)

// PDF 健康概览（分布计数 + 异常文档总数）
const health = ref<PdfHealthOverview | null>(null)
const healthLoading = ref(false)

const healthCards = computed(() => {
  const counts = health.value?.counts ?? {}
  return [
    { key: 'ok', label: '检查正常', value: counts.ok ?? 0 },
    { key: 'warning', label: '有警告', value: counts.warning ?? 0 },
    { key: 'error', label: '无法解析', value: counts.error ?? 0 },
    { key: 'missing', label: '文件缺失', value: counts.missing ?? 0 },
    { key: 'unchecked', label: '未检查', value: health.value?.unchecked ?? 0 }
  ]
})

const progressPercent = computed(() => {
  const p = batchProgress.value
  if (!p || p.total === 0) return 0
  return Math.min(100, Math.round((p.processed / p.total) * 100))
})

/** 文件构成计数（后端 `stats.file_counts`；缺失时全部当 0） */
const fileCounts = computed(() => {
  const c = stats.value?.file_counts
  return {
    pdf_only: c?.pdf_only ?? 0,
    pdf_and_md: c?.pdf_and_md ?? 0,
    md_only: c?.md_only ?? 0,
    none: c?.none ?? 0,
    with_zip: c?.with_zip ?? 0
  }
})

/** 「只有 PDF、没有 Markdown」的文献列表链接（列表页支持 pdf_state / md_state） */
const pdfOnlyLink = { path: '/assist', query: { pdf_state: 'present', md_state: 'absent' } }

const fileCards = computed(() => [
  {
    key: 'pdf-only',
    label: '只有 PDF（无 Markdown）',
    value: fileCounts.value.pdf_only,
    to: pdfOnlyLink,
    hint: '有 PDF 但还没解析出 Markdown（待解析 / 解析失败）'
  },
  {
    key: 'pdf-and-md',
    label: 'PDF + Markdown',
    value: fileCounts.value.pdf_and_md,
    to: { path: '/assist', query: { pdf_state: 'present', md_state: 'present' } },
    hint: '解析完成的文献'
  },
  {
    key: 'md-only',
    label: '只有 Markdown（缺 PDF）',
    value: fileCounts.value.md_only,
    to: { path: '/assist', query: { pdf_state: 'absent', md_state: 'present' } },
    hint: 'PDF 文件丢了，但解析结果还在'
  },
  {
    key: 'none',
    label: '无文件',
    value: fileCounts.value.none,
    to: { path: '/assist', query: { pdf_state: 'absent', md_state: 'absent' } },
    hint: 'PDF 与 Markdown 都不存在（如 BibTeX 导入的纯元数据条目）'
  },
  {
    key: 'with-zip',
    label: '有资源包',
    value: fileCounts.value.with_zip,
    to: { path: '/assist', query: { zip_state: 'present' } },
    hint: '含图片资源包（MinerU 解析产物）'
  }
])


function formatSize(bytes: number): string {
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  if (bytes < 1024 * 1024 * 1024) return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
  return (bytes / (1024 * 1024 * 1024)).toFixed(1) + ' GB'
}

async function load() {
  try {
    const s = await getStats()
    stats.value = s
  } catch (e) {
    showError('加载失败')
  }
}

/** 加载 PDF 健康概览（分布计数 + 异常文档总数） */
async function loadPdfHealth() {
  healthLoading.value = true
  try {
    health.value = await getPdfHealthOverview()
  } catch {
    showError('PDF 健康数据加载失败')
  } finally {
    healthLoading.value = false
  }
}

async function batchExtractHeuristic() {
  if (
    !confirm(
      '确定对「已有 Markdown」的文档批量做启发式元数据抽取？\n\n只识别 Markdown 里的「引用格式 / 引用本文 / DOI」，没有这些线索的文档会跳过（不会调用大模型）。'
    )
  )
    return
  try {
    const res = await startBatchExtract('heuristic')
    showSuccess(res.detail || '批量启发式抽取已开始')
  } catch {
    showError('批量启发式抽取提交失败')
  }
}

async function batchParse() {
  if (!confirm('确定批量解析「有 PDF 但缺少 Markdown」的文档？没有 PDF 的记录会被跳过。')) return
  try {
    const res = await startBatchParse()
    showInfo(res.detail)
  } catch {
    showError('批量解析提交失败')
  }
}

async function recalcHashes() {
  if (!confirm('确定重新计算所有文档哈希？任务在后台逐个执行，进度见下方「活跃任务」。')) return
  try {
    const res = await recalculateHashes()
    if (res.started === false) showInfo(res.detail)
    else showToast(res.detail, 'success')
  } catch {
    showError('哈希重算提交失败')
  }
}

async function checkPdfHealthBatch() {
  if (!confirm('确定批量检查所有文档的 PDF 健康？任务在后台逐个执行，进度见下方「活跃任务」。')) return
  try {
    const res = await checkPdfHealth()
    if (res.started === false) showInfo(res.detail)
    else showToast(res.detail, 'success')
  } catch {
    showError('PDF 健康检查提交失败')
  }
}

async function dedup() {
  if (!confirm('确定按文件哈希去重？相同哈希的文档将保留最早的一个，其余删除。此操作不可逆！')) return
  try {
    const res = await deduplicateDocuments()
    showToast(res.detail, 'success')
  } catch {
    showError('文献去重失败')
  }
}

function goDuplicates() {
  router.push('/assist/duplicates')
}

async function doSyncFileSizes() {
  if (!confirm('确定同步所有文档的文件信息（PDF/MD/ZIP 大小）到数据库？')) return
  try {
    const res = await syncFileSizes()
    showToast(res.detail, 'success')
  } catch {
    showError('同步文件信息失败')
  }
}

/** 只补大小信息为空的文档（不动已有数据） */
async function syncMissingFileSizes() {
  if (!confirm('确定只为「大小信息缺失」的文档补写 PDF/Markdown/资源包 大小？任务在后台执行。')) return
  try {
    const res = await syncFileSizes({ onlyMissing: true })
    showToast(res.detail, 'success')
  } catch {
    showError('补充文件大小失败')
  }
}

/** 只检查尚未做过健康检查的文档 */
async function checkMissingPdfHealth() {
  if (!confirm('确定只检查「从未做过健康检查」的文档？任务在后台逐个执行，进度见下方「活跃任务」。')) return
  try {
    const res = await checkPdfHealth({ onlyMissing: true })
    if (res.started === false) showInfo(res.detail)
    else showToast(res.detail, 'success')
  } catch {
    showError('PDF 健康检查提交失败')
  }
}

/** 只为缺少 file_hash 的文档计算哈希 */
async function recalcMissingHashes() {
  if (!confirm('确定只为「缺少 file_hash」的文档计算哈希？任务在后台逐个执行，进度见下方「活跃任务」。')) return
  try {
    const res = await recalculateHashes({ onlyMissing: true })
    if (res.started === false) showInfo(res.detail)
    else showToast(res.detail, 'success')
  } catch {
    showError('哈希补齐提交失败')
  }
}

function refresh() {
  pipelineStore.fetchStatus()
  loadPdfHealth()
}

function taskTypeLabel(type: string): string {
  const labels: Record<string, string> = {
    parse: '解析',
    extract: '元数据提取',
    'extract-heuristic': '启发式抽取',
    index: '索引',
    crossref: 'Crossref',
    'extract-pdf-meta': 'PDF 元数据',
    hash: '重算哈希',
    'pdf-health': 'PDF 健康检查'
  }
  return labels[type] || type
}

/** 「活跃任务」里是否有可展示的进度（MinerU 为页数，重算哈希为条数） */
function hasTaskProgress(task: ActiveTask): boolean {
  return task.extracted_pages != null && task.total_pages != null && task.total_pages > 0
}

function formatTime(iso: string) {
  return new Date(iso).toLocaleString('zh-CN')
}

onMounted(() => {
  load()
  loadPdfHealth()
  pipelineStore.fetchStatus()
  pipelineStore.startPolling(3000)
})

onUnmounted(() => {
  pipelineStore.stopPolling()
})
</script>

<style scoped>
.section-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 16px;
}

.subsection {
  margin-top: 20px;
}

.subsection h4 {
  margin: 0 0 12px 0;
  font-size: 15px;
  color: var(--text-primary);
}

.table-wrap {
  overflow-x: auto;
}

/* ── 文件构成 ── */
.file-card {
  text-decoration: none;
  color: inherit;
  cursor: pointer;
  transition: border-color 0.15s, transform 0.15s, box-shadow 0.15s;
}

.file-card:hover {
  border-color: var(--accent, #2f9e44);
  transform: translateY(-1px);
  box-shadow: 0 2px 8px rgba(0, 0, 0, 0.06);
}

.file-card:active {
  transform: scale(0.99);
}

/* 「只有 PDF 无 Markdown」是待解析的主体，标黄提醒 */
.file-card.file-pdf-only .stat-value {
  color: #9a6400;
}

.file-card.file-md-only .stat-value {
  color: var(--danger-text);
}

.file-card.file-none .stat-value {
  color: var(--text-secondary);
}

/* ── PDF 健康 ── */
.health-card .stat-value {
  color: var(--text-primary);
}

.health-card.health-ok .stat-value {
  color: #1b7f4b;
}

.health-card.health-warning .stat-value {
  color: #9a6400;
}

.health-card.health-error .stat-value,
.health-card.health-missing .stat-value {
  color: var(--danger-text);
}

.health-card.health-unchecked .stat-value {
  color: var(--text-secondary);
}

.health-link-row {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
  margin-top: 20px;
}

.health-link-hint {
  font-size: 12px;
  color: var(--text-secondary);
}

.tool-tag {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 4px;
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  font-size: 12px;
  white-space: nowrap;
  max-width: 140px;
  overflow: hidden;
  text-overflow: ellipsis;
}

.parse-progress {
  display: flex;
  align-items: center;
  gap: 6px;
  min-width: 100px;
}

.progress-mini-bar-wrap {
  width: 50px;
  height: 6px;
  background: var(--border);
  border-radius: 3px;
  overflow: hidden;
  flex-shrink: 0;
}

.progress-mini-bar {
  height: 100%;
  background: var(--primary);
  border-radius: 3px;
  transition: width 0.5s ease;
}

.progress-text {
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
}

.text-muted {
  color: var(--text-secondary);
  font-size: 12px;
}

.empty-hint {
  text-align: center;
  padding: 20px;
  color: var(--text-secondary);
  font-size: 13px;
}

.batch-progress {
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 16px;
}

.progress-header {
  display: flex;
  justify-content: space-between;
  align-items: center;
  margin-bottom: 12px;
}

.stage {
  font-weight: 600;
  text-transform: uppercase;
}

.target-db {
  font-size: 13px;
  color: var(--text-secondary);
}

.progress-bar-wrap {
  height: 12px;
  background: var(--border);
  border-radius: 6px;
  overflow: hidden;
  margin-bottom: 12px;
}

.progress-bar {
  height: 100%;
  background: var(--primary);
  transition: width 0.3s ease;
}

.progress-stats {
  display: flex;
  gap: 16px;
  flex-wrap: wrap;
  font-size: 13px;
  color: var(--text-secondary);
}

@media (max-width: 768px) {
  .stats-grid {
    grid-template-columns: repeat(2, 1fr);
    gap: 10px;
  }

  .stat-card {
    padding: 14px;
  }

  .stat-value {
    font-size: 24px;
  }

  .action-buttons {
    flex-wrap: wrap;
    gap: 8px;
  }

  .action-buttons .btn {
    flex: 1;
    min-width: 120px;
    padding: 10px 14px;
    font-size: 13px;
  }
}

@media (max-width: 480px) {
  .stats-grid {
    grid-template-columns: 1fr;
    gap: 8px;
  }

  .stat-card {
    padding: 12px;
  }

  .stat-value {
    font-size: 22px;
  }

  .action-buttons .btn {
    min-width: 100px;
    font-size: 12px;
    padding: 8px 10px;
  }
}

</style>