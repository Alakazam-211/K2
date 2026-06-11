import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
// Phase 2 Unit 7a — settings live in the daemon.
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'

interface SidebarState {
  isCollapsed: boolean
  width: number

  toggle: () => void
  setWidth: (width: number) => void
  collapse: () => void
  expand: () => void
  initFromSettings: () => Promise<void>
}

export const useSidebarStore = create<SidebarState>((set, get) => ({
  isCollapsed: false,
  width: SIDEBAR_DEFAULT_WIDTH,

  toggle: () => {
    const next = !get().isCollapsed
    set({ isCollapsed: next })
    settingsUpdate({ sidebarCollapsed: next }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  setWidth: (width: number) =>
    set({ width: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  collapse: () => {
    set({ isCollapsed: true })
    settingsUpdate({ sidebarCollapsed: true }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  expand: () => {
    set({ isCollapsed: false })
    settingsUpdate({ sidebarCollapsed: false }).catch((e: unknown) => console.error('[sidebar]', e))
  },

  initFromSettings: async () => {
    try {
      const settings = await settingsGet()
      set({ isCollapsed: settings.sidebarCollapsed })
    } catch {
      // ignore — use defaults
    }
  }
}))

// Initialize on import
useSidebarStore.getState().initFromSettings()
