<template>
  <div class="doc-manage-view" v-if="doc">
    <div class="detail-header">
      <h2>文档管理: {{ doc.title || doc.filename || '(无标题)' }}</h2>
      <router-link :to="{ name: 'detail', params: { id: doc.id } }" class="btn btn-sm btn-outline">
        返回详情
      </router-link>
    </div>

    <!-- 文档错误横幅 -->
    <div v-if="derivedStatus?.status === 'error' && derivedStatus.detail" class="doc-error-banner">
      <span class="error-icon">⚠</span>
      <span>{{ derivedStatus.detail }}</span>
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
          <button
            class="btn"
            title="用大模型读 Markdown 逐字段校对（慢，但覆盖全）"
            @click="runStage('extract')"
          >从 Markdown（大模型）</button>
          <button
            class="btn btn-outline"
            title="只识别 Markdown 里的「引用格式 / 引用本文 / DOI」，几毫秒完成；没有这些线索会跳过"
            @click="runStage('extract-fast')"
          >从 Markdown（启发式）</button>
          <button class="btn btn-outline" @click="enrichPdfMeta">PDF 元数据</button>
          <button class="btn btn-outline" @click="enrichCrossref">Crossref</button>
        </div>
      </div>

      <!-- 文件管理 -->
      <div class="action-group">
        <span class="action-label">文件管理</span>
        <div class="action-buttons">
          <button class="btn btn-outline" @click="selectReplacePdf" :disabled="uploading">替换 PDF</button>
          <button class="btn btn-outline" @click="selectReplaceMarkdown" :disabled="uploading">替换 Markdown</button>
          <button class="btn btn-outline" @click="selectReplaceAsset" :disabled="uploading">替换图片资源包</button>
          <button class="btn btn-outline" @click="rehash" :disabled="uploading">重算哈希</button>
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
          <div v-if="uploading" class="upload-progress-wrap">
            <div class="upload-progress-bar">
              <div
                class="upload-progress-fill"
                :style="{ width: uploadProgress + '%' }"
              ></div>
            </div>
            <span class="upload-progress-text">
              {{ uploadLabel }} {{ uploadProgress }}%
            </span>
          </div>
        </div>
      </div>

      <!-- PDF 健康（仅 admin：与批量检查共用同一套判定） -->
      <div v-if="tokenStore.isAdmin" class="action-group">
        <span class="action-label">PDF 健康</span>
        <div class="action-buttons">
          <button class="btn btn-outline" :disabled="healthChecking" @click="checkHealth">
            {{ healthChecking ? '检查中…' : '检查 PDF 健康' }}
          </button>
          <span class="health-badge" :class="`health-${healthStatus}`">{{ healthLabel }}</span>
          <span v-if="healthMeta" class="health-meta">{{ healthMeta }}</span>
        </div>
        <p v-if="healthMessage" class="health-detail">{{ healthMessage }}</p>
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

      <!-- 文件事实（状态由这三列推导，不再提供「设置状态」） -->
      <div class="action-group">
        <span class="action-label">文件事实</span>
        <div class="action-buttons file-facts">
          <span class="file-fact">
            <span class="file-label">Markdown</span>
            <span class="file-fact-value" :class="doc.has_md ? 'ok' : 'muted'">
              {{ doc.has_md ? '存在' : '不存在' }}
            </span>
          </span>
          <span class="file-fact">
            <span class="file-label">PDF</span>
            <span class="file-fact-value" :class="doc.has_pdf ? 'ok' : 'muted'">
              {{ doc.has_pdf ? '存在' : '不存在' }}
            </span>
          </span>
          <span class="file-fact">
            <span class="file-label">同步</span>
            <span class="file-fact-value" :class="doc.synced ? 'ok' : 'warn'">
              {{ doc.synced ? (doc.fs_synced_at || '已同步') : '未同步' }}
            </span>
          </span>
          <button class="btn btn-sm btn-outline" @click="load()">重新同步</button>
        </div>
      </div>

      <!-- 清空解析产物（唯一的「重新解析」通道） -->
      <div class="action-group">
        <span class="action-label">清空解析产物</span>
        <div class="action-buttons">
          <button class="btn btn-warning" @click="clearParseOutputClick">删除 Markdown + 资源包</button>
        </div>
        <p class="action-hint">状态由文件事实推导，删除后即回到「待解析」；此操作不可撤销。</p>
      </div>

      <!-- 危险操作 -->
      <div class="action-group action-group-danger">
        <span class="action-label">危险操作</span>
        <div class="action-buttons">
          <button class="btn btn-danger" @click="removeDocument">删除文档</button>
        </div>
      </div>
    </div>

    <!-- 元数据编辑：覆盖 documents 的全部 16 个元数据列（文件事实列不在此处） -->
    <div class="section">
      <h3>编辑元数据</h3>
      <p class="form-hint">留空即清空该字段；DOI 会自动去掉 <code>https://doi.org/</code> 等前缀，文档类型会映射到 Zotero 标准类型名。</p>

      <h4 class="form-subtitle">中文</h4>
      <div class="form-group">
        <label>标题</label>
        <input v-model="form.title" type="text" />
      </div>
      <div class="form-group">
        <label>作者</label>
        <input v-model="form.authors" type="text" placeholder="多位作者用 ; 或 , 分隔" />
      </div>
      <div class="form-group">
        <label>期刊</label>
        <input v-model="form.journal" type="text" />
      </div>
      <div class="form-group">
        <label>关键词</label>
        <input v-model="form.keywords" type="text" placeholder="多个关键词用 ; 分隔" />
      </div>
      <div class="form-group">
        <label>摘要</label>
        <textarea v-model="form.abstract" rows="4"></textarea>
      </div>

      <h4 class="form-subtitle">英文</h4>
      <div class="form-group">
        <label>英文标题</label>
        <input v-model="form.title_en" type="text" />
      </div>
      <div class="form-group">
        <label>英文作者</label>
        <input v-model="form.authors_en" type="text" />
      </div>
      <div class="form-group">
        <label>英文期刊</label>
        <input v-model="form.journal_en" type="text" />
      </div>
      <div class="form-group">
        <label>英文关键词</label>
        <input v-model="form.keywords_en" type="text" />
      </div>
      <div class="form-group">
        <label>英文摘要</label>
        <textarea v-model="form.abstract_en" rows="3"></textarea>
      </div>

      <h4 class="form-subtitle">书目</h4>
      <div class="form-row">
        <div class="form-group">
          <label>年份</label>
          <input v-model.number="form.year" type="number" min="1000" max="2200" placeholder="留空 = 清空" />
        </div>
        <div class="form-group">
          <label>文档类型</label>
          <select v-model="form.doc_type" class="doc-type-select">
            <option value="">（未设置）</option>
            <!-- 历史遗留值不在标准词表里时单独列出来，避免打开编辑页就被悄悄改掉 -->
            <option v-if="legacyDocType" :value="legacyDocType">
              {{ legacyDocType }}（历史值，保存后归一为 {{ docTypeLabel(normalizeDocType(legacyDocType)) }}）
            </option>
            <option v-for="opt in docTypeOptions" :key="opt.value" :value="opt.value">
              {{ opt.label }}（{{ opt.value }}）
            </option>
          </select>
          <p class="form-hint">候选来自后端标准词表（Zotero 类型名）；填其他值后端会归一化到最接近的候选。</p>
        </div>
        <div class="form-group">
          <label>DOI</label>
          <input v-model="form.doi" type="text" />
        </div>
      </div>
      <div class="form-row">
        <div class="form-group">
          <label>来源（出版者 / 学位授予单位）</label>
          <input v-model="form.source" type="text" />
        </div>
        <div class="form-group">
          <label>分类</label>
          <input v-model="form.category" type="text" />
        </div>
        <div class="form-group">
          <label>语言</label>
          <input v-model="form.language" type="text" list="language-options" placeholder="zh / en" />
          <datalist id="language-options">
            <option v-for="l in LANGUAGE_OPTIONS" :key="l" :value="l" />
          </datalist>
        </div>
      </div>

      <button class="btn" @click="saveMetadata" :disabled="saving">{{ saving ? '保存中…' : '保存' }}</button>
    </div>

  </div>

  <div v-else class="loading">加载中...</div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch, computed } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import {
  getDocument,
  getDocTypes,
  updateDocument,
  replacePdf,
  replaceMarkdown,
  replaceAsset,
  deleteDocument,
  rehashDocument
} from '@/api/documents'
import { getServiceConfig } from '@/api/config'
import {
  parseDocument,
  extractDocument,
  indexDocument,
  getDocumentIndexes,
  clearParseOutput,
  crossrefDocument,
  extractPdfMeta
} from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import { useTokenStore } from '@/stores/token'
import { checkDocumentPdfHealth } from '@/api/admin'
import type { Document } from '@/types/document'
import { deriveDocStatus } from '@/utils/docStatus'
import {
  DOC_TYPE_OPTIONS,
  normalizeDocType,
  docTypeLabel,
  type DocTypeOption
} from '@/utils/docType'
import type { VectorDbConfig } from '@/types/config'
import type { DocumentIndexInfo } from '@/types/pipeline'

