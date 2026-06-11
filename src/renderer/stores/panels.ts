import { create } from 'zustand'
import {
  SIDEBAR_DEFAULT_WIDTH,
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH
} from '../../shared/constants'
// Phase 2 Unit 7a — settings live in the daemon.
import { settingsGet, settingsUpdate } from '@/lib/daemon-settings'
// Phase 2.5 fix (finding #547) — retry the initial load when the
// daemon comes online after a slow boot or restart.
import { onDaemonConnected } from '@/lib/daemon-reconnect'
// #625 — re-init panel layout against the NEW host on a host switch.
import { onActiveHostChange } from '@/stores/connect-host'

type PanelTab = 'files' | 'changes' | 'history' | 'workspace'

interface PanelsState {
  // Left auxiliary panel (between projects sidebar and terminal)
  leftPanelOpen: boolean
  leftPanelWidth: number
  leftPanelActiveTab: PanelTab
  leftPanelTabs: PanelTab[]

  // Right auxiliary panel (right of terminal)
  rightPanelOpen: boolean
  rightPanelWidth: number
  rightPanelActiveTab: PanelTab
  rightPanelTabs: PanelTab[]

  // Focus window: which side shows the workspace header
  focusWorkspaceHeaderSide: 'left' | 'right'

  // Actions
  toggleLeftPanel: () => void
  toggleRightPanel: () => void
  setLeftPanelWidth: (width: number) => void
  setRightPanelWidth: (width: number) => void
  setLeftPanelActiveTab: (tab: PanelTab) => void
  setRightPanelActiveTab: (tab: PanelTab) => void

  // Move a tab from one side to the other
  moveTabToLeft: (tab: PanelTab) => void
  moveTabToRight: (tab: PanelTab) => void

  // Move workspace header between sides (focus window)
  moveFocusWorkspaceHeader: (side: 'left' | 'right') => void

  /** Activate a specific tab on whichever side has it, opening the panel if needed. */
  activateTab: (tab: PanelTab) => void

  initFromSettings: () => Promise<void>
}

let panelsInitialized = false

/** Phase 2.5 fix (finding #547) — `hasLoadedFromDaemon` defense-in-
 *  depth gate. Until the daemon hands back a real settings snapshot,
 *  every `settingsUpdate(...)` call from this store is a no-op (the
 *  UI still updates locally — interactions stay responsive — but we
 *  refuse to clobber settings.json with hard-coded defaults). Sub-fix
 *  A's port stability + Sub-fix C's reconnect retry should make this
 *  gate non-triggering in steady state; it exists for the edge case
 *  where the user clicks a panel toggle in the window between app
 *  boot and the first successful `initFromSettings`. */
let hasLoadedFromDaemon = false

/** Test-only — reset both gates so tests don't leak state. */
export function __resetPanelsLoadGateForTests(): void {
  panelsInitialized = false
  hasLoadedFromDaemon = false
}

/** Suppress a settings write when we haven't yet confirmed the
 *  daemon's baseline. Returns `true` to indicate the caller should
 *  skip the actual `settingsUpdate(...)`. */
function shouldSuppressPersist(): boolean {
  if (hasLoadedFromDaemon) return false
  console.warn(
    '[panels] persist suppressed: settings not yet loaded from daemon ' +
    '(early-boot click before initFromSettings completed; UI state stays local)'
  )
  return true
}

