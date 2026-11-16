<template>
  <div class="doi-resolve">
    <div v-if="state === 'loading'" class="loading">正在按 DOI 定位文献…</div>

    <div v-else class="resolve-card">
      <h2>{{ headline }}</h2>
      <p v-if="doi" class="doi-text">{{ doi }}</p>
      <div class="resolve-actions">
        <a
          v-if="doi"
          :href="doiOrgUrl"
          target="_blank"
          rel="noopener noreferrer"
          class="btn btn-sm btn-outline"
        >在 doi.org 打开</a>
        <router-link
          v-if="doi"
          :to="{ path: '/assist', query: { q: `doi:${doi}` } }"
          class="btn btn-sm btn-outline"
        >按 DOI 检索</router-link>
        <router-link to="/assist" class="btn btn-sm">返回文献列表</router-link>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { useRoute, useRouter } from 'vue-router'
import { resolveDocIdByDoi } from '@/api/documents'

const route = useRoute()
const router = useRouter()

const state = ref<'loading' | 'invalid' | 'notfound' | 'failed'>('loading')
const failMessage = ref('')

// DOI 常被人整段粘贴成 https://doi.org/... 或 doi:...，先压平再查库与展示
// （后端 normalize_doi 也会剥前缀，这里只是为了检索式和提示里不带噪声）。
const raw = String(route.params.doi || '')
const doi = raw
  .replace(/^https?:\/\/(dx\.)?doi\.org\//i, '')
  .replace(/^doi:\s*/i, '')
  .trim()

// DOI 自带的 `/` 是合法路径字符，encodeURI 只处理空格、`#` 这类真正需要转义的
const doiOrgUrl = computed(() => `https://doi.org/${encodeURI(doi)}`)

const headline = computed(() => {
  if (state.value === 'invalid') return '链接里没有 DOI'
  if (state.value === 'notfound') return '库中没有该 DOI 对应的文献'
  if (state.value === 'failed') return failMessage.value
  return ''
})

/** DOI 短链 → 文献详情页；解析成功后用 replace，短链地址不留进历史 */
async function resolve() {
  if (!doi) {
    state.value = 'invalid'
    return
  }
  try {
    const res = await resolveDocIdByDoi(doi)
    if (res.found && res.id != null) {
      await router.replace({ name: 'detail', params: { id: res.id } })
      return
    }
    state.value = 'notfound'
  } catch (e) {
    const status = (e as { response?: { status?: number } }).response?.status
    // 401 已由全局拦截器接管理（跳登录页，登录后回跳本短链）
    if (status !== 401) {
      failMessage.value = status === 400 ? 'DOI 格式无效' : '解析失败，请稍后重试'
      state.value = 'failed'
    }
  }
}

onMounted(resolve)
</script>

<style scoped>
.resolve-card {
  max-width: 640px;
  margin: 32px auto;
  padding: 24px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
}

.resolve-card h2 {
  font-size: 18px;
  margin-bottom: 8px;
}

.doi-text {
  color: var(--text-secondary);
  word-break: break-all;
}

.resolve-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-top: 16px;
}

.loading {
  padding: 32px;
  text-align: center;
  color: var(--text-secondary);
}
</style>
