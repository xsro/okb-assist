<template>
  <div class="doc-manage-view" v-if="doc">
    <div class="detail-header">
      <h2>文档管理: {{ doc.title || doc.filename || '(无标题)' }}</h2>
      <router-link :to="{ name: 'detail', params: { id: doc.id } }" class="btn btn-sm btn-outline">
        返回详情
      </router-link>
    </div>

    <!-- 文档错误横幅 -->
    <div v-if="doc.status === 'error' && doc.status_message" class="doc-error-banner">
      <span class="error-icon">⚠</span>
      <span>{{ doc.status_message }}</span>
    </div>

    <!-- 文献操作 -->
    <div class="section">
      <h3>文献操作</h3>

      <!-- 流水线 -->
      <div class="action-group">
        <span class="action-label">流水线</span>
        <div class="action-buttons">
          <button class="btn" @click="runStage('parse')">解析</button>
          <div class="index-control">
            <select v-model="selectedIndexDb" class="select">
              <option value="">选择索引库</option>
              <option
                v-for="db in enabledVectorDbs"
                :key="db.id"
                :value="db.id"
              >
                {{ db.name || db.id }}
              </option>
            </select>
            <button
              class="btn"
              :disabled="!selectedIndexDb"
              @click="runIndex"
            >
              索引
            </button>
          </div>
        </div>
      </div>

      <!-- 已索引数据库 -->
      <div class="action-group">
        <span class="action-label">已索引库</span>
        <div class="action-buttons">
          <div v-if="docIndexes.length === 0" class="empty-hint">
            尚未索引到任何数据库
          </div>
          <div v-else class="index-list">
            <div
              v-for="idx in docIndexes"
              :key="idx.vector_db_id"
              class="index-item"
              :class="`status-${idx.status}`"
            >
              <span class="index-tag-text">
                {{ idx.vector_db_id }}
                <small>({{ idx.status }})</small>
              </span>
              <span
                v-if="idx.status === 'error' && idx.error_message"
                class="index-error"
                :title="idx.error_message"
              >⚠ {{ idx.error_message }}</span>
            </div>
          </div>
        </div>
      </div>

      <!-- 元数据补全 -->
      <div class="action-group">
        <span class="action-label">元数据补全</span>
        <div class="action-buttons">
          <button class="btn" @click="runStage('extract')">从 Markdown 文件</button>
          <button class="btn btn-outline" @click="enrichPdfMeta">PDF 元数据</button>
          <button class="btn btn-outline" @click="enrichCrossref">Crossref</button>
        </div>
      </div>

      <!-- 文件管理 -->
      <div class="action-group">
        <span class="action-label">文件管理</span>
        <div class="action-buttons">
          <button class="btn btn-outline" @click="selectReplacePdf">替换 PDF</button>
          <button class="btn btn-outline" @click="selectReplaceMarkdown">替换 Markdown</button>
          <button class="btn btn-outline" @click="selectReplaceAsset">替换图片资源包</button>
          <button class="btn btn-outline" @click="rehash">重算哈希</button>
          <span v-if="hashResult" class="hash-result">
            {{ hashResult.old_file_hash === hashResult.file_hash ? '哈希一致' : '哈希已更新' }}
          </span>
          <input
            ref="replacePdfInput"
            type="file"
            accept=".pdf,application/pdf"
            style="display: none"
            @change="onReplacePdfSelected"
          />
          <input
            ref="replaceMarkdownInput"
            type="file"
            accept=".md,text/markdown"
            style="display: none"
            @change="onReplaceMarkdownSelected"
          />
          <input
            ref="replaceAssetInput"
            type="file"
            accept=".zip,application/zip"
            style="display: none"
            @change="onReplaceAssetSelected"
          />
        </div>
      </div>

      <!-- 文件大小 -->
      <div class="action-group">
        <span class="action-label">文件大小</span>
        <div class="action-buttons file-sizes-list">
          <span class="file-size-item">
            <span class="file-label">PDF</span>
            <span class="file-value">{{ formatFile(doc.pdf_size) }}</span>
          </span>
          <span class="file-size-item">
            <span class="file-label">Markdown</span>
            <span class="file-value">{{ formatFile(doc.md_size) }}</span>
          </span>
          <span class="file-size-item">
            <span class="file-label">资源包 ZIP</span>
            <span class="file-value">{{ formatFile(doc.zip_size) }}</span>
          </span>
        </div>
      </div>

      <!-- 状态设置 -->
      <div class="action-group">
        <span class="action-label">设置状态</span>
        <div class="action-buttons">
          <select v-model="selectedTargetStatus" class="select status-select">
            <option value="uploaded">uploaded</option>
            <option value="markdown_done">markdown_done</option>
          </select>
          <button class="btn btn-warning" @click="setStatus">设置状态</button>
        </div>
      </div>

      <!-- 危险操作 -->
      <div class="action-group action-group-danger">
        <span class="action-label">危险操作</span>
        <div class="action-buttons">
          <button class="btn btn-danger" @click="removeDocument">删除文档</button>
        </div>
      </div>
    </div>

    <!-- 元数据编辑 -->
    <div class="section">
      <h3>编辑元数据</h3>
      <div class="form-group">
        <label>标题</label>
        <input v-model="form.title" type="text" />
      </div>
      <div class="form-group">
        <label>作者</label>
        <input v-model="form.authors" type="text" />
      </div>
      <div class="form-group">
        <label>期刊</label>
        <input v-model="form.journal" type="text" />
      </div>
      <div class="form-row">
        <div class="form-group">
          <label>年份</label>
          <input v-model.number="form.year" type="number" />
        </div>
        <div class="form-group">
          <label>文档类型</label>
          <input v-model="form.doc_type" type="text" />
        </div>
        <div class="form-group">
          <label>DOI</label>
          <input v-model="form.doi" type="text" />
        </div>
      </div>
      <div class="form-group">
        <label>关键词</label>
        <input v-model="form.keywords" type="text" />
      </div>
      <div class="form-group">
        <label>摘要</label>
        <textarea v-model="form.abstract" rows="4"></textarea>
      </div>
      <button class="btn" @click="saveMetadata">保存</button>
    </div>

  </div>

  <div v-else class="loading">加载中...</div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { getDocument, updateDocument, replacePdf, replaceMarkdown, replaceAsset, deleteDocument, rehashDocument } from '@/api/documents'
