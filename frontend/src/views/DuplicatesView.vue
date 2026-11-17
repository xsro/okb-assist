<template>
  <div class="duplicates-view">
    <h2>去重</h2>

    <!-- 检测模式：标题相似 / DOI 重复（两种依据来自同一次比较） -->
    <div class="mode-tabs">
      <button
        class="btn btn-sm"
        :class="mode === 'title' ? 'btn-primary' : 'btn-outline'"
        @click="mode = 'title'"
      >
        标题相似（{{ hasScan ? titleGroups.length + ' 组' : '未比较' }}）
      </button>
      <button
        class="btn btn-sm"
        :class="mode === 'doi' ? 'btn-primary' : 'btn-outline'"
        @click="mode = 'doi'"
      >
        DOI 重复（{{ hasScan ? doiGroups.length + ' 组' : '未比较' }}）
      </button>
      <!-- 一次比较同时产出两个标签页的结果，并覆盖写检测记录 -->
      <button class="btn btn-sm" :disabled="scanning || merging" @click="runScan">
        {{ scanning ? '比较中…' : hasScan ? '重新比较' : '开始比较' }}
      </button>
    </div>

    <p class="hint">
      <template v-if="mode === 'title'">
        按「归一化标题 + 编辑距离 / 内容词重合度」聚类，英文题名（<code>i18n.en.title</code>）
        作为别名参与比较。默认全部打勾（全部保留），取消勾选表示该条将被合并删除。
      </template>
      <template v-else>
        按「归一化 DOI」分组（忽略 <code>https://doi.org/</code> 前缀、大小写与尾部标点），
        组内同时给出标题一致性。默认全部打勾（全部保留），取消勾选表示该条将被合并删除。
      </template>
    </p>

    <!-- 记录状态条：检测时间、库是否已变化、写盘是否成功 -->
    <div v-if="hasScan" class="scan-status">
      <span class="scan-time">
        检测时间：{{ formatTime(generatedAt) }}
        <template v-if="scannedDocs"> · 参与比较 {{ scannedDocs }} 篇</template>
      </span>
      <span v-if="stale" class="scan-flag is-stale" title="检测之后库里又有增删改">
        库已变化，建议重新比较
      </span>
      <span v-if="notSaved" class="scan-flag is-warn" :title="detail">{{ notSavedText }}</span>
    </div>

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
            <span
              v-for="badge in plan.group.badges"
              :key="badge.text"
              class="group-badge"
              :class="`is-${badge.tone}`"
              :title="badge.title"
              >{{ badge.text }}</span
            >
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
      <template v-if="loading">{{ scanning ? '整库比较中…' : '载入上次检测结果…' }}</template>
      <template v-else-if="!hasScan">
        <p>尚未比较。</p>
        <p class="empty-tip">
          点击上方「开始比较」对整库执行一次重复检测（标题相似 + DOI 重复），
          结果会存到服务端，下次打开本页自动载入。
          <template v-if="detail">（{{ detail }}）</template>
        </p>
        <button class="btn btn-sm" :disabled="scanning || merging" @click="runScan">
          开始比较
        </button>
      </template>
      <template v-else>
        {{ mode === 'title' ? '暂无相似文献' : '暂无 DOI 重复的文献' }}
      </template>
    </div>
  </div>
</template>

<script setup lang="ts">
import { onMounted, ref, computed } from 'vue'
import { getDuplicateScan, runDuplicateScan, mergeDocuments } from '@/api/documents'
import { useToast } from '@/composables/useToast'
import { useRequireToken } from '@/composables/useRequireToken'
import type {
  Document,
  DuplicateScanRecord,
  SimilarDoiItem,
  SimilarTitleItem,
} from '@/types/document'

const { showSuccess, showError, showInfo } = useToast()
const { requireToken } = useRequireToken()

