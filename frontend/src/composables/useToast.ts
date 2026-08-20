import { useToastStore, type ToastType } from '@/stores/toast'

export function useToast() {
  const store = useToastStore()

  return {
    showToast: store.add,
    showSuccess: (message: string) => store.add(message, 'success'),
    showError: (message: string) => store.add(message, 'error'),
    showInfo: (message: string) => store.add(message, 'info')
  }
}

export function toast(message: string, type: ToastType = 'info') {
  const store = useToastStore()
  store.add(message, type)
}
