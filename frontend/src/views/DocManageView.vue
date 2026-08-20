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
      <div class="action-buttons">
        <button class="btn" @click="runStage('parse')">解析</button>
        <button class="btn" @click="runStage('index')">索引</button>
  
        元数据提取：
        <button class="btn btn-outline" @click="enrichPdfMeta">从 PDF 元数据</button>
        <button class="btn btn-outline" @click="enrichCrossref">从 Crossref </button>
        <button class="btn" @click="runStage('extract')">从解析的 markdown 文件</button>
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
  resetDocument,
  crossrefDocument,
  extractPdfMeta
} from '@/api/pipeline'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document } from '@/types/document'

const route = useRoute()
const { showSuccess, showError, showInfo } = useToast()
const { requireToken } = useRequireToken()

const doc = ref<Document | null>(null)
const form = ref<Partial<Document>>({})

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
onMounted(load)
</script>

<style scoped>
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