export const usePanelsStore = create<PanelsState>((set, get) => ({
  leftPanelOpen: true,
  leftPanelWidth: SIDEBAR_DEFAULT_WIDTH,
  leftPanelActiveTab: 'files',
  leftPanelTabs: ['files', 'workspace'],

  rightPanelOpen: true,
  rightPanelWidth: SIDEBAR_DEFAULT_WIDTH,
  rightPanelActiveTab: 'history',
  rightPanelTabs: ['history', 'changes'],

  focusWorkspaceHeaderSide: 'left',

  toggleLeftPanel: () => {
    const next = !get().leftPanelOpen
    set({ leftPanelOpen: next })
    if (shouldSuppressPersist()) return
    settingsUpdate({ leftPanelOpen: next }).catch((e: unknown) => console.error('[panels]', e))
  },
  toggleRightPanel: () => {
    const next = !get().rightPanelOpen
    set({ rightPanelOpen: next })
    if (shouldSuppressPersist()) return
    settingsUpdate({ rightPanelOpen: next }).catch((e: unknown) => console.error('[panels]', e))
  },

  setLeftPanelWidth: (width) =>
    set({ leftPanelWidth: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),
  setRightPanelWidth: (width) =>
    set({ rightPanelWidth: Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, width)) }),

  setLeftPanelActiveTab: (tab) => {
    set({ leftPanelActiveTab: tab })
    if (shouldSuppressPersist()) return
    settingsUpdate({ leftPanelActiveTab: tab }).catch((e: unknown) => console.error('[panels]', e))
  },
  setRightPanelActiveTab: (tab) => {
    set({ rightPanelActiveTab: tab })
    if (shouldSuppressPersist()) return
    settingsUpdate({ rightPanelActiveTab: tab }).catch((e: unknown) => console.error('[panels]', e))
  },

  moveTabToLeft: (tab) => {
    set((s) => ({
      leftPanelTabs: s.leftPanelTabs.includes(tab) ? s.leftPanelTabs : [...s.leftPanelTabs, tab],
      rightPanelTabs: s.rightPanelTabs.filter((t) => t !== tab),
      leftPanelActiveTab: tab,
      rightPanelActiveTab:
        s.rightPanelActiveTab === tab
          ? s.rightPanelTabs.find((t) => t !== tab) ?? s.rightPanelActiveTab
          : s.rightPanelActiveTab
    }))
    if (shouldSuppressPersist()) return
    const s = get()
    settingsUpdate({
      leftPanelTabs: s.leftPanelTabs,
      rightPanelTabs: s.rightPanelTabs,
      leftPanelActiveTab: s.leftPanelActiveTab,
      rightPanelActiveTab: s.rightPanelActiveTab
    }).catch((e: unknown) => console.error('[panels]', e))
  },

  moveTabToRight: (tab) => {
    set((s) => ({
      rightPanelTabs: s.rightPanelTabs.includes(tab)
        ? s.rightPanelTabs
        : [...s.rightPanelTabs, tab],
      leftPanelTabs: s.leftPanelTabs.filter((t) => t !== tab),
      rightPanelActiveTab: tab,
      leftPanelActiveTab:
        s.leftPanelActiveTab === tab
          ? s.leftPanelTabs.find((t) => t !== tab) ?? s.leftPanelActiveTab
          : s.leftPanelActiveTab
    }))
    if (shouldSuppressPersist()) return
    const s = get()
    settingsUpdate({
      leftPanelTabs: s.leftPanelTabs,
      rightPanelTabs: s.rightPanelTabs,
      leftPanelActiveTab: s.leftPanelActiveTab,
      rightPanelActiveTab: s.rightPanelActiveTab
    }).catch((e: unknown) => console.error('[panels]', e))
  },

  moveFocusWorkspaceHeader: (side) => {
    set({ focusWorkspaceHeaderSide: side })
  },

  activateTab: (tab: PanelTab) => {
    const state = get()
    // Check left panel first
    if (state.leftPanelTabs.includes(tab)) {
      set({ leftPanelOpen: true, leftPanelActiveTab: tab })
      return
    }
    // Then right panel
    if (state.rightPanelTabs.includes(tab)) {
      set({ rightPanelOpen: true, rightPanelActiveTab: tab })
      return
    }
    // Tab not on either side — add to right panel
    set({
      rightPanelOpen: true,
      rightPanelTabs: [...state.rightPanelTabs, tab],
      rightPanelActiveTab: tab,
    })
  },

  initFromSettings: async () => {
    try {
      const settings = await settingsGet()

      // Migrate old tab names to new ones
      const VALID_TABS: PanelTab[] = ['files', 'changes', 'history', 'workspace']
      const migrateTab = (t: string): PanelTab | null => {
        if (t === 'agents' || t === 'reviews') return 'workspace'
        // 0.36.0 had a short-lived 'heartbeats' drawer tab that has
        // since folded back into the Workspace panel. Drop it from
        // saved layouts so users don't see a stale tab they can't
        // populate.
        if (t === 'heartbeats') return null
        if (VALID_TABS.includes(t as PanelTab)) return t as PanelTab
        return null
      }
      const migrateTabs = (tabs: string[]): PanelTab[] => {
        const mapped = tabs.map(migrateTab).filter((t): t is PanelTab => t !== null)
        // Deduplicate while preserving order
        return [...new Set(mapped)]
      }

      let leftTabs = settings.leftPanelTabs?.length
        ? migrateTabs(settings.leftPanelTabs as string[])
        : ['files', 'workspace'] as PanelTab[]
      let rightTabs = settings.rightPanelTabs?.length
        ? migrateTabs(settings.rightPanelTabs as string[])
        : ['history', 'changes'] as PanelTab[]

      // Ensure 'workspace' only appears on one side — prefer left
      if (leftTabs.includes('workspace') && rightTabs.includes('workspace')) {
        rightTabs = rightTabs.filter((t) => t !== 'workspace')
      }

      let leftActive = settings.leftPanelActiveTab
        ? (migrateTab(settings.leftPanelActiveTab as string) ?? leftTabs[0])
        : leftTabs[0]
      let rightActive = settings.rightPanelActiveTab
        ? (migrateTab(settings.rightPanelActiveTab as string) ?? rightTabs[0])
        : rightTabs[0]

      // Ensure active tabs are valid for their side
      if (!leftTabs.includes(leftActive)) leftActive = leftTabs[0]
      if (!rightTabs.includes(rightActive)) rightActive = rightTabs[0]

      set({
        leftPanelOpen: settings.leftPanelOpen,
        rightPanelOpen: settings.rightPanelOpen,
        leftPanelActiveTab: leftActive,
        rightPanelActiveTab: rightActive,
        leftPanelTabs: leftTabs,
        rightPanelTabs: rightTabs,
      })

      // CRITICAL: flip the persist gate ONLY on success. A
      // catch-and-default branch (below) leaves it false, so a
      // user click that fires before the daemon comes online stays
      // in-memory and doesn't clobber settings.json with defaults.
      // Phase 2.5 fix (finding #547).
      hasLoadedFromDaemon = true

      // Only persist on first init to avoid sync:settings → initFromSettings → settings_update loop
      if (!panelsInitialized) {
        panelsInitialized = true
        setTimeout(() => {
          settingsUpdate({
            leftPanelTabs: leftTabs,
            rightPanelTabs: rightTabs,
            leftPanelActiveTab: leftActive,
            rightPanelActiveTab: rightActive,
          }).catch((e: unknown) => console.error('[panels] migration persist failed:', e))
        }, 2000)
      }
    } catch {
      // ignore — use defaults; `hasLoadedFromDaemon` stays false.
      // The daemon-reconnect listener (registered below) will retry
      // this fn once the daemon-events WS connects.
    }
  }
}))

