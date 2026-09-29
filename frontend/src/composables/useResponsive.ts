import { ref, computed, onMounted, onUnmounted } from 'vue'

/** 响应式断点工具：统一管理移动端/桌面端检测 */
export function useResponsive() {
  const viewportWidth = ref(window.innerWidth)

  function onResize() {
    viewportWidth.value = window.innerWidth
  }

  onMounted(() => {
    window.addEventListener('resize', onResize)
  })

  onUnmounted(() => {
    window.removeEventListener('resize', onResize)
  })

  const isMobile = computed(() => viewportWidth.value <= 768)
  const isSmallMobile = computed(() => viewportWidth.value <= 480)
  const isTablet = computed(() => viewportWidth.value > 768 && viewportWidth.value <= 1024)
  const isDesktop = computed(() => viewportWidth.value > 1024)

  return { viewportWidth, isMobile, isSmallMobile, isTablet, isDesktop }
}