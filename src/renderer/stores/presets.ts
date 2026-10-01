import { create } from 'zustand'
import { emit } from '@tauri-apps/api/event'
// Plan B — agent presets are host-aware daemon data: route them through the
// `/cli/presets/*` HTTP layer (local OR remote) instead of the
// localhost-pinned Tauri `presets_*` invoke proxy. The old Tauri shims
// (commands/agents.rs) emitted `sync:presets` from Rust on each mutation so
// other windows re-fetch; we now re-emit that event from the renderer after
// each successful mutation (see `emitPresetsChanged`).
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { ensureOneCli, installableCliProgram } from '@/lib/ensure-cli'
import { parseCommand } from '@/lib/agent-resolve'
import { onActiveHostChange } from '@/stores/connect-host'
import { useTabsStore, registerPresetsStore } from './tabs'
import type { TerminalPane, Tab, PaneGroup, Item } from './tabs'
import { primaryScope } from '@/kessel/server-scope'
import { hostScopedKey } from '@/lib/host-scoped-storage'

/** One boolean per connected host. Missing key means the launch strip is shown. */
export const SHOW_LAUNCH_BAR_STORAGE_PREFIX = 'k2.showLaunchBar.'

/** `<hostKey>|k2.showLaunchBar` — keyed by the server's host key
 *  (`lib/host-key.ts`), not the client-made host id (Home M1, MS6). The old
 *  `k2.showLaunchBar.<id:host:port>` keys are migrated once. */
export function showLaunchBarStorageKey(hostKey: string): string {
  return hostScopedKey(hostKey, SHOW_LAUNCH_BAR_STORAGE_PREFIX.slice(0, -1))
}

export function readShowLaunchBar(hostKey: string): boolean {
  try {
    if (typeof localStorage === 'undefined') return true
    const raw = localStorage.getItem(showLaunchBarStorageKey(hostKey))
    if (raw == null) return true
    return raw !== '0' && raw !== 'false'
  } catch {
    return true
  }
}

function writeShowLaunchBar(hostKey: string, shown: boolean): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.setItem(showLaunchBarStorageKey(hostKey), shown ? '1' : '0')
  } catch {
    /* private mode / sandboxed storage */
  }
}

function currentHostKey(): string {
  return primaryScope().hostKey
}

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

export type InjectFlowKey = 'paste' | 'esc' | 'space' | 'return'

export interface InjectFlowStep {
  key: InjectFlowKey
  waitMs: number
}

/** D5 — visual/runtime default when `inject_flow` is NULL. */
export const DEFAULT_INJECT_FLOW: InjectFlowStep[] = [
  { key: 'paste', waitMs: 150 },
  { key: 'return', waitMs: 250 },
  { key: 'return', waitMs: 120 },
]

/** Grok: paste then one Return. Steer made the second CR a new turn. */
export const GROK_INJECT_FLOW: InjectFlowStep[] = [
  { key: 'paste', waitMs: 150 },
  { key: 'return', waitMs: 250 },
]

export function cloneDefaultInjectFlow(): InjectFlowStep[] {
  return DEFAULT_INJECT_FLOW.map((s) => ({ ...s }))
}

export function cloneGrokInjectFlow(): InjectFlowStep[] {
  return GROK_INJECT_FLOW.map((s) => ({ ...s }))
}

export function programIsGrok(command: string | null | undefined): boolean {
  const token = (command ?? '').trim().split(/\s+/)[0] ?? ''
  const base = token.split(/[/\\]/).pop()?.toLowerCase() ?? ''
  return base === 'grok' || base === 'grok.exe' || base === 'grok.cmd'
}

export function cloneDefaultInjectFlowForCommand(command: string): InjectFlowStep[] {
  return programIsGrok(command) ? cloneGrokInjectFlow() : cloneDefaultInjectFlow()
}