import { getServiceConfig } from '@/api/config'
import {
  parseDocument,
  extractDocument,
  indexDocument,
  getDocumentIndexes,
  resetDocument,
  crossrefDocument,
  extractPdfMeta
} from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document } from '@/types/document'
import type { VectorDbConfig } from '@/types/config'
import type { DocumentIndexInfo } from '@/types/pipeline'

const route = useRoute()
const router = useRouter()
const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const doc = ref<Document | null>(null)
const form = ref<Partial<Document>>({})
const replacePdfInput = ref<HTMLInputElement>()
const replaceMarkdownInput = ref<HTMLInputElement>()
const replaceAssetInput = ref<HTMLInputElement>()
const vectorDbs = ref<VectorDbConfig[]>([])
const selectedIndexDb = ref('')
const selectedTargetStatus = ref('uploaded')
const docIndexes = ref<DocumentIndexInfo[]>([])
const hashResult = ref<{ file_hash: string; old_file_hash: string | null } | null>(null)

const enabledVectorDbs = computed(() =>
  vectorDbs.value.filter((db) => db.enabled !== false)
)

async function loadVectorDbs() {
  try {
    const config = await getServiceConfig()
    vectorDbs.value = config.vector_dbs || []
  } catch {
    vectorDbs.value = []
  }
}

async function loadIndexes() {
  const id = parseInt(route.params.id as string)
  try {
    const res = await getDocumentIndexes(id)
    docIndexes.value = res.indexes
  } catch {
    docIndexes.value = []
  }
}

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    doc.value = await getDocument(id)
    form.value = { ...doc.value }
    await loadIndexes()
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

