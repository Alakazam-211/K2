// Pin-to-size shared helper (S7c, reworked for the right-click +
// modal flow, PRD presence-multiplayer §5.5)
//
// v0.40.27 shipped pin-to-size as a pushpin icon-button on both tab
// bars that opened a ContextMenu of presets. That entry point is gone:
// pinning now lives behind the tab's RIGHT-CLICK menu ("Pin
// Dimensions…" opens PinDimensionsModal; "Unpin Dimensions" clears
// instantly). This module keeps the pieces both tab bars and the
// modal share:
//
// - resolvePinSessionId — item→daemon-sessionId resolution; success
//   doubles as the "offer the menu entry at all" gate (PURE).
// - PIN_PRESETS / buildPresetRows — the modal's preset list (PURE).
// - validatePinDims + pinForm* helpers — the modal's field logic
//   (PURE, unit-tested without rendering).
// - applyPinSize — the daemon POST + store mirror (pin AND unpin).
//   Throws on failure; callers surface the message however their
//   surface renders errors (modal inline, tab bar transient hint).

import { usePinnedSizeStore } from '@/stores/pinned-size'
import { agentChatId } from '@/lib/terminal-id'
import { daemonCliPost } from '@/lib/daemon-cli'

// ── Types ────────────────────────────────────────────────────────────────

/** The preset grid sizes offered by the Pin Dimensions modal.
 *  cols × rows, matching the field order in the modal. */
export const PIN_PRESETS: ReadonlyArray<{ cols: number; rows: number }> = [
  { cols: 80, rows: 24 },
  { cols: 100, rows: 30 },
  { cols: 100, rows: 40 },
  { cols: 120, rows: 36 },
  { cols: 120, rows: 40 },
  { cols: 132, rows: 43 },
  { cols: 160, rows: 48 },
  { cols: 160, rows: 50 },
  { cols: 200, rows: 60 },
]

/** Daemon-enforced bounds for /cli/terminal/pin-size (the route
 *  rejects anything outside; the modal validates client-side too). */
export const PIN_BOUNDS = {
  minCols: 20,
  maxCols: 500,
  minRows: 5,
  maxRows: 200,
} as const

/** What the daemon's /cli/terminal/pin-size answers with. */
interface PinSizeResponse {
  success: boolean
  pinned: { cols: number; rows: number; setBy?: string | null } | null
  persisted: boolean
}

/** Structural view of a tab item — matches both PaneTabBar's local
 *  Item shape and the tabs store's `Item` without importing either.
 *  'browser' items (browser-pane arc) have no PTY grid and resolve to
 *  null like file viewers. */
export interface PinnableItem {
  type: 'terminal' | 'file-viewer' | 'agent' | 'browser'
  data: unknown
}

// ── Resolution (pure) ────────────────────────────────────────────────────

/** Daemon session UUID for a tab item, or null when the item has no
 *  live Kessel session (file viewers, work boards, legacy panes,
 *  not-yet-spawned terminals). Terminal items key straight off
 *  their terminalId (`-shell` covers the agent-exit fallback pane);
 *  agent chat items reconstruct the AgentChatPane's terminal id
 *  (`agent-chat:<projectId>`, see terminal-id.ts) via the projects
 *  store's path→id mapping. Resolution success doubles as the
 *  "offer the pin menu entry at all" gate. */
export function resolvePinSessionId(
  item: PinnableItem,
  sessions: Record<string, string>,
  projects: ReadonlyArray<{ id: string; path: string }>,
): string | null {
  if (item.type === 'terminal') {
    const tid = (item.data as { terminalId: string }).terminalId
    return sessions[tid] ?? sessions[`${tid}-shell`] ?? null
  }
  if (item.type === 'agent') {
    const data = item.data as { agentName: string; projectPath: string; section?: string }
    if (data.section !== 'chat') return null
    const project = projects.find((p) => p.path === data.projectPath)
    if (!project) return null
    return sessions[agentChatId(project.id, data.agentName)] ?? null
  }
  return null
}

// ── Modal preset list (pure) ─────────────────────────────────────────────

/** One clickable row in the modal's preset list. */
export interface PinPresetRow {
  id: string
  label: string
  cols: number
  rows: number
}

/** Build the modal's preset rows: the fixed PIN_PRESETS plus — only
 *  when this window has measured dims for the session — a trailing
 *  "Match my window now" row carrying the live numbers. */