const route = useRoute()
const router = useRouter()
const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()
const tokenStore = useTokenStore()

const doc = ref<Document | null>(null)

/** 状态由前端推导（后端只给文件事实与运行时字段） */
const derivedStatus = computed(() => (doc.value ? deriveDocStatus(doc.value) : null))
const form = ref<Partial<Document>>({})
const saving = ref(false)
/** 文档类型候选：先取前端静态词表，挂载后用后端 `/documents/doc-types` 的 `standard` 覆盖 */
const docTypeOptions = ref<DocTypeOption[]>([...DOC_TYPE_OPTIONS])
/** 当前值不在标准词表里（历史脏值）时单独展示，避免用户以为丢了数据 */
const legacyDocType = computed(() => {
  const value = (form.value.doc_type || '').trim()
  if (!value) return ''
  return docTypeOptions.value.some((o) => o.value === value) ? '' : value
})
const LANGUAGE_OPTIONS = ['zh', 'en', 'ja', 'ko', 'fr', 'de', 'ru']
const replacePdfInput = ref<HTMLInputElement>()
const replaceMarkdownInput = ref<HTMLInputElement>()
const replaceAssetInput = ref<HTMLInputElement>()
const vectorDbs = ref<VectorDbConfig[]>([])
const selectedIndexDb = ref('')
const docIndexes = ref<DocumentIndexInfo[]>([])
const hashResult = ref<{ file_hash: string; old_file_hash: string | null } | null>(null)

