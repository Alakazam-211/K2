// Per-client selected-tab store (per-client-view-state.md, Phase 2).
//
// The user's SELECTED tab is per-client VIEW state — never canonical, never
// transmitted, never adopted from a peer. Pre-0.39.43 it leaked into the
// shared workspace layout (`serializeCurrentLayout` → `workspace-layouts/save`
// → adopted by peers on `TabOrderChanged` / cold-load), so one client's
// reorder/save hijacked another client's selected tab. This store moves the
// selection into local-only memory, keyed by `${projectId}:${workspaceId}`.
//
// Persistence is best-effort `localStorage` so a fresh load (same machine/app
// instance) restores the prior selection synchronously. In a `node`/headless
// env (vitest, the headless daemon's renderer-less path) `localStorage` is
// absent — the store degrades to a pure in-memory map, which is correct: there
// is no per-client view to restore there anyway.

import { create } from 'zustand'
import { scopedKey, type ServerScope } from '@/kessel/server-scope'
import {
  SELECTED_TABS_STORAGE_KEY,
  ensureHostScopedKeysMigrated,
} from '@/lib/host-scoped-storage'

/** localStorage key for the persisted selection map. */
const STORAGE_KEY = SELECTED_TABS_STORAGE_KEY

/** Build the per-(server, workspace) selection key:
 *  `<hostKey>|${projectId}:${workspaceId}` (Home M1, MS6). The part after
 *  the host key mirrors the `activeWorkspaceKey` shape the tabs store uses.
 *  Older unprefixed entries are migrated once to `local|…`. */
function keyFor(scope: ServerScope, projectId: string, workspaceId: string): string {
  return scopedKey(scope, `${projectId}:${workspaceId}`)
}

/** Read the persisted map from localStorage (best-effort; empty in node). */
function hydrateFromStorage(): Record<string, string> {
  ensureHostScopedKeysMigrated()
  try {
    if (typeof localStorage === 'undefined') return {}
    const raw = localStorage.getItem(STORAGE_KEY)
    if (!raw) return {}
    const parsed = JSON.parse(raw) as unknown
    if (parsed && typeof parsed === 'object') {
      // Keep only string→string entries (defensive against corruption).
      const out: Record<string, string> = {}
      for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
        if (typeof v === 'string') out[k] = v
      }
      return out
    }
    return {}
  } catch {
    return {}
  }
}

/** Persist the map to localStorage (best-effort; no-op in node). */
function persistToStorage(map: Record<string, string>): void {
  try {
    if (typeof localStorage === 'undefined') return
    localStorage.setItem(STORAGE_KEY, JSON.stringify(map))
  } catch {
    /* quota / private-mode / unavailable — selection stays in-memory only */
  }
}

interface SelectedTabsState {
  /** `<hostKey>|${projectId}:${workspaceId}` -> selected tabId. */
  selected: Record<string, string>
  /** Persist the user's selected tab for a workspace (local only). */
  setSelected: (scope: ServerScope, projectId: string, workspaceId: string, tabId: string) => void
  /** Read the saved selection (or null if none). */
  getSelected: (scope: ServerScope, projectId: string, workspaceId: string) => string | null
  /** Clear all selections (host switch — selection is per-machine). */
  reset: () => void
}

export const useSelectedTabsStore = create<SelectedTabsState>((set, get) => ({
  // Hydrate synchronously at module load so a cold load sees the prior
  // selection before the first render reads it.
  selected: hydrateFromStorage(),

  setSelected: (scope, projectId, workspaceId, tabId) => {
    const key = keyFor(scope, projectId, workspaceId)
    const prev = get().selected
    if (prev[key] === tabId) return
    const next = { ...prev, [key]: tabId }
    set({ selected: next })
    persistToStorage(next)
  },

  getSelected: (scope, projectId, workspaceId) => {
    return get().selected[keyFor(scope, projectId, workspaceId)] ?? null
  },

  reset: () => {
    if (Object.keys(get().selected).length === 0) return
    set({ selected: {} })
    persistToStorage({})
  },
}))

/** Non-hook accessors so the tabs store (plain module, not a React component)
 *  can read/write without a `useStore` subscription. */
export function getSelectedTab(scope: ServerScope, projectId: string, workspaceId: string): string | null {
  return useSelectedTabsStore.getState().getSelected(scope, projectId, workspaceId)
}

export function setSelectedTab(
  scope: ServerScope,
  projectId: string,
  workspaceId: string,
  tabId: string,
): void {
  useSelectedTabsStore.getState().setSelected(scope, projectId, workspaceId, tabId)
}

export function resetSelectedTabs(): void {
  useSelectedTabsStore.getState().reset()
}
