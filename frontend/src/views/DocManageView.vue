<template>
  <div class="doc-manage-view" v-if="doc">
    <div class="detail-header">
      <h2>文档管理: {{ doc.title }}</h2>
      <router-link :to="{ name: 'detail', params: { id: doc.id } }" class="btn btn-sm btn-outline">
        返回详情
      </router-link>
    </div>

    <!-- 流水线控制 -->
    <div class="section">
      <h3>处理流水线</h3>
      <div class="pipeline-steps">
        <div class="step" :class="{ active: doc.status === 'parsing', done: isDone('markdown_done') }">
          <span class="step-num">1</span>
          <span class="step-label">解析</span>
        </div>
        <div class="step-arrow">→</div>
        <div class="step" :class="{ active: doc.status === 'extracting', done: isDone('meta_done') }">
          <span class="step-num">2</span>
          <span class="step-label">提取</span>
        </div>
        <div class="step-arrow">→</div>
        <div class="step" :class="{ active: doc.status === 'indexing', done: isDone('indexed') }">
          <span class="step-num">3</span>
          <span class="step-label">索引</span>
        </div>
      </div>

      <div class="action-buttons">
        <button class="btn" @click="runStage('parse')">解析</button>
        <button class="btn" @click="runStage('extract')">提取</button>
        <button class="btn" @click="runStage('index')">索引</button>
        <button class="btn" @click="runStage('process')">全流程</button>
        <button class="btn btn-danger" @click="reset">重置</button>
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

    <!-- 外部操作 -->
    <div class="section">
      <h3>外部操作</h3>
      <div class="action-buttons">
        <button class="btn btn-outline" @click="enrichCrossref">Crossref 补充</button>
        <button class="btn btn-outline" @click="enrichPdfMeta">PDF 元数据提取</button>
      </div>
    </div>
  </div>

  <div v-else class="loading">加载中...</div>
</template>

<script setup lang="ts">
import { ref, onMounted, watch } from 'vue'
import { useRoute } from 'vue-router'
import { getDocument, updateDocument } from '@/api/documents'
import {
  parseDocument,
  extractDocument,
  indexDocument,
  processDocument,
  resetDocument
} from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document } from '@/types/document'

const route = useRoute()
const { showSuccess, showError, showInfo } = useToast()
const { requireToken } = useRequireToken()

const doc = ref<Document | null>(null)
const form = ref<Partial<Document>>({})

function isDone(status: string): boolean {
  const order = ['uploaded', 'parsing', 'markdown_done', 'extracting', 'meta_done', 'indexing', 'indexed']
  if (!doc.value) return false
  const currentIdx = order.indexOf(doc.value.status)
  const targetIdx = order.indexOf(status)
  return currentIdx >= targetIdx
}

async function load() {
  const id = parseInt(route.params.id as string)
  if (!requireToken()) return
  try {
    doc.value = await getDocument(id)
    form.value = { ...doc.value }
  } catch {
    showError('加载失败')
  }
}

async function runStage(stage: string) {
  const id = parseInt(route.params.id as string)
  try {
    if (stage === 'process') {
      await processDocument(id)
    } else if (stage === 'parse') {
      await parseDocument(id)
    } else if (stage === 'extract') {
      await extractDocument(id)
    } else if (stage === 'index') {
      await indexDocument(id)
    }
    showSuccess('已启动任务')
    setTimeout(load, 2000)
  } catch {
    showError('操作失败')
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

async function enrichCrossref() {
  showInfo('Crossref 补充功能开发中')
}

async function enrichPdfMeta() {
  showInfo('PDF 元数据提取功能开发中')
}

watch(() => route.params.id, load)
onMounted(load)
</script>

<style scoped>
.pipeline-steps {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-bottom: 16px;
}
.step {
  display: flex;
  flex-direction: column;
  align-items: center;
  padding: 8px 16px;
  border-radius: 8px;
  background: #f5f5f5;
  min-width: 80px;
}
.step.active {
  background: #e3f2fd;
  border: 2px solid var(--primary);
}
.step.done {
  background: #e8f5e9;
}
.step-num {
  font-size: 18px;
  font-weight: 700;
}
.step-label {
  font-size: 12px;
  color: var(--text-secondary);
}
.step-arrow {
  font-size: 20px;
  color: var(--text-secondary);
}
.action-buttons {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}
.form-row {
  display: flex;
  gap: 12px;
}
.form-row .form-group {
  flex: 1;
}
</style>