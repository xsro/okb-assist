<template>
  <div class="doc-manage-view" v-if="doc">
    <div class="detail-header">
      <h2>文档管理: {{ doc.title }}</h2>
      <router-link :to="{ name: 'detail', params: { id: doc.id } }" class="btn btn-sm btn-outline">
        返回详情
      </router-link>
    </div>

    <!-- 文献操作 -->
    <div class="section">
      <h3>文献操作</h3>

      <!-- 流水线 -->
      <div class="action-group">
        <span class="action-label">流水线</span>
        <div class="action-buttons">
          <button class="btn" @click="runStage('parse')">解析</button>
          <button class="btn" @click="runStage('extract')">提取</button>
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
            <span
              v-for="idx in docIndexes"
              :key="idx.vector_db_id"
              class="index-tag"
              :class="`status-${idx.status}`"
            >
              {{ idx.vector_db_id }}
              <small>({{ idx.status }})</small>
            </span>
          </div>
        </div>
      </div>

      <!-- 元数据补全 -->
      <div class="action-group">
        <span class="action-label">元数据补全</span>
        <div class="action-buttons">
          <button class="btn btn-outline" @click="enrichPdfMeta">PDF 元数据</button>
          <button class="btn btn-outline" @click="enrichCrossref">Crossref</button>
        </div>
      </div>

      <!-- 文件管理 -->
      <div class="action-group">
        <span class="action-label">文件管理</span>
        <div class="action-buttons">
          <button class="btn btn-outline" @click="selectReplacePdf">替换 PDF</button>
          <input
            ref="replacePdfInput"
            type="file"
            accept=".pdf,application/pdf"
            style="display: none"
            @change="onReplacePdfSelected"
          />
        </div>
      </div>

      <!-- 危险操作 -->
      <div class="action-group action-group-danger">
        <span class="action-label">危险操作</span>
        <div class="action-buttons">
          <button class="btn btn-danger" @click="reset">重置</button>
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
import { useRoute } from 'vue-router'
import { getDocument, updateDocument, replacePdf } from '@/api/documents'
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
const { showSuccess, showError } = useToast()
const { requireToken } = useRequireToken()

const doc = ref<Document | null>(null)
const form = ref<Partial<Document>>({})
const replacePdfInput = ref<HTMLInputElement>()
const vectorDbs = ref<VectorDbConfig[]>([])
const selectedIndexDb = ref('')
const docIndexes = ref<DocumentIndexInfo[]>([])

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
  } catch {
    showError('加载失败')
  }
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
  } catch {
    showError('操作失败')
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
  } catch {
    showError('索引提交失败')
  }
}

async function reset() {
  const id = parseInt(route.params.id as string)
  if (!confirm('确定重置该文档状态？')) return
  try {
    await resetDocument(id)
    showSuccess('已重置')
    load()
  } catch {
    showError('重置失败')
  }
}

async function saveMetadata() {
  const id = parseInt(route.params.id as string)
  try {
    doc.value = await updateDocument(id, form.value)
    showSuccess('元数据已保存')
  } catch {
    showError('保存失败')
  }
}

function selectReplacePdf() {
  replacePdfInput.value?.click()
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
  } catch {
    showError('PDF 替换失败')
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
  } catch {
    showError('Crossref 补充失败')
  }
}

async function enrichPdfMeta() {
  const id = parseInt(route.params.id as string)
  try {
    await extractPdfMeta(id)
    showSuccess('PDF 元数据提取任务已提交')
    setTimeout(load, 2000)
  } catch {
    showError('PDF 元数据提取失败')
  }
}

watch(() => route.params.id, load)
onMounted(() => {
  loadVectorDbs()
  load()
})
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

.index-list {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
  align-items: center;
}

.index-tag {
  display: inline-flex;
  align-items: center;
  gap: 4px;
  padding: 4px 10px;
  border-radius: 4px;
  background: var(--bg-secondary);
  border: 1px solid var(--border);
  font-size: 13px;
}

.index-tag small {
  color: var(--text-secondary);
}

.index-tag.status-indexed {
  border-color: var(--success);
  color: var(--success);
}

.index-tag.status-indexing {
  border-color: var(--warning);
  color: var(--warning);
}

.index-tag.status-error {
  border-color: var(--danger);
  color: var(--danger);
}

.empty-hint {
  color: var(--text-secondary);
  font-size: 13px;
}

.action-group-danger .action-label {
  color: var(--danger);
}

.form-row {
  display: flex;
  gap: 12px;
}

.form-row .form-group {
  flex: 1;
}

@media (max-width: 640px) {
  .action-group {
    flex-direction: column;
    align-items: flex-start;
    gap: 10px;
  }

  .action-label {
    width: auto;
  }

  .index-control {
    width: 100%;
  }

  .select {
    flex: 1;
  }
}
</style>
