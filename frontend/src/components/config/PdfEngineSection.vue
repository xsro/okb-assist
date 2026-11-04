<template>
  <div class="section">
    <h3>PDF 解析引擎</h3>

    <div class="engine-options">
      <label
        v-for="opt in options"
        :key="opt.value"
        class="engine-option"
        :class="{
          selected: modelValue.pdf?.engine === opt.value,
          disabled: !opt.available
        }"
        :title="opt.available ? opt.desc : opt.unavailableHint"
      >
        <input
          type="radio"
          name="pdf-engine"
          :value="opt.value"
          :checked="modelValue.pdf?.engine === opt.value"
          :disabled="!opt.available"
          @change="select(opt.value)"
        />
        <span class="engine-body">
          <span class="engine-name">
            {{ opt.label }}
            <span v-if="!opt.available" class="engine-tag">不可用</span>
            <span v-else-if="opt.value === capabilities?.active && modelValue.pdf?.engine !== opt.value" class="engine-tag tag-fallback">
              生效中（已回退）
            </span>
            <span v-else-if="opt.value === capabilities?.active" class="engine-tag tag-active">生效中</span>
          </span>
          <span class="engine-desc">{{ opt.available ? opt.desc : opt.unavailableHint }}</span>
        </span>
      </label>
    </div>

    <p class="hint">
      影响元数据抽取、PDF 信息报告与 <strong>PDF 健康检查</strong>。改动保存后即时生效（无需重启）。
    </p>
    <p v-if="modelValue.pdf?.engine === 'mupdf'" class="hint hint-warn">
      内置 mupdf 没有回传「已自动修复」诊断的接口：xref 损坏但能修复的 PDF 在健康检查里会显示为正常。
      需要检出这类损坏请选 mutool。
    </p>
  </div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import type { PdfCapabilities, PdfEngineName, ServiceConfig } from '@/types/config'

const props = defineProps<{
  modelValue: ServiceConfig
}>()

const emit = defineEmits<{
  (e: 'update:modelValue', val: ServiceConfig): void
}>()

const capabilities = computed<PdfCapabilities | undefined>(() => props.modelValue.pdf_capabilities)

interface EngineOption {
  value: PdfEngineName
  label: string
  desc: string
  available: boolean
  unavailableHint: string
}

const options = computed<EngineOption[]>(() => {
  const caps = capabilities.value
  // 服务端未下发能力信息时按「mutool 可用、mupdf 视配置而定」处理（老后端兼容）
  const mupdfAvailable = caps ? caps.mupdf : false
  return [
    {
      value: 'mutool',
      label: 'mutool 命令行',
      desc: '调用系统 mutool（system.json 的 mutool_path）。能检出 xref 损坏/截断等结构问题，所有构建都可用。',
      available: true,
      unavailableHint: ''
    },
    {
      value: 'mupdf',
      label: '内置 mupdf 库',
      desc: '使用编译进二进制的 mupdf 库，无需外部 mutool。',
      available: mupdfAvailable,
      unavailableHint: '当前二进制未启用 mupdf feature（需用 --features mupdf 编译），只能使用 mutool 命令行。'
    }
  ]
})

function select(engine: PdfEngineName) {
  if (!props.modelValue.pdf) {
    props.modelValue.pdf = { engine }
  } else {
    props.modelValue.pdf.engine = engine
  }
  emit('update:modelValue', props.modelValue)
}
</script>

<style scoped>
.engine-options {
  display: flex;
  flex-direction: column;
  gap: 8px;
  margin-top: 8px;
}

.engine-option {
  display: flex;
  align-items: flex-start;
  gap: 10px;
  padding: 10px 12px;
  border: 1px solid var(--border-color);
  border-radius: 8px;
  cursor: pointer;
  transition: border-color 0.15s, background 0.15s;
}

.engine-option:hover:not(.disabled) {
  border-color: var(--primary-color);
}

.engine-option.selected {
  border-color: var(--primary-color);
  background: rgba(45, 156, 106, 0.08);
}

.engine-option.disabled {
  cursor: not-allowed;
  opacity: 0.55;
}

.engine-option input[type='radio'] {
  margin-top: 3px;
  flex: 0 0 auto;
}

.engine-body {
  display: flex;
  flex-direction: column;
  gap: 4px;
}

.engine-name {
  font-weight: 600;
  display: flex;
  align-items: center;
  gap: 6px;
  flex-wrap: wrap;
}

.engine-tag {
  font-size: 11px;
  font-weight: 500;
  padding: 1px 6px;
  border-radius: 999px;
  background: var(--bg-tertiary, #eee);
  color: var(--text-secondary);
}

.engine-tag.tag-active {
  background: rgba(45, 156, 106, 0.16);
  color: var(--primary-color);
}

.engine-tag.tag-fallback {
  background: rgba(230, 162, 60, 0.18);
  color: #b7791f;
}

.engine-desc {
  font-size: 12px;
  color: var(--text-secondary);
  line-height: 1.5;
}

.hint {
  font-size: 12px;
  color: var(--text-secondary);
  margin: 8px 0 0;
  line-height: 1.5;
}

.hint-warn {
  color: #b7791f;
}
</style>
