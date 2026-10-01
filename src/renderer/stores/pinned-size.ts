// Pin-to-size store (S7b, PRD presence-multiplayer §5.5)
//
// The daemon owns the canonical pin (S7a: /cli/terminal/pin-size +
// `pin_initial`/`pin_changed` grid-WS frames; while pinned it clamps
// EVERY resize path). The renderer is a thin mirror, exactly like
// `session-labels.ts`: TerminalPane's grid-WS handler writes pin
// frames in here, and any surface that needs pin state (the pane's
// badge/letterbox math reads its own local state; PaneTabBar's
// pin-size menu reads THIS store) subscribes by daemon sessionId.
//
// Why the extra two maps beyond sessionId→pin:
// - `sessions` (terminalId → daemon sessionId): the tab bar only
//   knows the renderer-side terminalId of its items; the pin-size
//   route wants the daemon session UUID. TerminalPane registers the
//   mapping when its idempotent spawn resolves (same moment it
//   stashes `sessionIdRef`), so "resolves to a live session" and
//   "menu is offered" are the same condition. No polling.
// - `dims` (sessionId → this window's latest measured grid fit):
//   "Match my window now" freezes the size THIS window would give
//   the PTY. TerminalPane's sendResize already computes exactly
//   that on every ResizeObserver pass — it records the dims here
//   even while pinned (emission is skipped, measurement is not),
//   so an unpin→match round-trip uses fresh numbers.

import { create } from 'zustand'
import { scopedKey, type ServerScope } from '@/kessel/server-scope'

// Home M1 (MS14): every map key is `<hostKey>|<id>` (`scopedKey`), because
// terminal ids are built from project ids and `k2 migrate` carries ids
// between boxes, so two servers can hand out the same id. Writers pass
// their scope; readers use `pinOf` / `dimsOf` / `sessionsOf`.

export interface PinnedSize {
  cols: number
  rows: number
  /** "owner" or the pinning connect-user's username. `null` when the
   *  setter is unknown (a mid-session `pin_changed` frame carries no
   *  identity) — UI omits the "by <who>" clause. */
  setBy: string | null
}

interface PinnedSizeState {
  /** daemon sessionId → live pin. Absent = unpinned. */
  pins: Record<string, PinnedSize>
  /** renderer terminalId → daemon sessionId, registered by
   *  TerminalPane at spawn-resolve, dropped on teardown. */
  sessions: Record<string, string>
  /** daemon sessionId → the grid dims this window last measured for
   *  itself (the ResizeObserver fit). */
  dims: Record<string, { cols: number; rows: number }>

  /** Mirror a `pin_initial`/`pin_changed` frame (or a successful
   *  pin-size POST response). `null` clears. Idempotent. */
  setPin: (scope: ServerScope, sessionId: string, pin: PinnedSize | null) => void
  registerSession: (scope: ServerScope, terminalId: string, sessionId: string) => void
  unregisterSession: (scope: ServerScope, terminalId: string) => void
  setDims: (scope: ServerScope, sessionId: string, cols: number, rows: number) => void
}

export const usePinnedSizeStore = create<PinnedSizeState>((set) => ({
  pins: {},
  sessions: {},
  dims: {},

  setPin: (scope, rawSessionId, pin) =>
    set((state) => {
      const sessionId = scopedKey(scope, rawSessionId)
      const current = state.pins[sessionId]
      if (pin === null) {
        if (current === undefined) return state
        const { [sessionId]: _drop, ...rest } = state.pins
        return { pins: rest }
      }
      if (
        current &&
        current.cols === pin.cols &&
        current.rows === pin.rows &&
        current.setBy === pin.setBy
      ) {
        return state
      }
      return { pins: { ...state.pins, [sessionId]: pin } }
    }),

  registerSession: (scope, rawTerminalId, sessionId) =>
    set((state) => {
      const terminalId = scopedKey(scope, rawTerminalId)
      if (state.sessions[terminalId] === sessionId) return state
      return { sessions: { ...state.sessions, [terminalId]: sessionId } }
    }),

  unregisterSession: (scope, rawTerminalId) =>
    set((state) => {
      const terminalId = scopedKey(scope, rawTerminalId)
      if (!(terminalId in state.sessions)) return state
      const { [terminalId]: _drop, ...rest } = state.sessions
      return { sessions: rest }
    }),

  setDims: (scope, rawSessionId, cols, rows) =>
    set((state) => {
      const sessionId = scopedKey(scope, rawSessionId)
      const current = state.dims[sessionId]
      if (current && current.cols === cols && current.rows === rows) {
        return state
      }
      return { dims: { ...state.dims, [sessionId]: { cols, rows } } }
    }),
}))

/** The live pin for `sessionId` on `scope`'s server. */
export function pinOf(
  state: Pick<PinnedSizeState, 'pins'>,
  scope: ServerScope,
  sessionId: string,
): PinnedSize | undefined {
  return state.pins[scopedKey(scope, sessionId)]
}

/** This window's measured dims for `sessionId` on `scope`'s server. */
export function dimsOf(
  state: Pick<PinnedSizeState, 'dims'>,
  scope: ServerScope,
  sessionId: string,
): { cols: number; rows: number } | undefined {
  return state.dims[scopedKey(scope, sessionId)]
}

const sessionViews = new WeakMap<Record<string, string>, Map<string, Record<string, string>>>()

/** `terminalId → sessionId` for one server, with the host prefix removed.
 *  Memoized per map object, so a React selector over it stays stable. */
export function sessionsOf(
  sessions: Record<string, string>,
  scope: ServerScope,
): Record<string, string> {
  const prefix = scopedKey(scope, '')
  let byScope = sessionViews.get(sessions)
  if (!byScope) {
    byScope = new Map()
    sessionViews.set(sessions, byScope)
  }
  const cached = byScope.get(prefix)
  if (cached) return cached
  const out: Record<string, string> = {}
  for (const [k, v] of Object.entries(sessions)) {
    if (k.startsWith(prefix)) out[k.slice(prefix.length)] = v
  }
  byScope.set(prefix, out)
  return out
}
