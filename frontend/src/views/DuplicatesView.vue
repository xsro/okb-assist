<template>
  <div class="duplicates-view">
    <h2>去重</h2>

    <!-- 检测模式：标题相似 / DOI 重复 -->
    <div class="mode-tabs">
      <button
        class="btn btn-sm"
        :class="mode === 'title' ? 'btn-primary' : 'btn-outline'"
        @click="mode = 'title'"
      >
        标题相似（{{ titleGroups.length }} 组）
      </button>
      <button
        class="btn btn-sm"
        :class="mode === 'doi' ? 'btn-primary' : 'btn-outline'"
        @click="mode = 'doi'"
      >
        DOI 重复（{{ doiGroups.length }} 组）
      </button>
      <button class="btn btn-sm btn-outline" :disabled="loading || merging" @click="load">
        {{ loading ? '检测中…' : '重新检测' }}
      </button>
    </div>

    <p class="hint">
      <template v-if="mode === 'title'">
        按「归一化标题 + 编辑距离相似度」分组。默认全部打勾（全部保留），取消勾选表示该条将被合并删除。
      </template>
      <template v-else>
        按「归一化 DOI」分组（忽略 <code>https://doi.org/</code> 前缀、大小写与尾部标点）。默认全部打勾（全部保留），取消勾选表示该条将被合并删除。
      </template>
    </p>

    <!-- 批量操作条 -->
    <div v-if="plans.length" class="groups-toolbar">
      <span class="toolbar-summary">
        共 {{ plans.length }} 组 · 可去重 {{ actionablePlans.length }} 组 · 将删除
        {{ pendingDeleteCount }} 篇
      </span>
      <div class="toolbar-actions">
        <button class="btn btn-sm btn-outline" :disabled="merging" @click="keepAllEverywhere">
          全部保留
        </button>
        <button class="btn btn-sm btn-outline" :disabled="merging" @click="keepFirstEverywhere">
          每组只留第一篇
        </button>
        <button
          class="btn btn-sm btn-danger"
          :disabled="merging || loading || !actionablePlans.length"
          @click="runBatchMerge"
        >
          {{
            merging
              ? `去重中… ${progressDone}/${progressTotal}`
              : `一键去重（${actionablePlans.length} 组）`
          }}
        </button>
      </div>
    </div>

    <div v-if="plans.length" class="groups-list">
      <div v-for="(plan, idx) in plans" :key="plan.group.key" class="group-card">
        <div class="group-head">
          <h4>组 {{ idx + 1 }} · {{ plan.group.documents.length }} 篇 · {{ plan.group.label }}</h4>
          <div class="group-head-actions">
            <span class="keep-count" :class="{ 'is-empty': !plan.kept.length }">
              保留 {{ plan.kept.length }} / {{ plan.group.documents.length }}
            </span>
            <button class="btn btn-sm btn-outline" :disabled="merging" @click="keepAll(plan.group)">
              全保留
            </button>
            <button
              class="btn btn-sm btn-outline-danger"
              :disabled="merging || !plan.actionable"
              @click="applyGroup(plan)"
            >
              {{
                plan.actionable
                  ? `应用本组（删 ${plan.removed.length} 篇）`
                  : plan.kept.length
                    ? '全部保留，无需处理'
                    : '请至少保留 1 篇'
              }}
            </button>
          </div>
        </div>

        <table class="doc-table">
          <thead>
            <tr>
              <th class="col-check">保留</th>
              <th>ID</th>
              <th>标题</th>
              <th>作者</th>
              <th>年份</th>
              <th>DOI</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            <tr
              v-for="doc in plan.group.documents"
              :key="doc.id"
              :class="{ 'row-removed': !isKept(plan.group.key, doc.id) }"
            >
              <td class="col-check">
                <input
                  type="checkbox"
                  :checked="isKept(plan.group.key, doc.id)"
                  :disabled="merging"
                  :title="isKept(plan.group.key, doc.id) ? '保留此条' : '此条将被合并删除'"
                  @change="toggleKeep(plan.group, doc.id)"
                />
              </td>
              <td>{{ doc.id }}</td>
              <td>{{ doc.title }}</td>
              <td>{{ doc.authors || '-' }}</td>
              <td>{{ doc.year || '-' }}</td>
              <td class="doi-cell">{{ doc.doi || '-' }}</td>
              <td class="col-action">
                <button
                  class="btn btn-sm"
                  :class="isOnlyKept(plan, doc.id) ? 'btn-primary' : 'btn-outline'"
                  :disabled="merging || isOnlyKept(plan, doc.id)"
                  @click="keepOnly(plan.group, doc.id)"
                >
                  {{ isOnlyKept(plan, doc.id) ? '保留此项' : '仅保此项' }}
                </button>
              </td>
            </tr>
          </tbody>
        </table>
      </div>
    </div>

    <div v-else class="empty">
      {{ loading ? '检测中…' : mode === 'title' ? '暂无相似文献' : '暂无 DOI 重复的文献' }}
    </div>
  </div>