export function isDefaultInjectFlow(steps: InjectFlowStep[]): boolean {
  return JSON.stringify(steps) === JSON.stringify(DEFAULT_INJECT_FLOW)
}

export function isDefaultInjectFlowForCommand(
  steps: InjectFlowStep[],
  command: string,
): boolean {
  const expected = programIsGrok(command) ? GROK_INJECT_FLOW : DEFAULT_INJECT_FLOW
  return JSON.stringify(steps) === JSON.stringify(expected)
}

/** GET is snake_case `inject_flow`; accept camelCase if a caller has it. */
export function readPresetInjectFlowJson(preset: {
  inject_flow?: string | null
  injectFlow?: string | null
}): string | null {
  if (typeof preset.inject_flow === 'string' && preset.inject_flow.length > 0) {
    return preset.inject_flow
  }
  if (typeof preset.injectFlow === 'string' && preset.injectFlow.length > 0) {
    return preset.injectFlow
  }
  return null
}

export function parseInjectFlowOrDefault(
  raw: string | null | undefined,
  command?: string,
): InjectFlowStep[] {
  const fallback = cloneDefaultInjectFlowForCommand(command ?? '')
  if (!raw) return fallback
  try {
    const parsed: unknown = JSON.parse(raw)
    if (!Array.isArray(parsed) || parsed.length === 0) return fallback
    const steps: InjectFlowStep[] = []
    for (const item of parsed) {
      if (!item || typeof item !== 'object') return fallback
      const rec = item as { key?: unknown; waitMs?: unknown }
      if (rec.key !== 'paste' && rec.key !== 'esc' && rec.key !== 'space' && rec.key !== 'return') {
        return fallback
      }
      if (typeof rec.waitMs !== 'number' || !Number.isInteger(rec.waitMs)) {
        return fallback
      }
      steps.push({ key: rec.key, waitMs: rec.waitMs })
    }
    return steps
  } catch {
    return fallback
  }
}

export interface AgentPreset {
  id: string
  label: string
  command: string
  icon: string | null
  enabled: number
  sortOrder: number
  isBuiltIn: number
  createdAt: number
  /** GET snake_case. NULL = program default (Grok: one Return; else paste/return/return). */
  inject_flow?: string | null
}

interface PresetsState {
  presets: AgentPreset[]
  showPresetsBar: boolean
  fetchPresets: () => Promise<void>
  togglePresetsBar: () => void
  /** Persist the launch-strip bit for the active host and update the store. */
  setShowLaunchBar: (shown: boolean) => void
  /**
   * `groupIndex` aims a `'tab'` launch at that column. Omitted keeps
   * `activeGroupIndex` (launch strip, Cmd+Shift+T).
   */
  launchPreset: (
    presetId: string,
    cwd: string,
    mode: 'tab' | 'split',
    groupIndex?: number,
  ) => Promise<void>
  // Mutations — each posts to the daemon then emits `sync:presets` on
  // success and refreshes the local list (mirrors the old Tauri shims).
  createPreset: (input: { label: string; command: string; icon?: string }) => Promise<AgentPreset>
  updatePreset: (input: {
    id: string
    label?: string
    command?: string
    icon?: string
    enabled?: number
    sortOrder?: number
    /** POST camelCase. `""` = NULL (Reset). Omitted = unchanged. */
    injectFlow?: string
  }) => Promise<void>
  deletePreset: (id: string) => Promise<void>
  reorderPresets: (ids: string[]) => Promise<void>
  resetPresetsToBuiltIns: () => Promise<void>
}

// ── Helpers ──────────────────────────────────────────────────────────────

// parseCommand moved to the PURE agent-resolution seam (@/lib/agent-resolve)
// so non-store code and tests can use it without pulling in zustand/tabs.
// Re-exported here so existing importers keep working.
export { parseCommand }

// ── Store ────────────────────────────────────────────────────────────────

