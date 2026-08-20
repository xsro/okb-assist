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
        <input v-model="grepPattern" type="text" placeholder="输入正则表达式..." />
        <button class="btn" @click="doGrepSearch">搜索</button>
      </div>
      <div v-if="grepResults" class="search-results">
        <h4>找到 {{ grepResults.length }} 个结果</h4>
        <div v-for="r in grepResults" :key="r.id" class="result-item">
          <router-link :to="{ name: 'detail', params: { id: r.id } }">
            {{ r.title }}
          </router-link>
          <p v-if="r.snippet" class="snippet">{{ r.snippet }}</p>
        </div>
      </div>
    </div>

    <!-- 语义搜索 -->
    <div v-if="activeTab === 'semantic'" class="tab-content">
      <div class="search-input">
        <input v-model="semanticQuery" type="text" placeholder="用自然语言描述你想找的文献..." />
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
      </div>
    </div>

    <!-- 元数据搜索 -->
    <div v-if="activeTab === 'metadata'" class="tab-content">
      <div class="search-input">
        <input v-model="metaQuery" type="text" placeholder="搜索标题、作者、期刊、关键词、摘要..." />
        <button class="btn" @click="doMetaSearch">搜索</button>
      </div>
      <div v-if="metaResults" class="search-results">
        <h4>找到 {{ metaResults.length }} 个结果</h4>
        <div v-for="r in metaResults" :key="r.id" class="result-item">
          <router-link :to="{ name: 'detail', params: { id: r.id } }">
            {{ r.title }}
          </router-link>
        </div>
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
import { grepSearch, semanticSearch, searchInfo } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import McpConfigPanel from '@/components/McpConfigPanel.vue'
import type { SearchResult, Document } from '@/types/document'

const { showError } = useToast()
const { requireToken } = useRequireToken()

const tabs = [
  { key: 'grep', label: '全文搜索' },
  { key: 'semantic', label: '语义搜索' },
  { key: 'metadata', label: '元数据搜索' },
  { key: 'mcp', label: 'MCP 配置' }
]
const activeTab = ref('grep')

const grepPattern = ref('')
const grepResults = ref<SearchResult[] | null>(null)
const semanticQuery = ref('')
const semanticResults = ref<SearchResult[] | null>(null)
const metaQuery = ref('')
const metaResults = ref<Document[] | null>(null)

async function doGrepSearch() {
  if (!requireToken() || !grepPattern.value.trim()) return
  try {
    const res = await grepSearch(grepPattern.value.trim())
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

async function doMetaSearch() {
  if (!requireToken() || !metaQuery.value.trim()) return
  try {
    const res = await searchInfo(metaQuery.value.trim())
    metaResults.value = res.results
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
  margin-bottom: 20px;
}
.search-input input {
  flex: 1;
  padding: 10px 16px;
  border: 1px solid var(--border);
  border-radius: 6px;
  font-size: 14px;
}
.search-results h4 {
  margin-bottom: 12px;
}
.result-item {
  padding: 12px 0;
  border-bottom: 1px solid var(--border);
}
.result-item a {
  font-weight: 500;
  font-size: 15px;
}
.snippet {
  margin-top: 4px;
  font-size: 13px;
  color: var(--text-secondary);
}
.score {
  font-size: 12px;
  color: var(--primary);
  margin-left: 8px;
}
</style>