// PDF 健康检查（单篇，admin）
const healthChecking = ref(false)
const healthResult = ref<{
  status: string
  message: string
  page_count: number | null
  engine: string
} | null>(null)

/** 状态值：ok / warning / error / missing；未检查时为 unchecked */
const healthStatus = computed(
  () => healthResult.value?.status || doc.value?.pdf_health || 'unchecked'
)
const healthLabel = computed(() => {
  const labels: Record<string, string> = {
    ok: '✓ 正常',
    warning: '⚠ 警告',
    error: '✕ 异常',
    missing: '✕ 缺失',
    unchecked: '未检查'
  }
  return labels[healthStatus.value] || healthStatus.value
})
/** 本次检查后的附加信息（引擎 + 页数） */
const healthMeta = computed(() => {
  const r = healthResult.value
  if (!r) return ''
  const parts: string[] = []
  if (r.page_count != null) parts.push(`${r.page_count} 页`)
  if (r.engine) parts.push(`引擎：${r.engine}`)
  return parts.join(' · ')
})
const healthMessage = computed(() => healthResult.value?.message || doc.value?.pdf_health_message || '')

async function checkHealth() {
  const id = parseInt(route.params.id as string)
  healthChecking.value = true
  try {
    const res = await checkDocumentPdfHealth(id)
    healthResult.value = res
    if (doc.value) {
      doc.value.pdf_health = res.status
      doc.value.pdf_health_message = res.message
    }
    const ok = res.status === 'ok'
    if (ok) showSuccess(`PDF 健康：正常（${res.page_count ?? '-'} 页）`)
    else showError(`PDF 健康：${res.status} — ${res.message}`)
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    healthChecking.value = false
  }
}

