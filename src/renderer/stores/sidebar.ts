import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
import { settingsGet } from '@/lib/daemon-settings'
import {
  getWindowLabel,
  isFocusWindowLabel,
  readWindowChrome,
  seedWindowChromeIfMissing,
  writeWindowChrome,
} from '@/lib/window-chrome'

interface SidebarState {
  isCollapsed: boolean
  width: number

  toggle: () => void
  setWidth: (width: number) => void
  collapse: () => void
  expand: () => void
  initFromSettings: () => Promise<void>
}

/** First chrome hydrate. Later sync:settings must not restamp collapsed
 *  from the daemon. No host-change hook — chrome is this window. */
let railHydrated = false

const bootLabel = getWindowLabel()
const bootChrome = isFocusWindowLabel(bootLabel) ? null : readWindowChrome(bootLabel)

function persistCollapsed(next: boolean): void {
  const label = getWindowLabel()
  if (isFocusWindowLabel(label)) return
  writeWindowChrome({ sidebarCollapsed: next }, label)
}

export function __resetSidebarChromeForTests(): void {
  railHydrated = false
}

export const useSidebarStore = create<SidebarState>((set, get) => ({
  isCollapsed: bootChrome?.sidebarCollapsed ?? false,
  width: SIDEBAR_DEFAULT_WIDTH,

  toggle: () => {
    const next = !get().isCollapsed
    set({ isCollapsed: next })
    persistCollapsed(next)
  },

  setWidth: (width: number) =>
    set({ width: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  collapse: () => {
    set({ isCollapsed: true })
    persistCollapsed(true)
  },

  expand: () => {
    set({ isCollapsed: false })
    persistCollapsed(false)
  },

  initFromSettings: async () => {
    if (railHydrated) return
    const label = getWindowLabel()
    if (isFocusWindowLabel(label)) {
      railHydrated = true
      return
    }
    try {
      const settings = await settingsGet()
      if (railHydrated) return
      const chrome = seedWindowChromeIfMissing({
        leftPanelOpen: settings.leftPanelOpen,
        rightPanelOpen: settings.rightPanelOpen,
        sidebarCollapsed: settings.sidebarCollapsed,
      }, label)
      set({ isCollapsed: chrome.sidebarCollapsed })
      railHydrated = true
    } catch {
      // ignore — use defaults / local chrome
    }
  }
}))

// Initialize on import
useSidebarStore.getState().initFromSettings()