export const usePresetsStore = create<PresetsState>((set, get) => ({
  presets: [],
  showPresetsBar: readShowLaunchBar(currentHostKey()),

  fetchPresets: async () => {
    try {
      const result = await daemonCliGet<AgentPreset[]>(primaryScope(), 'presets/list')
      set({ presets: result })
    } catch (err) {
      console.error('Failed to fetch presets:', err)
    }
  },

  togglePresetsBar: () => {
    get().setShowLaunchBar(!get().showPresetsBar)
  },

  setShowLaunchBar: (shown: boolean) => {
    writeShowLaunchBar(currentHostKey(), shown)
    set({ showPresetsBar: shown })
  },

  // POST body is camelCase (the daemon's PresetsCreateBody/PresetsUpdateBody
  // deserialize `sortOrder` etc.). Omit `icon` to leave it unset on create.
  createPreset: async (input) => {
    const created = await daemonCliPost<AgentPreset>(primaryScope(), 'presets/create', input)
    emitPresetsChanged()
    await get().fetchPresets()
    return created
  },

  updatePreset: async (input) => {
    await daemonCliPost(primaryScope(), 'presets/update', input)
    emitPresetsChanged()
    await get().fetchPresets()
  },

  deletePreset: async (id) => {
    await daemonCliPost(primaryScope(), 'presets/delete', { id })
    emitPresetsChanged()
    await get().fetchPresets()
  },

  reorderPresets: async (ids) => {
    await daemonCliPost(primaryScope(), 'presets/reorder', { ids })
    emitPresetsChanged()
    await get().fetchPresets()
  },

  resetPresetsToBuiltIns: async () => {
    await daemonCliPost(primaryScope(), 'presets/reset', {})
    emitPresetsChanged()
    await get().fetchPresets()
  },

  launchPreset: async (presetId: string, cwd: string, mode: 'tab' | 'split', groupIndex?: number) => {
    const preset = get().presets.find((p) => p.id === presetId)
    if (!preset) {
      console.error(`[presets] Preset not found: ${presetId}`)
      return
    }

    const { command, args } = parseCommand(preset.command)
    // Only the four launch basenames. A path token is not an install.
    const program = installableCliProgram(command)
    if (program) {
      try {
        await ensureOneCli(program)
      } catch (err) {
        console.error('[presets] ensure-cli failed:', err)
        return
      }
    }

    const tabsStore = useTabsStore.getState()
    const openTab = (group: number): void => {
      tabsStore.addTabToGroup(group, cwd, {
        title: preset.label,
        command,
        args,
      })
    }

    if (mode === 'tab') {
      openTab(groupIndex ?? tabsStore.activeGroupIndex)
      return
    }

    // Split mode: split the active tab. Ensure already ran, so a missing
    // active tab opens a tab without calling ensure a second time.
    const activeTab = tabsStore.tabs.find((t) => t.id === tabsStore.activeTabId)
    if (!activeTab) {
      openTab(tabsStore.activeGroupIndex)
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
      args,
    }

    tabsStore.splitPane(activeTab.id, firstPaneId, newPaneId, newPane, 'column')
  }
}))

// Register with tabs store to break circular dependency
registerPresetsStore(() => usePresetsStore.getState())

// Host switch loads that host's bit. A missing key stays shown.
// Session mint fires with the same key and must not rewrite the flag.
onActiveHostChange((nextKey, prevKey) => {
  if (nextKey === prevKey) return
  usePresetsStore.setState({ showPresetsBar: readShowLaunchBar(primaryScope().hostKey) })
})

// ── Tree helpers ─────────────────────────────────────────────────────────

function getFirstLeaf(tree: unknown): string | null {
  if (tree === null || tree === undefined) return null
  if (typeof tree === 'string') return tree
  if (typeof tree === 'object' && tree !== null && 'first' in tree) {
    return getFirstLeaf((tree as { first: unknown }).first)
  }
  return null
}
