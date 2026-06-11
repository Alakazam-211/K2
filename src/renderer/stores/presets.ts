import { create } from 'zustand'
import { emit } from '@tauri-apps/api/event'
// Plan B — agent presets are host-aware daemon data: route them through the
// `/cli/presets/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `presets_*` invoke proxy. The old Tauri shims
// (commands/agents.rs) emitted `sync:presets` from Rust on each mutation so
// other windows re-fetch; we now re-emit that event from the renderer after
// each successful mutation (see `emitPresetsChanged`).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { useTabsStore, registerPresetsStore } from './tabs'
import type { TerminalPane, Tab, PaneGroup, Item } from './tabs'

/**
 * Plan B cross-window sync: the old Tauri `presets_*` mutation commands
 * emitted `sync:presets` from Rust so OTHER windows re-fetch their preset
 * list. Now that the renderer talks to the daemon directly, that Rust-side
 * emit no longer fires — so we emit the SAME event from the renderer after
 * each successful mutation. `useWindowSync.ts` listens for it and calls
 * `fetchPresets()`. Fire-and-forget; a failed emit (non-Tauri/web) is
 * non-fatal — only cross-window refresh is affected.
 */
function emitPresetsChanged(): void {
  void emit('sync:presets').catch((e) =>
    console.warn('[presets] sync:presets emit failed:', e),
  )
}

// ── Types ────────────────────────────────────────────────────────────────

export interface AgentPreset {
  id: string
  label: string
  command: string
  icon: string | null
  enabled: number
  sortOrder: number
  isBuiltIn: number
  createdAt: number
}

interface PresetsState {
  presets: AgentPreset[]
  showPresetsBar: boolean
  fetchPresets: () => Promise<void>
  togglePresetsBar: () => void
  launchPreset: (presetId: string, cwd: string, mode: 'tab' | 'split') => void
  // Mutations — each posts to the daemon then emits `sync:presets` on
  // success and refreshes the local list (mirrors the old Tauri shims).
  createPreset: (input: { label: string; command: string; icon?: string }) => Promise<void>
  updatePreset: (input: {
    id: string
    label?: string
    command?: string
    icon?: string
    enabled?: number
    sortOrder?: number
  }) => Promise<void>
  deletePreset: (id: string) => Promise<void>
  reorderPresets: (ids: string[]) => Promise<void>
  resetPresetsToBuiltIns: () => Promise<void>
}

// ── Helpers ──────────────────────────────────────────────────────────────

export function parseCommand(commandStr: string): { command: string; args: string[] } {
  // Split respecting quoted strings
  const parts: string[] = []
  let current = ''
  let inQuote: string | null = null

  for (let i = 0; i < commandStr.length; i++) {
    const ch = commandStr[i]

    if (inQuote) {
      if (ch === inQuote) {
        inQuote = null
      } else {
        current += ch
      }
    } else if (ch === '"' || ch === "'") {
      inQuote = ch
    } else if (ch === ' ') {
      if (current.length > 0) {
        parts.push(current)
        current = ''
      }
    } else {
      current += ch
    }
  }
  if (current.length > 0) {
    parts.push(current)
  }

  const [command, ...args] = parts
  return { command: command || '', args }
}

// ── Store ────────────────────────────────────────────────────────────────

export const usePresetsStore = create<PresetsState>((set, get) => ({
  presets: [],
  showPresetsBar: true,

  fetchPresets: async () => {
    try {
      const result = await daemonCliGet<AgentPreset[]>('presets/list')
      set({ presets: result })
    } catch (err) {
      console.error('Failed to fetch presets:', err)
    }
  },

  togglePresetsBar: () => {
    set((state) => ({ showPresetsBar: !state.showPresetsBar }))
  },

  // POST body is camelCase (the daemon's PresetsCreateBody/PresetsUpdateBody
  // deserialize `sortOrder` etc.). Omit `icon` to leave it unset on create.
  createPreset: async (input) => {
    await daemonCliPost('presets/create', input)
    emitPresetsChanged()
    await get().fetchPresets()
  },

  updatePreset: async (input) => {
    await daemonCliPost('presets/update', input)
    emitPresetsChanged()
    await get().fetchPresets()
  },

  deletePreset: async (id) => {
    await daemonCliPost('presets/delete', { id })
    emitPresetsChanged()
    await get().fetchPresets()
  },

  reorderPresets: async (ids) => {
    await daemonCliPost('presets/reorder', { ids })
    emitPresetsChanged()
    await get().fetchPresets()
  },

  resetPresetsToBuiltIns: async () => {
    await daemonCliPost('presets/reset', {})
    emitPresetsChanged()
    await get().fetchPresets()
  },

  launchPreset: (presetId: string, cwd: string, mode: 'tab' | 'split') => {
    const preset = get().presets.find((p) => p.id === presetId)
    if (!preset) {
      console.error(`[presets] Preset not found: ${presetId}`)
      return
    }

    const { command, args } = parseCommand(preset.command)
    const tabsStore = useTabsStore.getState()

    if (mode === 'tab') {
      // Use addTabToGroup which respects the active group
      const activeGroup = tabsStore.activeGroupIndex
      tabsStore.addTabToGroup(activeGroup, cwd, {
        title: preset.label,
        command,
        args
      })
    } else {
      // Split mode: split the active tab
      const activeTab = tabsStore.tabs.find((t) => t.id === tabsStore.activeTabId)
      if (!activeTab) {
        // No active tab, create one instead
        get().launchPreset(presetId, cwd, 'tab')
        return
      }

      const firstPaneId = getFirstLeaf(activeTab.mosaicTree)
      if (!firstPaneId) return

      const newPaneId = crypto.randomUUID()
      const newPane: TerminalPane = {
        type: 'terminal',
        terminalId: newPaneId,
        cwd,
        command,
        args
      }

      tabsStore.splitPane(activeTab.id, firstPaneId, newPaneId, newPane, 'column')
    }
  }
}))

// Register with tabs store to break circular dependency
registerPresetsStore(() => usePresetsStore.getState())

// ── Tree helpers ─────────────────────────────────────────────────────────

function getFirstLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getFirstLeaf((tree as { first: unknown }).first)
  }
  return null
}
