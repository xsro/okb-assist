import { ref, computed } from 'vue'

/** 分页逻辑复用 composable */
export function usePagination<T>(
  defaultPageSize = 20,
  defaultSortBy = 'created_at'
) {
  const items = ref<T[]>([])
  const loading = ref(false)
  const page = ref(1)
  const pageSize = ref(defaultPageSize)
  const total = ref(0)
  const sortBy = ref(defaultSortBy)
  const sortOrder = ref<'asc' | 'desc'>('desc')
  const error = ref<string | null>(null)

  const totalPages = computed(() => Math.max(1, Math.ceil(total.value / pageSize.value)))
  const hasPrevious = computed(() => page.value > 1)
  const hasNext = computed(() => page.value < totalPages.value)
  const sortOrderLabel = computed(() => (sortOrder.value === 'asc' ? '升序 ↑' : '降序 ↓'))

  function toggleSortOrder() {
    sortOrder.value = sortOrder.value === 'asc' ? 'desc' : 'asc'
  }

  async function load(fetchFn: (params: Record<string, unknown>) => Promise<{ items: T[]; total: number }>, extraParams: Record<string, unknown> = {}) {
    loading.value = true
    error.value = null
    try {
      const res = await fetchFn({
        page: page.value,
        page_size: pageSize.value,
        sort_by: sortBy.value,
        sort_order: sortOrder.value,
        ...extraParams
      })
      items.value = res.items
      total.value = res.total
    } catch (e) {
      error.value = e instanceof Error ? e.message : '加载失败'
      items.value = []
    } finally {
      loading.value = false
    }
  }

  function reset() {
    page.value = 1
    total.value = 0
    items.value = []
    error.value = null
  }

  return {
    // 状态
    items,
    loading,
    error,
    page,
    pageSize,
    total,
    totalPages,
    sortBy,
    sortOrder,
    hasPrevious,
    hasNext,
    sortOrderLabel,
    // 方法
    load,
    reset,
    toggleSortOrder
  }
}