</template>

<script setup lang="ts">
import { ref, computed, onMounted } from 'vue'
import { getSimilarTitles, getSimilarDois, mergeDocuments } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type { Document, SimilarTitleGroup, SimilarDoiGroup } from '@/types/document'

const { showSuccess, showError, showInfo } = useToast()
const { requireToken } = useRequireToken()

const mode = ref<'title' | 'doi'>('title')
const titleGroups = ref<SimilarTitleGroup['groups']>([])
const doiGroups = ref<SimilarDoiGroup['groups']>([])
const loading = ref(false)
const merging = ref(false)
const progressDone = ref(0)
const progressTotal = ref(0)

/** 两种模式统一成同一渲染结构：key 用于稳定绑定「保留」选择 */
interface GroupView {
  key: string
  label: string
  documents: Document[]
}

const currentGroups = computed<GroupView[]>(() =>
  mode.value === 'title'
    ? titleGroups.value.map((g) => ({
        key: `title:${g.normalized_title}`,
        label: `相似标题：${g.normalized_title}`,
        documents: g.documents,
      }))
    : doiGroups.value.map((g) => ({
        key: `doi:${g.doi_key}`,
        label: `DOI：${g.doi}`,
        documents: g.documents,
      }))
)

/**
 * 每个组的「保留」选择：key → 保留的文档 ID 数组。
 * 缺省（key 不存在）视为全部保留，因此默认全部打勾、不删任何东西。
 */
const keepMap = ref<Record<string, number[]>>({})

/** 把两份检测结果里的所有组重置为「全部保留」 */
function resetSelection() {
  const next: Record<string, number[]> = {}
  for (const g of titleGroups.value) {
    next[`title:${g.normalized_title}`] = g.documents.map((d) => d.id)
  }
  for (const g of doiGroups.value) {
    next[`doi:${g.doi_key}`] = g.documents.map((d) => d.id)
  }
  keepMap.value = next
}

function isKept(groupKey: string, id: number): boolean {
  const kept = keepMap.value[groupKey]
  return kept ? kept.includes(id) : true
}

function toggleKeep(group: GroupView, id: number) {
  const kept = keepMap.value[group.key] ?? group.documents.map((d) => d.id)
  keepMap.value = {
    ...keepMap.value,
    [group.key]: kept.includes(id) ? kept.filter((v) => v !== id) : [...kept, id],
  }
}

function keepAll(group: GroupView) {
  keepMap.value = { ...keepMap.value, [group.key]: group.documents.map((d) => d.id) }
}

function keepOnly(group: GroupView, id: number) {
  keepMap.value = { ...keepMap.value, [group.key]: [id] }
}

function keepAllEverywhere() {
  resetSelection()
  showInfo('已恢复为全部保留（不会删除任何文献）')
}

