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

/** localStorage key for the persisted selection map. */
const STORAGE_KEY = 'k2so:selected-tabs'

/** Build the per-(client, workspace) selection key. Mirrors the
 *  `activeWorkspaceKey` shape (`${projectId}:${workspaceId}`) the tabs store
 *  already uses, so the two stay aligned. */
function keyFor(projectId: string, workspaceId: string): string {
  return `${projectId}:${workspaceId}`
}

/** Read the persisted map from localStorage (best-effort; empty in node). */
function hydrateFromStorage(): Record<string, string> {
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
  /** `${projectId}:${workspaceId}` -> selected tabId. */
  selected: Record<string, string>
  /** Persist the user's selected tab for a workspace (local only). */
  setSelected: (projectId: string, workspaceId: string, tabId: string) => void
  /** Read the saved selection (or null if none). */
  getSelected: (projectId: string, workspaceId: string) => string | null
  /** Clear all selections (host switch — selection is per-machine). */
  reset: () => void
}

export const useSelectedTabsStore = create<SelectedTabsState>((set, get) => ({
  // Hydrate synchronously at module load so a cold load sees the prior
  // selection before the first render reads it.
  selected: hydrateFromStorage(),

  setSelected: (projectId, workspaceId, tabId) => {
    const key = keyFor(projectId, workspaceId)
    const prev = get().selected
    if (prev[key] === tabId) return
    const next = { ...prev, [key]: tabId }
    set({ selected: next })
    persistToStorage(next)
  },

  getSelected: (projectId, workspaceId) => {
    return get().selected[keyFor(projectId, workspaceId)] ?? null
  },

  reset: () => {
    if (Object.keys(get().selected).length === 0) return
    set({ selected: {} })
    persistToStorage({})
  },
}))

/** Non-hook accessors so the tabs store (plain module, not a React component)
 *  can read/write without a `useStore` subscription. */
export function getSelectedTab(projectId: string, workspaceId: string): string | null {
  return useSelectedTabsStore.getState().getSelected(projectId, workspaceId)
}

export function setSelectedTab(projectId: string, workspaceId: string, tabId: string): void {
  useSelectedTabsStore.getState().setSelected(projectId, workspaceId, tabId)
}

export function resetSelectedTabs(): void {
  useSelectedTabsStore.getState().reset()
}