const mode = ref<'title' | 'doi'>('title')
const titleGroups = ref<SimilarTitleItem[]>([])
const doiGroups = ref<SimilarDoiItem[]>([])
/** 正在执行整库比较（POST）/ 正在载入上次记录（GET） */
const scanning = ref(false)
const recordLoading = ref(false)
/** 载入或比较期间禁用写操作 */
const loading = computed(() => scanning.value || recordLoading.value)
/** 是否已经有一次比较结果（0 组也算比较过） */
const hasScan = ref(false)
/** 记录元信息：检测时间、参与比较的文档数、是否已过期、是否写盘成功 */
const generatedAt = ref('')
const scannedDocs = ref(0)
const stale = ref(false)
const notSaved = ref(false)
/** 后端给出的原因（记录读不出来 / 写不进去） */
const detail = ref('')
const notSavedText = computed(
  () => detail.value || '检测记录未写入服务端，下次打开需要重新比较'
)
const merging = ref(false)
const progressDone = ref(0)
const progressTotal = ref(0)

/** 组头徽标：算法给出的分数与「另一条依据」的佐证 */
interface GroupBadge {
  text: string
  title: string
  tone: 'ok' | 'warn' | 'info'
}

/** 两种模式统一成同一渲染结构：key 用于稳定绑定「保留」选择 */
interface GroupView {
  key: string
  label: string
  documents: Document[]
  badges: GroupBadge[]
}

function percent(score?: number | null): string {
  return score === null || score === undefined ? '' : `${Math.round(score * 100)}%`
}

function titleBadges(g: SimilarTitleItem): GroupBadge[] {
  const out: GroupBadge[] = []
  if (g.score !== undefined && g.score !== null) {
    out.push({
      text: `相似 ${percent(g.score)}`,
      title: '组内最弱一条边的相似度（编辑距离 + 内容词重合度），偏低说明只是部分重合',
      tone: (g.score ?? 1) >= 0.9 ? 'ok' : 'info',
    })
  }
  if (g.same_doi) {
    out.push({ text: '同 DOI', title: '组内 DOI 一致，基本可以确认是同一条文献', tone: 'ok' })
  }
  if (g.doi_conflict) {
    out.push({
      text: 'DOI 冲突',
      title: '组内出现互不相同的 DOI：标题像但标识不一致，请先核对再合并',
      tone: 'warn',
    })
  }
  if (g.cross_lang) {
    out.push({
      text: '中英标题',
      title: '这组里有成员是靠 i18n 英文题名别名与对方主标题对上的',
      tone: 'info',
    })
  }
  return out
}

function doiBadges(g: SimilarDoiItem): GroupBadge[] {
  const out: GroupBadge[] = []
  if (g.title_score !== null && g.title_score !== undefined) {
    out.push({
      text: `标题相似 ${percent(g.title_score)}`,
      title: '组内标题之间的最高相似度',
      tone: (g.title_score ?? 0) >= 0.9 ? 'ok' : 'info',
    })
  }
  if (g.title_conflict) {
    out.push({
      text: '标题不符',
      title: 'DOI 相同但标题明显不像：多半是某条的 DOI 填错，请核对后再合并',
      tone: 'warn',
    })
  }
  return out
}

const currentGroups = computed<GroupView[]>(() =>
  mode.value === 'title'
    ? titleGroups.value.map((g) => ({
        key: `title:${g.normalized_title}`,
        label: `相似标题：${g.normalized_title}`,
        documents: g.documents,
        badges: titleBadges(g),
      }))
    : doiGroups.value.map((g) => ({
        key: `doi:${g.doi_key}`,
        label: `DOI：${g.doi}`,
        documents: g.documents,
        badges: doiBadges(g),
      }))
)

/**
 * 每个组的「保留」选择：key → 保留的文档 ID 数组。
 * 缺省（key 不存在）视为全部保留，因此默认全部打勾、不删任何东西。
 */
const keepMap = ref<Record<string, number[]>>({})