/** 每组只保留第一篇 —— 批量去重的一键预设 */
function keepFirstEverywhere() {
  const next: Record<string, number[]> = {}
  for (const g of plans.value) {
    const first = g.group.documents[0]
    if (first) next[g.group.key] = [first.id]
  }
  keepMap.value = next
  showInfo('已把每组的第一篇设为保留项，确认无误后点「一键去重」')
}

interface GroupPlan {
  group: GroupView
  kept: Document[]
  removed: Document[]
  /** 只留 1 篇（有可删除项）时才能一键处理 */
  actionable: boolean
}

const plans = computed<GroupPlan[]>(() =>
  currentGroups.value.map((group) => {
    const kept = group.documents.filter((d) => isKept(group.key, d.id))
    const removed = group.documents.filter((d) => !isKept(group.key, d.id))
    return { group, kept, removed, actionable: kept.length === 1 && removed.length > 0 }
  })
)

const actionablePlans = computed(() => plans.value.filter((p) => p.actionable))

const pendingDeleteCount = computed(() =>
  actionablePlans.value.reduce((sum, p) => sum + p.removed.length, 0)
)

function isOnlyKept(plan: GroupPlan, id: number): boolean {
  return plan.kept.length === 1 && plan.kept[0]?.id === id
}

async function load() {
  if (!requireToken()) return
  loading.value = true
  // 两个检测都是 admin 级只读查询，一次都拉回来，切换 tab 不再等
  const [titles, dois] = await Promise.allSettled([getSimilarTitles(), getSimilarDois()])
  if (titles.status === 'fulfilled') {
    titleGroups.value = titles.value.groups
  } else {
    showError('标题相似检测失败')
  }
  if (dois.status === 'fulfilled') {
    doiGroups.value = dois.value.groups
  } else {
    showError('DOI 重复检测失败')
  }
  // 重新检测后一律恢复「全部保留」，避免沿用已失效的选择
  resetSelection()
  loading.value = false
}

/** 执行一次合并（保留 1 篇，其余删除） */
async function mergeInto(group: GroupView, target: Document, sources: Document[]): Promise<boolean> {
  const ids = sources.map((d) => d.id)
  if (!ids.length) return false
  try {
    const res = await mergeDocuments(target.id, ids)
    if (res.detail) showSuccess(res.detail)
    return true
  } catch {
    showError(`组「${group.label}」合并失败（目标 ${target.id}）`)
    return false
  }
}

/** 单组应用：把未勾选的条目合并进唯一勾选的那篇 */
async function applyGroup(plan: GroupPlan) {
  if (merging.value || !plan.actionable) return
  const target = plan.kept[0]
  if (
    !confirm(
      `确定保留「${target.title || target.id}」并删除本组其余 ${plan.removed.length} 篇文献吗？\n被删除的文献文件与记录将永久移除，此操作不可逆！`
    )
  ) {
    return
  }
  merging.value = true
  progressTotal.value = 1
  progressDone.value = 0
  try {
    const ok = await mergeInto(plan.group, target, plan.removed)
    progressDone.value = 1
    if (ok) await load()
  } finally {
    merging.value = false
  }
}

