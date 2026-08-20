<template>
  <div class="markdown-viewer" v-html="rendered"></div>
</template>

<script setup lang="ts">
import { computed } from 'vue'
import { marked } from 'marked'
import DOMPurify from 'dompurify'

const props = defineProps<{ content: string }>()

const rendered = computed(() => {
  // async: false 强制同步返回字符串，随后用 DOMPurify 净化，防止 XSS
  const raw = marked.parse(props.content, {
    async: false,
    gfm: true,
    breaks: true
  }) as string
  return DOMPurify.sanitize(raw)
})
</script>