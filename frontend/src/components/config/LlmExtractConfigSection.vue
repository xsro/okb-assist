<template>
  <div class="section">
    <div class="llm-card">
      <div class="card-header-collapsible" @click="expanded = !expanded">
        <div class="card-summary">
          <span class="collapse-arrow" :class="{ expanded }">▶</span>
          <h3>信息补全模型</h3>
          <span class="protocol-badge">{{ protocolLabel(effective.protocol) }}</span>
          <span class="llm-model-preview">{{ effective.model }}</span>
          <span class="llm-url-preview">{{ effective.url }}</span>
          <span class="status-dot-small" :class="statusClass" :title="statusText"></span>
        </div>
        <button class="btn btn-sm btn-ghost" @click.stop="expanded = !expanded">
          {{ expanded ? '收起' : '展开' }}
        </button>
      </div>

      <div v-show="expanded" class="card-body-collapsible">
        <p class="form-hint">
          元数据抽取（信息补全）使用此模型。留空的字段从下方选中的模型池条目继承。
        </p>
        <div class="form-group">
          <label>模型来源</label>
          <select v-model="extract.from" @change="emitUpdate">
            <option value="">模型池第一项</option>
            <option v-for="model in models" :key="model.name" :value="model.name">
              {{ model.name || '（未命名）' }} — {{ model.model }}
            </option>
          </select>
          <p class="form-hint">当前继承自：{{ inheritedLabel }}</p>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>协议</label>
            <select v-model="extract.protocol" @change="emitUpdate">
              <option value="">继承（{{ protocolLabel(inherited.protocol) }}）</option>
              <option value="ollama">Ollama（原生协议）</option>
              <option value="openai">OpenAI（兼容协议）</option>
            </select>
          </div>
          <div class="form-group">
            <label>模型</label>
            <input v-model="extract.model" type="text" :placeholder="`继承：${inherited.model}`" @input="emitUpdate" />
          </div>
        </div>
        <div class="form-row">
          <div class="form-group">
            <label>URL</label>
            <input v-model="extract.url" type="text" :placeholder="`继承：${inherited.url}`" @input="emitUpdate" />
          </div>
          <div class="form-group">
            <label>API Key</label>
            <div class="input-with-toggle">
              <input v-model="extract.key" :type="keyVisible ? 'text' : 'password'" :placeholder="keyPlaceholder" @input="emitUpdate" />
              <button class="toggle-visibility" type="button" @click="keyVisible = !keyVisible" :title="keyVisible ? '隐藏 Key' : '显示 Key'">
                <svg v-if="keyVisible" width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M17.94 17.94A10.07 10.07 0 0 1 12 20c-7 0-11-8-11-8a18.45 18.45 0 0 1 5.06-5.94M9.9 4.24A9.12 9.12 0 0 1 12 4c7 0 11 8 11 8a18.5 18.5 0 0 1-2.16 3.19m-6.72-1.07a3 3 0 1 1-4.24-4.24"/><line x1="1" y1="1" x2="23" y2="23"/></svg>
              <svg v-else width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"><path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z"/><circle cx="12" cy="12" r="3"/></svg>
              </button>
            </div>
          </div>
        </div>
        <div class="effective-row">
          当前生效：{{ protocolLabel(effective.protocol) }} · {{ effective.url }} · {{ effective.model }}
        </div>
        <div class="status-test-row">
          <span class="status-dot" :class="statusClass"></span>
          <span class="status-text">{{ statusText }}</span>
          <button class="btn btn-sm btn-outline" @click="test" :disabled="testing">
            {{ testing ? '测试中...' : '测试' }}
          </button>
        </div>
      </div>
    </div>
  </div>
</template>

<script setup lang="ts">
import { computed, ref } from 'vue'
import type { LlmExtractConfig, LlmModelEntry, ModelEndpoint } from '@/types/config'
import { testService } from '@/api/config'
import { useToast } from '@/composables/useToast'

const { showError, showToast } = useToast()

const props = defineProps<{
  extract: LlmExtractConfig
  models: LlmModelEntry[]
}>()

const emit = defineEmits<{
  (e: 'update'): void
}>()

function emitUpdate() {
  emit('update')
}

const expanded = ref(false)
const testing = ref(false)
const keyVisible = ref(false)
const localStatus = ref<{ status: string; detail: string } | null>(null)

const FALLBACK: ModelEndpoint = {
  protocol: 'ollama',
  url: 'http://127.0.0.1:11434',
  key: '',
  model: 'qwen3.5:9b'
}

/** 继承来源：from 命中的模型池条目，未命中则第一项 */
const inherited = computed<ModelEndpoint>(() => {
  const byName = props.extract.from
    ? props.models.find(m => m.name === props.extract.from)
    : undefined
  const base = byName ?? props.models[0]
  return base ?? FALLBACK
})

/** 实际生效的配置（留空即继承） */
const effective = computed<ModelEndpoint>(() => ({
  protocol: props.extract.protocol || inherited.value.protocol || FALLBACK.protocol,
  url: props.extract.url || inherited.value.url || FALLBACK.url,
  key: props.extract.key || inherited.value.key || '',
  model: props.extract.model || inherited.value.model || FALLBACK.model
}))

const inheritedLabel = computed(() => {
  const base = inherited.value
  return `${protocolLabel(base.protocol)} · ${base.url} · ${base.model || '（未填写模型）'}`
})