export function buildPresetRows(
  dims: { cols: number; rows: number } | null,
): PinPresetRow[] {
  const rows: PinPresetRow[] = PIN_PRESETS.map(({ cols, rows: r }) => ({
    id: `preset:${cols}x${r}`,
    label: `${cols}×${r}`,
    cols,
    rows: r,
  }))
  if (dims) {
    rows.push({
      id: 'match',
      label: `Match my window now (${dims.cols}×${dims.rows})`,
      cols: dims.cols,
      rows: dims.rows,
    })
  }
  return rows
}

// ── Modal field logic (pure) ─────────────────────────────────────────────

export type PinDimsValidation =
  | { ok: true; cols: number; rows: number }
  | { ok: false; error: string }

function parseWhole(raw: string): number | null {
  const trimmed = raw.trim()
  if (!/^\d+$/.test(trimmed)) return null
  return Number(trimmed)
}

/** Validate the modal's two text fields against the daemon's bounds.
 *  Column errors win over row errors (fields are checked in display
 *  order). Non-numeric input — including empty — is invalid. */
export function validatePinDims(colsRaw: string, rowsRaw: string): PinDimsValidation {
  const cols = parseWhole(colsRaw)
  if (cols === null) {
    return { ok: false, error: 'Columns must be a whole number' }
  }
  if (cols < PIN_BOUNDS.minCols || cols > PIN_BOUNDS.maxCols) {
    return {
      ok: false,
      error: `Columns must be between ${PIN_BOUNDS.minCols} and ${PIN_BOUNDS.maxCols}`,
    }
  }
  const rows = parseWhole(rowsRaw)
  if (rows === null) {
    return { ok: false, error: 'Rows must be a whole number' }
  }
  if (rows < PIN_BOUNDS.minRows || rows > PIN_BOUNDS.maxRows) {
    return {
      ok: false,
      error: `Rows must be between ${PIN_BOUNDS.minRows} and ${PIN_BOUNDS.maxRows}`,
    }
  }
  return { ok: true, cols, rows }
}

/** The modal's form state: raw field text + which preset row (if any)
 *  the current values came from. Editing a field clears the selection
 *  — the values have diverged from the clicked preset. */
export interface PinFormState {
  cols: string
  rows: string
  selectedPresetId: string | null
}

/** Initial form state: prefilled from the current pin when the
 *  session is already pinned (re-pin path), empty otherwise. */
export function pinFormFromPin(
  pin: { cols: number; rows: number } | null,
): PinFormState {
  return pin
    ? { cols: String(pin.cols), rows: String(pin.rows), selectedPresetId: null }
    : { cols: '', rows: '', selectedPresetId: null }
}

/** Preset row clicked → populate both fields and highlight the row. */
export function pinFormPresetClicked(row: PinPresetRow): PinFormState {
  return {
    cols: String(row.cols),
    rows: String(row.rows),
    selectedPresetId: row.id,
  }
}

/** Field edited → new value, selection cleared. */
export function pinFormFieldEdited(
  state: PinFormState,
  field: 'cols' | 'rows',
  value: string,
): PinFormState {
  return { ...state, [field]: value, selectedPresetId: null }
}

// ── Daemon POST + store mirror ───────────────────────────────────────────

/** Pin (dims) or unpin (null) a session. POSTs /cli/terminal/pin-size
 *  and mirrors the authoritative answer into the pinned-size store
 *  immediately so menu labels / badges are right without waiting for
 *  the pane's `pin_changed` frame (which still converges the pane
 *  itself). Throws on daemon rejection — callers render the error. */
export async function applyPinSize(
  sessionId: string,
  dims: { cols: number; rows: number } | null,
): Promise<void> {
  if (dims === null) {
    await daemonCliPost<PinSizeResponse>('terminal/pin-size', {
      session: sessionId,
      clear: true,
    })
    usePinnedSizeStore.getState().setPin(sessionId, null)
    return
  }
  const res = await daemonCliPost<PinSizeResponse>('terminal/pin-size', {
    session: sessionId,
    cols: dims.cols,
    rows: dims.rows,
  })
  if (res.pinned) {
    usePinnedSizeStore.getState().setPin(sessionId, {
      cols: res.pinned.cols,
      rows: res.pinned.rows,
      setBy: res.pinned.setBy ?? null,
    })
  }
}