// Initialize on import
usePanelsStore.getState().initFromSettings()

// Phase 2.5 fix (finding #547): if the initial fetch above raced
// the daemon's HTTP listener and lost (heavy first-boot migrations
// can keep the daemon busy for 5+ seconds), the reconnect-bus fires
// `daemon:connected` once the WS handshakes. Re-running
// `initFromSettings` is cheap (one HTTP GET) and idempotent (sets
// the same state again on success). The gate stays false until a
// successful load, so any user interaction in the meantime stays
// ephemeral instead of persisting defaults.
onDaemonConnected(() => {
  if (hasLoadedFromDaemon) return
  usePanelsStore.getState().initFromSettings()
})

// #625 — on a real active-host CHANGE, drop BOTH load gates (mirroring
// `__resetPanelsLoadGateForTests`) and re-init the panel layout from the
// NEW host's daemon. `initFromSettings()` uses host-aware `settingsGet()`
// (reads `activeHost` at call time), and `onActiveHostChange` fires AFTER
// the flip, so this targets the new host. Resetting `hasLoadedFromDaemon`
// re-arms the suppress-persist gate until the new host's baseline lands;
// resetting `panelsInitialized` lets the one-time tab-migration persist
// run once against the new host.
onActiveHostChange(() => {
  __resetPanelsLoadGateForTests()
  void usePanelsStore.getState().initFromSettings()
})