const keyPlaceholder = computed(() =>
  inherited.value.key ? '继承：已配置的 Key' : '继承：无需鉴权'
)

function protocolLabel(protocol?: string): string {
  return protocol === 'openai' ? 'OpenAI' : 'Ollama'
}

const statusClass = computed(() => {
  const s = localStatus.value?.status
  if (!s) return ''
  if (s === 'connected') return 'ok'
  if (s === 'not_configured' || s === 'disabled') return 'warning'
  return 'error'
})

const statusText = computed(() => {
  const s = localStatus.value
  if (!s) return '未测试'
  const map: Record<string, string> = {
    connected: '正常',
    not_configured: '未配置',
    disconnected: '无法连接',
    error: '连接错误'
  }
  return map[s.status] || s.status
})

async function test() {
  testing.value = true
  try {
    // 直接按“当前生效”的值测试，保证测的是屏幕上看到的配置
    const res = await testService({
      service_type: 'llm',
      protocol: effective.value.protocol,
      url: effective.value.url,
      key: effective.value.key,
      model: effective.value.model
    })
    localStatus.value = { status: res.status, detail: res.detail }
    showToast(res.detail, res.status === 'connected' ? 'success' : 'error')
  } catch {
    showError('连接测试失败')
  } finally {
    testing.value = false
  }
}

function syncStatus(status: { status: string; detail: string }) {
  localStatus.value = status
}

defineExpose({ syncStatus })
</script>

<style scoped>
.llm-card {
  margin-bottom: 8px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-radius: 8px;
  overflow: hidden;
}

.card-header-collapsible {
  display: flex;
  justify-content: space-between;
  align-items: center;
  padding: 10px 14px;
  cursor: pointer;
  user-select: none;
  transition: background 0.15s;
}
.card-header-collapsible:hover {
  background: var(--bg-hover, rgba(0,0,0,0.03));
}
.card-summary {
  display: flex;
  align-items: center;
  gap: 8px;
  flex: 1;
  min-width: 0;
  overflow: hidden;
}
.card-summary h3 {
  margin: 0;
  font-size: 14px;
  font-weight: 600;
  white-space: nowrap;
}
.collapse-arrow {
  font-size: 10px;
  transition: transform 0.2s;
  flex-shrink: 0;
  color: var(--text-secondary);
}
.collapse-arrow.expanded {
  transform: rotate(90deg);
}
.protocol-badge {
  font-size: 11px;
  padding: 1px 6px;
  border-radius: 4px;
  border: 1px solid var(--border);
  color: var(--text-secondary);
  white-space: nowrap;
  flex-shrink: 0;
}
.llm-model-preview {
  font-size: 12px;
  color: var(--text-primary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
}
.llm-url-preview {
  font-size: 12px;
  color: var(--text-secondary);
  white-space: nowrap;
  overflow: hidden;
  text-overflow: ellipsis;
  flex: 1;
  min-width: 0;
}

.card-body-collapsible {
  padding: 14px;
  border-top: 1px solid var(--border);
}

.btn-ghost {
  background: transparent;
  border: 1px solid transparent;
  color: var(--text-secondary);
  font-size: 12px;
  padding: 4px 10px;
}
.btn-ghost:hover {
  border-color: var(--border);
  background: var(--bg-hover, rgba(0,0,0,0.03));
}

.form-hint {
  margin: 4px 0 0;
  font-size: 11px;
  line-height: 1.5;
  color: var(--text-secondary);
}

.effective-row {
  margin-top: 10px;
  padding: 6px 10px;
  border-radius: 6px;
  font-size: 12px;
  background: var(--bg-hover, rgba(0,0,0,0.03));
  color: var(--text-secondary);
  word-break: break-all;
}

.status-test-row {
  display: flex;
  align-items: center;
  gap: 8px;
  margin-top: 12px;
  padding-top: 12px;
  border-top: 1px dashed var(--border);
}
.status-dot {
  display: inline-block;
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
}
.status-dot.ok { background: var(--success); }
.status-dot.error { background: var(--danger); }
.status-dot.warning { background: var(--warning); }
.status-text {
  font-size: 12px;
  color: var(--text-secondary);
  flex: 1;
}
.status-test-row .btn {
  margin-left: auto;
}
.status-dot-small {
  display: inline-block;
  width: 6px;
  height: 6px;
  border-radius: 50%;
  flex-shrink: 0;
}
.status-dot-small.ok { background: var(--success); }
.status-dot-small.error { background: var(--danger); }
.status-dot-small.warning { background: var(--warning); }

.input-with-toggle {
  display: flex;
  align-items: stretch;
  gap: 0;
}
.input-with-toggle input {
  flex: 1;
  border-top-right-radius: 0;
  border-bottom-right-radius: 0;
}
.toggle-visibility {
  display: flex;
  align-items: center;
  justify-content: center;
  padding: 0 10px;
  background: var(--bg-white);
  border: 1px solid var(--border);
  border-left: none;
  border-radius: 0 6px 6px 0;
  cursor: pointer;
  color: var(--text-secondary);
  min-width: 36px;
  transition: background 0.15s, color 0.15s;
}
.toggle-visibility:hover {
  background: var(--bg-hover, rgba(0,0,0,0.03));
  color: var(--text-primary);
}
</style>
