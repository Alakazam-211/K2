import { create } from 'zustand'
import {
  SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH,
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
import { isSetupChecklistShowing, isSetupNotFinished, useSetupChecklistShowing } from '@/stores/first-run'
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
  /** The width the person chose (kept). What's drawn is
   *  `displayedSidebarWidth`: never under the setup checklist's floor
   *  while the checklist shows. */
  width: number

  toggle: () => void
  setWidth: (width: number) => void
  collapse: () => void
  expand: () => void
  initFromSettings: () => Promise<void>
}

/** The narrowest the sidebar may be drawn: the setup checklist's floor
 *  while it shows (until the intro is done), else the normal minimum. */
export function sidebarMinWidth(checklistShowing: boolean): number {
  return checklistShowing ? SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH : SIDEBAR_MIN_WIDTH
}

/** The width to draw: the saved width, raised to the checklist's floor
 *  while it shows. The saved width itself is left alone. */
export function displayedSidebarWidth(saved: number, checklistShowing: boolean): number {
  return checklistShowing ? Math.max(saved, SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH) : saved
}

/** The width to save for a requested one (a drag, a double-click reset).
 *  Clamped to the range; while the checklist shows, a request at or under
 *  its floor keeps a narrower saved width (the floor is drawn either way),
 *  so the person's own width comes back once the checklist goes. */
export function nextSavedSidebarWidth(saved: number, requested: number, checklistShowing: boolean): number {
  const next = Math.max(sidebarMinWidth(checklistShowing), Math.min(SIDEBAR_MAX_WIDTH, requested))
  if (checklistShowing && next === SETUP_CHECKLIST_MIN_SIDEBAR_WIDTH && saved < next) return saved
  return next
}

/** The sidebar's drawn width for this window. */
export function useSidebarDisplayWidth(): number {
  const saved = useSidebarStore((s) => s.width)
  return displayedSidebarWidth(saved, useSetupChecklistShowing())
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

  // Until setup is finished (every step done or skipped), the nav doesn't
  // collapse (⌘B, the button, any caller), whether the list is open or
  // hidden as its Get started button. Opening a collapsed one still works,
  // and a nav already collapsed stays so.
  toggle: () => {
    const next = !get().isCollapsed
    if (next && isSetupNotFinished()) return
    set({ isCollapsed: next })
    persistCollapsed(next)
  },

  setWidth: (width: number) =>
    set({ width: nextSavedSidebarWidth(get().width, width, isSetupChecklistShowing()) }),

  collapse: () => {
    if (isSetupNotFinished()) return
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
