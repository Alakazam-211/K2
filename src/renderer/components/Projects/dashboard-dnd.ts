// Projects V1 P5 (§6.2) → §6.8 — the tiny bridge that lets drags and
// commands born OUTSIDE the dashboard subtree land on the mounted
// ProjectDashboard:
//
//   - a MEMBER-ROW drag (ProjectNav's drawer, Sidebar's mouse-threshold
//     pattern) or an HTML-DOC drag streams the pointer into this store
//     (the dashboard renders its 5-zone drop highlights from it); on
//     mouseup the initiator hands the final point to whichever
//     dashboard registered the drop handler,
//   - the presets menu (the tab-row button in ProjectsPage, §6.8.4)
//     sends its shape through the preset handler the same way.
//
// The INITIATOR owns the drag lifecycle exactly like
// Sidebar.handleProjectMouseDown: mousedown → 3/5px threshold →
// document mousemove/mouseup. No dashboard mounted → the drag/command
// simply cancels. Last-mount-wins (only one dashboard renders at a
// time — the tab is exclusive).

import { create } from 'zustand'
import type { PresetShape } from './dashboard-layout'

/** What an external drag carries (§6.8.2): a member resolves to its
 *  canonical terminal pane; an HTML doc to its #587 doc pane. Existing
 *  panes (header drags) are dashboard-internal and never cross this
 *  bridge. */
export type DashDragPayload =
  | { type: 'member'; workspaceId: string }
  | { type: 'htmlDoc'; workspaceId: string; filePath: string }

export interface DashDrag {
  payload: DashDragPayload
  x: number
  y: number
}

interface DashboardDndState {
  /** Live external drag, or null. The dashboard subscribes for the
   *  drop-zone highlights. */
  drag: DashDrag | null
  setDrag: (drag: DashDrag | null) => void
}

export const useDashboardDndStore = create<DashboardDndState>((set) => ({
  drag: null,
  setDrag: (drag) => set({ drag }),
}))

type DashDropHandler = (payload: DashDragPayload, x: number, y: number) => void

let dropHandler: DashDropHandler | null = null

/** The mounted dashboard registers itself as the drop target; returns
 *  the unregister. */
export function registerDashDropHandler(handler: DashDropHandler): () => void {
  dropHandler = handler
  return () => {
    if (dropHandler === handler) dropHandler = null
  }
}

/** Initiator mouseup → offer the drop to the registered dashboard. */
export function completeDashDrag(payload: DashDragPayload, x: number, y: number): void {
  dropHandler?.(payload, x, y)
}

// ── Presets (§6.8.4) ──────────────────────────────────────────────────────

type PresetHandler = (shape: PresetShape) => void

let presetHandler: PresetHandler | null = null

/** The mounted dashboard registers its re-tile handler; returns the
 *  unregister. */
export function registerPresetHandler(handler: PresetHandler): () => void {
  presetHandler = handler
  return () => {
    if (presetHandler === handler) presetHandler = null
  }
}

/** The tab-row presets menu → re-tile the open dashboard. */
export function requestPreset(shape: PresetShape): void {
  presetHandler?.(shape)
}

// ── ⌘1…⌘9 pane switching ─────────────────────────────────────────────────
// The page-level capture keydown (ProjectsPage) lands the shortcut on
// whichever dashboard is mounted — no dashboard (Feedback tab open) →
// the combo is still swallowed there, but no-ops here.

type PaneShortcutHandler = (num: number) => void

let paneShortcutHandler: PaneShortcutHandler | null = null

/** The mounted dashboard registers its ⌘N focus handler; returns the
 *  unregister. */
export function registerPaneShortcutHandler(handler: PaneShortcutHandler): () => void {
  paneShortcutHandler = handler
  return () => {
    if (paneShortcutHandler === handler) paneShortcutHandler = null
  }
}

/** The Projects page's Cmd+digit keydown → focus pane N. */
export function requestPaneShortcut(num: number): void {
  paneShortcutHandler?.(num)
}
