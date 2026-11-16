import { create } from 'zustand'

interface ConfirmDialogOptions {
  title: string
  message: string
  confirmLabel?: string
  destructive?: boolean
  /** The no-op button's words (default "Cancel"), e.g. "Keep it". */
  cancelLabel?: string
  /** The no-op is the default: it has focus, so Enter keeps things as they
   *  are (for an action that can't be undone). */
  safeDefault?: boolean
}

/** One answer of a `choose` dialog (besides Cancel). */
export interface ConfirmChoice {
  id: string
  label: string
  /** The answer Enter gives, drawn as the dialog's main button. */
  primary?: boolean
  /** One plain line under the label: what this answer does. Given on any
   *  choice, the answers stack as rows above Cancel. */
  hint?: string
  /** The safe answer: it has the focus when the dialog opens, so Enter
   *  picks it (Enter presses whichever answer has the focus). */
  safeDefault?: boolean
}

interface ChooseDialogOptions {
  title: string
  /** The body line; empty draws none. */
  message: string
  /** Two (or more) real answers, in the order they show, left to right,
   *  after Cancel (rows above Cancel when they have hints). */
  choices: ConfirmChoice[]
  /** The whole title on hover, for a title cut short (a long name). */
  titleTooltip?: string
}

interface ConfirmDialogState {
  isOpen: boolean
  title: string
  message: string
  confirmLabel: string
  confirmDestructive: boolean
  cancelLabel: string
  safeDefault: boolean
  /** A `choose` title cut short: the whole of it on hover ('': none). */
  titleTooltip: string
  onResolve: ((confirmed: boolean) => void) | null
  /** A `choose` dialog's answers (null: a plain confirm). */
  choices: ConfirmChoice[] | null
  onChoose: ((id: string | null) => void) | null

  confirm: (opts: ConfirmDialogOptions) => Promise<boolean>
  /** A dialog with several real answers and a Cancel that is none of them:
   *  resolves the picked choice's id, or null for Cancel (Esc, ×, a press
   *  outside). For a question where Cancel must not mean either answer. */
  choose: (opts: ChooseDialogOptions) => Promise<string | null>
  close: () => void
}

/** The closed dialog. */
export const CONFIRM_DIALOG_CLOSED = {
  isOpen: false,
  title: '',
  message: '',
  confirmLabel: 'Confirm',
  confirmDestructive: false,
  cancelLabel: 'Cancel',
  safeDefault: false,
  titleTooltip: '',
  onResolve: null,
  choices: null,
  onChoose: null,
} as const

export const useConfirmDialogStore = create<ConfirmDialogState>((set, get) => {
  /** Any dialog already open is answered as cancelled. */
  const cancelOpen = (): void => {
    const current = get()
    if (!current.isOpen) return
    current.onResolve?.(false)
    current.onChoose?.(null)
  }
  return {
    ...CONFIRM_DIALOG_CLOSED,

    confirm: (opts) => {
      return new Promise<boolean>((resolve) => {
        cancelOpen()
        set({
          isOpen: true,
          title: opts.title,
          message: opts.message,
          confirmLabel: opts.confirmLabel ?? 'Confirm',
          confirmDestructive: opts.destructive ?? false,
          cancelLabel: opts.cancelLabel ?? 'Cancel',
          safeDefault: opts.safeDefault ?? false,
          titleTooltip: '',
          onResolve: resolve,
          choices: null,
          onChoose: null,
        })
      })
    },

    choose: (opts) => {
      return new Promise<string | null>((resolve) => {
        cancelOpen()
        set({
          ...CONFIRM_DIALOG_CLOSED,
          isOpen: true,
          title: opts.title,
          message: opts.message,
          choices: opts.choices,
          titleTooltip: opts.titleTooltip ?? '',
          onChoose: resolve,
        })
      })
    },

    close: () => {
      const { onResolve, onChoose } = get()
      onResolve?.(false)
      onChoose?.(null)
      set({ ...CONFIRM_DIALOG_CLOSED })
    },
  }
})
