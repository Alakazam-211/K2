// Send feedback to K2 (prd-k2-product-feedback-v1 PF3–PF5): whether this
// window's dialog is open, which tab, and where it was opened from. Per-window
// view state (each window has its own JS context), never a daemon setting:
// the daemon owns the outbox, the switches and the remembered choice.
//
// Opened by the top-bar button and by the two menu items (the macOS app
// menu's `menu:send-feedback`, the in-window Menu's K2 section), so Zen,
// which has no top bar, still reaches it.

import { create } from 'zustand'
import { usePageViewStore } from '@/stores/page-view'
import { useSettingsStore } from '@/stores/settings'

export type SendFeedbackTab = 'send' | 'sent'

interface SendFeedbackStore {
  open: boolean
  tab: SendFeedbackTab
  /** Where the dialog was opened from (Agents, Settings, Zen…): the
   *  diagnostics' "Where you were" line, only when they are included. */
  openedFrom: string | null
  openDialog: (opts?: { tab?: SendFeedbackTab; from?: string }) => void
  setTab: (tab: SendFeedbackTab) => void
  close: () => void
}

const PAGE_LABELS: Record<string, string> = {
  home: 'Home',
  agents: 'Agents',
  projects: 'Projects',
  feedback: 'Tickets',
  wiki: 'Wiki',
}

/** The page this window shows now, as the person would name it. */
export function currentPageLabel(): string {
  try {
    if (useSettingsStore.getState().settingsOpen) return 'Settings'
  } catch {
    // The settings store isn't loaded in every window kind.
  }
  return PAGE_LABELS[usePageViewStore.getState().page] ?? 'K2'
}

export const useSendFeedbackStore = create<SendFeedbackStore>((set) => ({
  open: false,
  tab: 'send',
  openedFrom: null,
  openDialog: (opts) => set({ open: true, tab: opts?.tab ?? 'send', openedFrom: opts?.from ?? currentPageLabel() }),
  setTab: (tab) => set({ tab }),
  close: () => set({ open: false }),
}))

/** Open the dialog (menus and the button). */
export function openSendFeedback(from?: string): void {
  useSendFeedbackStore.getState().openDialog(from ? { from } : undefined)
}
