<template>
  <div class="tools-view">
    <h2>工具面板</h2>

    <div class="tools-tabs">
      <button
        v-for="tab in tabs"
        :key="tab.key"
        class="tab"
        :class="{ active: activeTab === tab.key }"
        @click="activeTab = tab.key"
      >
        {{ tab.label }}
      </button>
    </div>

    <!-- 全文搜索 -->
    <div v-if="activeTab === 'grep'" class="tab-content">
      <div class="search-input">
        <input
          v-model="grepPattern"
          type="text"
          placeholder="输入正则表达式..."
          @keyup.enter="doGrepSearch"
        />
        <button class="btn" @click="doGrepSearch">搜索</button>
      </div>

      <div class="advanced-toggle">
        <button class="btn btn-sm btn-outline" @click="showGrepAdvanced = !showGrepAdvanced">
          {{ showGrepAdvanced ? '收起高级选项' : '高级选项' }}
        </button>
      </div>

      <div v-if="showGrepAdvanced" class="advanced-options">
        <div class="option-row">
          <div class="form-group">
            <label>结果数量</label>
            <input v-model.number="grepLimit" type="number" min="1" max="1000" />
          </div>
          <div class="form-group">
            <label>上下文行数</label>
            <input v-model.number="grepContext" type="number" min="0" max="10" />
          </div>
          <div class="form-group">
            <label>算法</label>
            <select v-model="grepAlgorithm">
              <option value="full">完整</option>
              <option value="fast">快速</option>
            </select>
          </div>
          <label class="checkbox-option">
            <input v-model="grepRegex" type="checkbox" />
            <span>正则表达式</span>
          </label>
        </div>
        <div class="form-group doc-ids-group">
          <label>限定文档 ID（逗号分隔，可选）</label>
          <input
            v-model="grepDocIds"
            type="text"
            placeholder="例如：1,3,7"
          />
        </div>
      </div>

      <div v-if="grepResults" class="search-results">
        <h4>找到 {{ grepResults.length }} 个结果</h4>
        <div v-for="r in grepResults" :key="`${r.id}-${r.snippet}`" class="result-item">
          <div class="result-header">
            <router-link :to="{ name: 'detail', params: { id: r.id } }">
              {{ r.title }}
            </router-link>
            <span class="doc-id">ID: {{ r.id }}</span>
          </div>
          <p v-if="r.snippet" class="snippet" v-html="highlightSnippet(r.snippet, grepPattern)" />
        </div>
        <div v-if="grepResults.length === 0" class="empty-hint">无匹配结果</div>
      </div>
    </div>

    <!-- 语义搜索 -->
    <div v-if="activeTab === 'semantic'" class="tab-content">
      <div class="search-input">
        <input
          v-model="semanticQuery"
          type="text"
          placeholder="用自然语言描述你想找的文献..."
          @keyup.enter="doSemanticSearch"
        />
        <button class="btn" @click="doSemanticSearch">搜索</button>
      </div>
      <div v-if="semanticResults" class="search-results">
        <h4>找到 {{ semanticResults.length }} 个结果</h4>
        <div v-for="r in semanticResults" :key="r.id" class="result-item">
          <router-link :to="{ name: 'detail', params: { id: r.id } }">
            {{ r.title }}
          </router-link>
          <span class="score">相似度: {{ r.score.toFixed(3) }}</span>
        </div>
        <div v-if="semanticResults.length === 0" class="empty-hint">无匹配结果</div>
      </div>
    </div>

    <!-- MCP 配置 -->
    <div v-if="activeTab === 'mcp'" class="tab-content">
      <McpConfigPanel />
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref } from 'vue'
import { grepSearch, semanticSearch } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import McpConfigPanel from '@/components/McpConfigPanel.vue'
import type { SearchResult } from '@/types/document'

const { showError } = useToast()
const { requireToken } = useRequireToken()

const tabs = [
  { key: 'grep', label: '全文搜索' },
  { key: 'semantic', label: '语义搜索' },
  { key: 'mcp', label: 'MCP 配置' }
]
const activeTab = ref('grep')