/** 一键去重：对所有「只保留 1 篇」的组依次执行合并；保留 ≥2 篇的组原样不动 */
async function runBatchMerge() {
  if (merging.value) return
  const todo = actionablePlans.value
  if (!todo.length) {
    showInfo('当前没有可去重的组（每组都保留了 2 篇以上）')
    return
  }
  const total = todo.reduce((sum, p) => sum + p.removed.length, 0)
  const skipped = plans.value.length - todo.length
  if (
    !confirm(
      `将处理 ${todo.length} 组、删除 ${total} 篇文献（合并到每组保留的那 1 篇）。` +
        (skipped > 0 ? `\n另有 ${skipped} 组保留 2 篇以上，保持原样不动。` : '') +
        `\n被删除的文献文件与记录将永久移除，此操作不可逆！`
    )
  ) {
    return
  }

  merging.value = true
  progressTotal.value = todo.length
  progressDone.value = 0
  // 组之间可能交叉引用同一篇文档，已删除的 ID 不再参与后续合并
  const deletedIds = new Set<number>()
  let okGroups = 0
  let failGroups = 0
  let deletedCount = 0
  try {
    for (const plan of todo) {
      const target = plan.kept[0]
      if (!target || deletedIds.has(target.id)) {
        // 目标已被前面的组删掉，跳过
        progressDone.value += 1
        continue
      }
      const sources = plan.removed.filter((d) => !deletedIds.has(d.id))
      if (!sources.length) {
        progressDone.value += 1
        continue
      }
      const ok = await mergeInto(plan.group, target, sources)
      if (ok) {
        okGroups += 1
        deletedCount += sources.length
        sources.forEach((d) => deletedIds.add(d.id))
      } else {
        failGroups += 1
      }
      progressDone.value += 1
    }
    if (okGroups) showSuccess(`已去重 ${okGroups} 组，删除 ${deletedCount} 篇文献`)
    if (failGroups) showError(`${failGroups} 组去重失败，请查看日志后重试`)
    if (okGroups) await load()
  } finally {
    merging.value = false
    progressDone.value = 0
    progressTotal.value = 0
  }
}

onMounted(load)
</script>

<style scoped>
.mode-tabs {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
  margin-bottom: 12px;
}

.hint {
  color: var(--text-secondary);
  margin-bottom: 16px;
}

/* ── 批量操作条 ─────────────────────────────────────── */
.groups-toolbar {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 10px;
  padding: 10px 14px;
  margin-bottom: 16px;
  background: var(--bg-footer, #f7f9f8);
  border: 1px solid var(--border);
  border-radius: 8px;
  position: sticky;
  top: 0;
  z-index: 5;
}

.toolbar-summary {
  font-size: 13px;
  color: var(--text-secondary);
}

.toolbar-actions {
  display: flex;
  flex-wrap: wrap;
  gap: 8px;
}

.group-card {
  margin-bottom: 24px;
  padding: 16px;
  background: #fff;
  border: 1px solid var(--border);
  border-radius: 8px;
}

.group-head {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: space-between;
  gap: 8px;
  margin-bottom: 12px;
}

.group-head h4 {
  margin: 0;
  word-break: break-all;
}

.group-head-actions {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 8px;
}

.keep-count {
  font-size: 12px;
  padding: 2px 8px;
  border-radius: 10px;
  background: rgba(34, 197, 94, 0.12);
  color: #15803d;
}

.keep-count.is-empty {
  background: rgba(239, 68, 68, 0.12);
  color: #b91c1c;
}

.col-check {
  width: 52px;
  text-align: center;
}

.col-check input {
  width: 16px;
  height: 16px;
  cursor: pointer;
}

.col-action {
  white-space: nowrap;
}

/* 未勾选（将被合并删除）的行做弱化标记 */
.row-removed td {
  background: rgba(239, 68, 68, 0.06);
  color: var(--text-secondary);
}

.row-removed td:nth-child(3) {
  text-decoration: line-through;
}

.doi-cell {
  font-family: ui-monospace, SFMono-Regular, Menlo, monospace;
  font-size: 12px;
  word-break: break-all;
}

.empty {
  text-align: center;
  padding: 60px;
  color: var(--text-secondary);
}

@media (max-width: 768px) {
  .group-card {
    padding: 12px;
    margin-bottom: 16px;
  }

  .group-head h4 {
    font-size: 15px;
  }

  .groups-toolbar {
    padding: 8px 10px;
  }

  .toolbar-actions .btn {
    flex: 1;
  }

  .doc-table th,
  .doc-table td {
    padding: 8px 10px;
    font-size: 12px;
  }

  .doc-table .btn {
    padding: 8px 10px;
    font-size: 12px;
  }

  .empty {
    padding: 40px 20px;
  }
}

@media (max-width: 480px) {
  .group-card {
    padding: 10px;
  }

  .doc-table th,
  .doc-table td {
    padding: 6px 8px;
    font-size: 11px;
  }
}
</style>