/** 把两组分组的选择都重置为「全部保留」（一次比较同时给出两种依据） */
function resetSelection() {
  const next: Record<string, number[]> = { ...keepMap.value }
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
  // 只覆盖当前标签页的组，另一模式的选择保留
  keepMap.value = { ...keepMap.value, ...next }
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

function formatTime(iso: string): string {
  if (!iso) return '未知'
  const d = new Date(iso)
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString()
}

/** 把一次比较的结果（或上次记录）套用到两个标签页 */
function applyRecord(record: DuplicateScanRecord) {
  titleGroups.value = record.title.groups
  doiGroups.value = record.doi.groups
  hasScan.value = true
  generatedAt.value = record.generated_at
  scannedDocs.value = record.scanned_docs ?? 0
  // 分组数据一旦换过，旧的「保留」勾选就不适用了，统一恢复全部保留
  resetSelection()
  // 当前标签页空了、另一页有结果时切过去；用户停在有结果的那页则不动
  const currentEmpty = (mode.value === 'title' ? record.title.groups : record.doi.groups).length === 0
  const otherHasGroups = (mode.value === 'title' ? record.doi.groups : record.title.groups).length > 0
  if (currentEmpty && otherHasGroups) mode.value = mode.value === 'title' ? 'doi' : 'title'
}

/** 读取服务端记录：进入页面自动调用，失败只提示不阻断 */
async function loadRecord() {
  if (!requireToken()) return
  if (loading.value) return
  recordLoading.value = true
  try {
    const res = await getDuplicateScan()
    detail.value = res.detail || ''
    if (res.found && res.result) {
      applyRecord(res.result)
      stale.value = !!res.stale
    }
  } catch {
    showError('载入上次检测结果失败')
  } finally {
    recordLoading.value = false
  }
}

/**
 * 重新比较整库：一次请求同时刷新「标题相似」与「DOI 重复」，并覆盖写检测记录。
 * 合并完成后也走这里，保证库内容与落盘记录同步。返回是否真的比较成功，
 * 调用方（合并流程）据此决定能不能清掉「库已变化」提示。
 */
async function runScan(): Promise<boolean> {
  if (!requireToken()) return false
  // 这里只挡「已在比较中」，不挡 merging：合并流程本身就在 merging 期间调用它刷新结果，
  // 手动触发比较的按钮和入口在 merging 时都是禁用的，不会并发。
  if (scanning.value) return false
  scanning.value = true
  try {
    const res = await runDuplicateScan()
    if (!res.found || !res.result) {
      detail.value = res.detail || ''
      showError(res.detail || '重复比较失败')
      return false
    }
    applyRecord(res.result)
    stale.value = false
    notSaved.value = res.saved === false
    detail.value = res.detail || ''
    showSuccess(
      `比较完成：标题相似 ${res.result.title.total_groups} 组 · DOI 重复 ${res.result.doi.total_groups} 组`
    )
    return true
  } catch {
    showError('重复比较失败，请查看日志')
    return false
  } finally {
    scanning.value = false
  }
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
    if (ok) await runScan()
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
    if (okGroups) await runScan()
  } finally {
    merging.value = false
    progressDone.value = 0
    progressTotal.value = 0
  }
}

// 进入页面自动载入上次检测记录（记录在服务端 data/tmp 下），不重新比较
onMounted(loadRecord)
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

/* ── 记录状态条 ─────────────────────────────────────── */
.scan-status {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  gap: 10px;
  margin-bottom: 12px;
  font-size: 13px;
  color: var(--text-secondary);
}

.scan-time {
  font-variant-numeric: tabular-nums;
}

.scan-flag {
  padding: 2px 8px;
  border-radius: 10px;
  font-size: 12px;
}

.scan-flag.is-stale {
  background: rgba(245, 158, 11, 0.14);
  color: #92400e;
}

.scan-flag.is-warn {
  background: rgba(239, 68, 68, 0.12);
  color: #b91c1c;
}

/* 组头徽标：相似度分数与另一条依据的佐证 */
.group-badge {
  font-size: 12px;
  padding: 2px 8px;
  border-radius: 10px;
  white-space: nowrap;
}

.group-badge.is-ok {
  background: rgba(34, 197, 94, 0.12);
  color: #15803d;
}

.group-badge.is-warn {
  background: rgba(239, 68, 68, 0.12);
  color: #b91c1c;
}

.group-badge.is-info {
  background: rgba(59, 130, 246, 0.12);
  color: #1d4ed8;
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

.empty-tip {
  margin: 8px 0 16px;
  font-size: 13px;
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
