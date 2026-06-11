import { create } from 'zustand'

interface ConfirmDialogOptions {
  title: string
  message: string
  confirmLabel?: string
  destructive?: boolean
}

interface ConfirmDialogState {
  isOpen: boolean
  title: string
  message: string
  confirmLabel: string
  confirmDestructive: boolean
  onResolve: ((confirmed: boolean) => void) | null

  confirm: (opts: ConfirmDialogOptions) => Promise<boolean>
  close: () => void
}

export const useConfirmDialogStore = create<ConfirmDialogState>((set, get) => ({
  isOpen: false,
  title: '',
  message: '',
  confirmLabel: 'Confirm',
  confirmDestructive: false,
  onResolve: null,

  confirm: (opts) => {
    return new Promise<boolean>((resolve) => {
      // Resolve any existing dialog as cancelled
      const current = get()
      if (current.isOpen && current.onResolve) {
        current.onResolve(false)
      }

      set({
        isOpen: true,
        title: opts.title,
        message: opts.message,
        confirmLabel: opts.confirmLabel ?? 'Confirm',
        confirmDestructive: opts.destructive ?? false,
        onResolve: resolve
      })
    })
  },

  close: () => {
    const { onResolve } = get()
    if (onResolve) {
      onResolve(false)
    }
    set({
      isOpen: false,
      title: '',
      message: '',
      confirmLabel: 'Confirm',
      confirmDestructive: false,
      onResolve: null
    })
  }
}))