const grepPattern = ref('')
const grepResults = ref<SearchResult[] | null>(null)
const showGrepAdvanced = ref(false)
const grepLimit = ref(20)
const grepContext = ref(2)
const grepAlgorithm = ref<'full' | 'fast'>('full')
const grepRegex = ref(true)
const grepDocIds = ref('')

const semanticQuery = ref('')
const semanticResults = ref<SearchResult[] | null>(null)

function parseDocIds(raw: string): number[] | undefined {
  const ids = raw
    .split(',')
    .map((s) => parseInt(s.trim()))
    .filter((n) => !isNaN(n) && n > 0)
  return ids.length > 0 ? ids : undefined
}

function highlightSnippet(snippet: string, pattern: string): string {
  if (!pattern.trim() || !grepRegex.value) return snippet
  try {
    const regex = new RegExp(`(${pattern.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')})`, 'gi')
    return snippet.replace(regex, '<mark>$1</mark>')
  } catch {
    return snippet
  }
}

async function doGrepSearch() {
  if (!requireToken() || !grepPattern.value.trim()) return
  try {
    const res = await grepSearch(grepPattern.value.trim(), {
      limit: grepLimit.value,
      context: grepContext.value,
      algorithm: grepAlgorithm.value,
      regex: grepRegex.value,
      docIds: parseDocIds(grepDocIds.value)
    })
    grepResults.value = res.results
  } catch {
    showError('搜索失败')
  }
}

async function doSemanticSearch() {
  if (!requireToken() || !semanticQuery.value.trim()) return
  try {
    const res = await semanticSearch(semanticQuery.value.trim())
    semanticResults.value = res.results
  } catch {
    showError('搜索失败')
  }
}
</script>

<style scoped>
.tools-tabs {
  display: flex;
  gap: 4px;
  margin-bottom: 20px;
  border-bottom: 1px solid var(--border);
}
.tab {
  padding: 10px 20px;
  border: none;
  background: transparent;
  cursor: pointer;
  font-size: 14px;
  color: var(--text-secondary);
  border-bottom: 2px solid transparent;
  transition: all 0.15s;
}
.tab.active {
  color: var(--primary);
  border-bottom-color: var(--primary);
}
.search-input {
  display: flex;
  gap: 12px;
  margin-bottom: 12px;
}
.search-input input {
  flex: 1;
  padding: 10px 16px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
}
.advanced-toggle {
  margin-bottom: 12px;
}
.advanced-options {
  background: #f9f9f9;
  border: 1px solid var(--border);
  border-radius: 8px;
  padding: 16px;
  margin-bottom: 20px;
}
.option-row {
  display: flex;
  gap: 16px;
  align-items: flex-end;
  flex-wrap: wrap;
}
.form-group {
  display: flex;
  flex-direction: column;
  gap: 6px;
}
.form-group label {
  font-size: 13px;
  color: var(--text-secondary);
}
.form-group input,
.form-group select {
  padding: 6px 10px;
  border: 1px solid var(--border);
  border-radius: 4px;
  font-size: 13px;
  background: #fff;
}
.checkbox-option {
  display: flex;
  align-items: center;
  gap: 6px;
  font-size: 13px;
  cursor: pointer;
}
.doc-ids-group {
  margin-top: 12px;
}
.doc-ids-group input {
  width: 100%;
  max-width: 400px;
}
.search-results h4 {
  margin-bottom: 12px;
}
.result-item {
  padding: 12px 0;
  border-bottom: 1px solid var(--border);
}
.result-header {
  display: flex;
  align-items: center;
  gap: 12px;
  flex-wrap: wrap;
}
.result-header a {
  font-weight: 500;
  font-size: 15px;
}
.doc-id {
  font-size: 12px;
  color: var(--text-secondary);
}
.snippet {
  margin-top: 8px;
  font-size: 13px;
  color: var(--text-secondary);
  line-height: 1.6;
  white-space: pre-wrap;
}
.snippet :deep(mark) {
  background: #ffeb3b;
  padding: 0 2px;
  border-radius: 2px;
}
.score {
  font-size: 12px;
  color: var(--primary);
  margin-left: 8px;
}
.empty-hint {
  text-align: center;
  padding: 40px;
  color: var(--text-secondary);
  font-size: 14px;
}
</style>