function getErrorMessage(err: unknown): string {
  if (err && typeof err === 'object') {
    const axiosErr = err as { response?: { data?: { detail?: string } }; message?: string }
    // Axios 错误：优先取后端返回的 detail 字段
    if (axiosErr.response?.data?.detail) {
      return axiosErr.response.data.detail
    }
    // 网络错误（如连接被拒绝、超时）
    if (axiosErr.message) {
      if (axiosErr.message.includes('Network Error') || axiosErr.message.includes('connect')) {
        return '无法连接到服务器，请检查后端是否运行'
      }
      if (axiosErr.message.includes('timeout')) {
        return '请求超时，请稍后重试'
      }
      return axiosErr.message
    }
  }
  return '操作失败'
}

async function runStage(stage: string) {
  const id = parseInt(route.params.id as string)
  try {
    if (stage === 'parse') {
      await parseDocument(id)
    } else if (stage === 'extract') {
      await extractDocument(id)
    }
    showSuccess('已启动任务')
    setTimeout(load, 2000)
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

async function runIndex() {
  const id = parseInt(route.params.id as string)
  const dbId = selectedIndexDb.value
  if (!dbId) return
  try {
    await indexDocument(id, dbId)
    showSuccess(`已提交索引到 ${dbId}`)
    selectedIndexDb.value = ''
    setTimeout(load, 2000)
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

async function setStatus() {
  const id = parseInt(route.params.id as string)
  const target = selectedTargetStatus.value
  if (!confirm(`确定将文档状态设置为「${target}」？`)) return
  try {
    const res = await resetDocument(id, target)
    showSuccess(`状态已设置为 ${res.status}`)
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

async function removeDocument() {
  const id = parseInt(route.params.id as string)
  const title = doc.value?.title || `#${id}`
  if (!confirm(`确定删除文档「${title}」吗？\n此操作将永久删除数据库记录与本地文件（PDF、Markdown 等），且不可恢复！`)) return
  if (!confirm('再次确认：真的要永久删除该文档吗？')) return
  try {
    await deleteDocument(id)
    showSuccess('文档已删除')
    router.push({ name: 'home' })
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

async function saveMetadata() {
  const id = parseInt(route.params.id as string)
  try {
    doc.value = await updateDocument(id, form.value)
    showSuccess('元数据已保存')
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

function selectReplacePdf() {
  replacePdfInput.value?.click()
}

function selectReplaceMarkdown() {
  replaceMarkdownInput.value?.click()
}

function selectReplaceAsset() {
  replaceAssetInput.value?.click()
}

async function onReplacePdfSelected(e: Event) {
  const id = parseInt(route.params.id as string)
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  if (!file.name.toLowerCase().endsWith('.pdf')) {
    showError('请选择 PDF 文件')
    input.value = ''
    return
  }
  try {
    doc.value = await replacePdf(id, file)
    showSuccess('PDF 已替换')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    input.value = ''
  }
}

async function onReplaceMarkdownSelected(e: Event) {
  const id = parseInt(route.params.id as string)
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  if (!file.name.toLowerCase().endsWith('.md')) {
    showError('请选择 Markdown (.md) 文件')
    input.value = ''
    return
  }
  try {
    await replaceMarkdown(id, file)
    showSuccess('Markdown 已替换')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    input.value = ''
  }
}

async function onReplaceAssetSelected(e: Event) {
  const id = parseInt(route.params.id as string)
  const input = e.target as HTMLInputElement
  const file = input.files?.[0]
  if (!file) return
  if (!file.name.toLowerCase().endsWith('.zip')) {
    showError('请选择 ZIP 资源包文件')
    input.value = ''
    return
  }
  try {
    await replaceAsset(id, file)
    showSuccess('图片资源包已替换')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    input.value = ''
  }
}

async function enrichCrossref() {
  const id = parseInt(route.params.id as string)
  try {
    await crossrefDocument(id)
    showSuccess('Crossref 补充任务已提交')
    setTimeout(load, 2000)
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

async function enrichPdfMeta() {
  const id = parseInt(route.params.id as string)
  try {
    await extractPdfMeta(id)
    showSuccess('PDF 元数据提取任务已提交')
    setTimeout(load, 2000)
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

async function rehash() {
  const id = parseInt(route.params.id as string)
  try {
    const res = await rehashDocument(id)
    hashResult.value = res
    showSuccess('哈希已重算')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  }
}

watch(() => route.params.id, load)
onMounted(() => {
  loadVectorDbs()
  load()
})

function formatSize(bytes: number | null): string {
  if (!bytes) return '-'
  if (bytes < 1024) return bytes + ' B'
  if (bytes < 1024 * 1024) return (bytes / 1024).toFixed(1) + ' KB'
  return (bytes / (1024 * 1024)).toFixed(1) + ' MB'
}

function formatFile(size: number | null): string {
  if (size == null) return '不存在'
  return formatSize(size)
}
</script>

<style scoped>
.action-group {
  display: flex;
  align-items: center;
  gap: 16px;
  padding: 12px 0;
  border-bottom: 1px solid var(--border);
  flex-wrap: wrap;
}

.action-group:last-child {
  border-bottom: none;
  padding-bottom: 0;
}

.action-label {
  width: 90px;
  flex-shrink: 0;
  font-size: 13px;
  color: var(--text-secondary);
}

.action-buttons {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  flex: 1;
  align-items: center;
}

.index-control {
  display: flex;
  gap: 8px;
  align-items: center;
}

.select {
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 4px;
  background: var(--bg);
  color: var(--text);
  font-size: 14px;
}

.status-select {
  max-width: 180px;
}

.index-list {
  display: flex;
  flex-direction: column;
  gap: 6px;
}

.index-item {
  display: flex;
  flex-direction: column;
  gap: 2px;
  padding: 6px 10px;
  border-radius: 6px;
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  font-size: 13px;
}

.index-item.status-indexed {
  border-color: var(--success);
}

.index-item.status-indexing {
  border-color: var(--warning);
}

.index-item.status-error {
  border-color: var(--danger);
  background: var(--danger-bg);
}

.index-tag-text small {
  color: var(--text-secondary);
}

.index-error {
  font-size: 12px;
  color: var(--danger-text);
  line-height: 1.5;
  word-break: break-word;
  cursor: help;
}

.empty-hint {
  color: var(--text-secondary);
  font-size: 13px;
}

.action-group-danger .action-label {
  color: var(--danger);
}

.hash-result {
  font-size: 12px;
  color: var(--text-secondary);
  padding: 4px 8px;
  background: var(--bg-secondary);
  border-radius: 4px;
}

.file-sizes-list {
  display: flex;
  flex-direction: row;
  gap: 20px;
  flex-wrap: wrap;
}
.file-size-item {
  display: inline-flex;
  gap: 8px;
  align-items: center;
  font-size: 13px;
}
.file-label {
  font-weight: 600;
  color: var(--text-secondary);
  min-width: 80px;
}
.file-value {
  color: var(--text);
}

.doc-error-banner {
  margin-bottom: 16px;
  padding: 10px 14px;
  background: var(--danger-bg);
  border: 1px solid var(--danger);
  border-radius: 6px;
  color: var(--danger-text);
  font-size: 13px;
  display: flex;
  align-items: flex-start;
  gap: 8px;
}

.doc-error-banner .error-icon {
  flex-shrink: 0;
  font-size: 16px;
}

.form-row {
  display: flex;
  gap: 12px;
}

.form-row .form-group {
  flex: 1;
}

@media (max-width: 768px) {
  .action-group {
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
    padding: 10px 0;
  }

  .action-label {
    width: auto;
    font-size: 13px;
    min-width: 70px;
  }

  .action-buttons {
    width: 100%;
    flex-wrap: wrap;
    gap: 8px;
  }

  .index-control {
    width: 100%;
    flex-wrap: wrap;
    gap: 8px;
  }

  .select {
    flex: 1;
    min-width: 140px;
  }

  .form-row {
    flex-direction: column;
    gap: 0;
  }

  .form-row .form-group {
    width: 100%;
  }

  .detail-header {
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }

  .detail-header h2 {
    font-size: 16px;
  }

  .detail-header .btn {
    width: 100%;
  }
}

@media (max-width: 480px) {
  .action-group {
    gap: 8px;
  }

  .action-label {
    min-width: 60px;
    font-size: 12px;
  }
}
</style>