// 上传进度状态
const uploading = ref(false)
const uploadLabel = ref('')
const uploadProgress = ref(0)

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
    healthResult.value = null
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
      showSuccess('已提交解析任务')
    } else if (stage === 'extract') {
      await extractDocument(id)
      showSuccess('已提交「大模型」元数据提取任务')
    } else if (stage === 'extract-fast') {
      await extractDocument(id, 'heuristic')
      showSuccess('已提交「启发式」元数据抽取任务（只识别引用格式 / DOI）')
    } else {
      showSuccess('已启动任务')
    }
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

async function clearParseOutputClick() {
  const id = parseInt(route.params.id as string)
  if (!confirm('确定删除该文档的 Markdown 与资源包？（删除后状态回到「待解析」，不可撤销）')) return
  if (!confirm('再次确认：此操作不可撤销。')) return
  try {
    const res = await clearParseOutput(id)
    showSuccess(`已清空解析产物（Markdown: ${res.removed_markdown ? '删除' : '无'}，资源包: ${res.removed_asset ? '删除' : '无'}）`)
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
  if (saving.value) return
  saving.value = true
  try {
    // 后端返回更新后的完整文档，直接赋回即可（旧实现把 {status:"ok"} 当成 Document，保存后整页字段变空）
    const updated = await updateDocument(id, buildMetadataPayload())
    if (updated && typeof updated.id === 'number') {
      doc.value = updated
      form.value = { ...updated }
      await loadIndexes()
    } else {
      // 兼容旧后端（只回 {status:"ok"}）：重新拉一次，保证页面状态与数据库一致
      await load()
    }
    showSuccess('元数据已保存')
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    saving.value = false
  }
}

/**
 * 只提交 16 个元数据字段（白名单）。
 *
 * 不再把整份 `DocumentOut`（含 id / 文件事实 / 健康状态等）发回后端：
 * 那些是非元数据列，混在里面只会让人误以为能改。
 * 空串/空值 = 清空该字段（后端把空串写为 NULL）。
 */
function buildMetadataPayload(): Partial<Document> {
  const f = form.value
  const text = (v: unknown) => (v === null || v === undefined ? '' : String(v))
  return {
    title: text(f.title),
    authors: text(f.authors),
    journal: text(f.journal),
    keywords: text(f.keywords),
    abstract: text(f.abstract),
    title_en: text(f.title_en),
    authors_en: text(f.authors_en),
    journal_en: text(f.journal_en),
    keywords_en: text(f.keywords_en),
    abstract_en: text(f.abstract_en),
    year: yearPayload(f.year),
    doc_type: text(f.doc_type),
    doi: text(f.doi),
    source: text(f.source),
    category: text(f.category),
    language: text(f.language)
  }
}

/** 年份：`<input type="number">` 清空后是空串，必须转成 null（否则后端反序列化整次请求 422） */
function yearPayload(v: unknown): number | null {
  if (v === null || v === undefined) return null
  const s = String(v).trim()
  if (s === '') return null
  const n = Number(s)
  return Number.isFinite(n) ? Math.trunc(n) : null
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
  uploading.value = true
  uploadLabel.value = '上传 PDF'
  uploadProgress.value = 0
  try {
    doc.value = await replacePdf(id, file, (p) => { uploadProgress.value = p })
    showSuccess('PDF 已替换')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    uploading.value = false
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
  uploading.value = true
  uploadLabel.value = '上传 Markdown'
  uploadProgress.value = 0
  try {
    await replaceMarkdown(id, file, (p) => { uploadProgress.value = p })
    showSuccess('Markdown 已替换')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    uploading.value = false
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
  uploading.value = true
  uploadLabel.value = '上传资源包'
  uploadProgress.value = 0
  try {
    await replaceAsset(id, file, (p) => { uploadProgress.value = p })
    showSuccess('图片资源包已替换')
    load()
  } catch (err) {
    showError(getErrorMessage(err))
  } finally {
    uploading.value = false
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
  loadDocTypeOptions()
  load()
})

/** 用后端下发的标准词表覆盖静态候选（拉不到就继续用静态表，不影响编辑） */
async function loadDocTypeOptions() {
  try {
    const res = await getDocTypes()
    if (Array.isArray(res?.standard) && res.standard.length) {
      docTypeOptions.value = res.standard
    }
  } catch {
    /* 词表接口失败不阻塞页面：静态候选与后端一致 */
  }
}

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

.file-facts {
  display: flex;
  flex-wrap: wrap;
  gap: 14px;
  align-items: center;
}

.file-fact {
  display: inline-flex;
  gap: 6px;
  align-items: center;
  font-size: 13px;
}

.file-fact-value.ok {
  color: var(--success-text);
}

.file-fact-value.muted {
  color: var(--text-secondary);
}

.file-fact-value.warn {
  color: var(--warning-text);
}

.action-hint {
  margin: 6px 0 0;
  font-size: 12px;
  color: var(--text-secondary);
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

/* ── PDF 健康 ── */
.health-badge {
  display: inline-block;
  padding: 2px 8px;
  border-radius: 10px;
  font-size: 12px;
  font-weight: 600;
  white-space: nowrap;
  border: 1px solid transparent;
}
.health-badge.health-ok {
  color: #1b7f4b;
  background: #e8f7ee;
  border-color: #b7e2c9;
}
.health-badge.health-warning {
  color: #9a6400;
  background: #fff6e0;
  border-color: #f0d69b;
}
.health-badge.health-error,
.health-badge.health-missing {
  color: var(--danger-text);
  background: #fdecec;
  border-color: #f3c1c1;
}
.health-badge.health-unchecked {
  color: var(--text-secondary);
  background: var(--bg-secondary);
  border-color: var(--border);
}
.health-meta {
  font-size: 12px;
  color: var(--text-secondary);
}
.health-detail {
  flex: 1 0 100%;
  margin: 0 0 0 106px;
  font-size: 12px;
  line-height: 1.6;
  color: var(--text-secondary);
  word-break: break-word;
}

@media (max-width: 768px) {
  .health-detail {
    margin-left: 0;
  }
}

.upload-progress-wrap {
  display: flex;
  align-items: center;
  gap: 10px;
  flex: 1;
  min-width: 180px;
}

.upload-progress-bar {
  flex: 1;
  height: 8px;
  background: var(--bg-secondary);
  border-radius: 4px;
  overflow: hidden;
}

.upload-progress-fill {
  height: 100%;
  background: var(--primary);
  border-radius: 4px;
  transition: width 0.2s ease;
}

.upload-progress-text {
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
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

.form-group select {
  width: 100%;
  padding: 8px 10px;
  border: 1px solid var(--border-color, #d1d5db);
  border-radius: 6px;
  background: var(--bg-card, #fff);
  color: var(--text-primary, #111827);
  font-size: 14px;
}

.form-group .form-hint {
  margin: 6px 0 0;
}

.form-hint {
  margin: 0 0 12px;
  color: var(--text-muted, #6b7280);
  font-size: 12px;
  line-height: 1.6;
}

.form-hint code {
  padding: 1px 4px;
  border-radius: 3px;
  background: var(--bg-subtle, #f3f4f6);
}

/* 元数据分组小标题（中文 / 英文 / 书目） */
.form-subtitle {
  margin: 16px 0 8px;
  font-size: 13px;
  font-weight: 600;
  color: var(--text-muted, #6b7280);
  border-bottom: 1px solid var(--border, #e5e7eb);
  padding-bottom: 4px;
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

  .upload-progress-wrap {
    width: 100%;
    min-width: 0;
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
