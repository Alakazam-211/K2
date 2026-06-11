// Issue #8 — active-viewer claim predicate.
//
// The daemon's `set_active` handshake tells it WHICH connected client
// is the live viewer of a session so it can size the grid and route
// resize/focus events. Pre-#8 the renderer keyed the claim on WINDOW
// focus alone: every mounted pane (including hidden background-tab
// panes kept alive with `display:none`) re-claimed `active:true` on
// each window-focus event. A long-lived session accumulated many such
// panes, so a single focus event produced a burst of simultaneous
// claims for sessions the user wasn't even looking at — churning the
// daemon's small grid broadcast channel into `lagged`/snapshot-flush
// cycles (the "stall then recover" symptom).
//
// The fix: a pane is the active viewer only when it is the visible,
// pane-focused pane in a focused window. All three must hold.
//
// Kept as a standalone pure function (no React, no DOM) so the truth
// table is exhaustively unit-testable and the TerminalPane effect has
// a single source of truth it can call from refs.
export interface ActiveViewerInputs {
  /** This pane's enclosing tab + pane-item are visible (not display:none). */
  visible: boolean
  /** This pane holds shadow-input focus (only one pane does at a time). */
  paneFocused: boolean
  /** The OS window is focused. */
  windowFocused: boolean
}

/**
 * The desired `set_active` value for this pane: true iff it is the
 * visible, focused pane in a focused window.
 */
export function computeDesiredActive(inputs: ActiveViewerInputs): boolean {
  return inputs.visible && inputs.paneFocused && inputs.windowFocused
}

// Issue #8 (0.39.13) — grid-WS hold predicate.
//
// The "why" behind the active-viewer flood: even with the
// `computeDesiredActive` gate (which keeps a hidden pane from CLAIMING
// active), every mounted `TerminalPane` still OPENED a grid-WS and
// stayed subscribed to the session's grid broadcast — including hidden
// (`display:none`) background tabs and off-screen heartbeat-spawn
// panes. A long-lived window therefore piled up N grid-WS subscribers
// on the daemon's small per-session broadcast channel, and any burst of
// output overran it into `lagged`/snapshot-flush cycles.
//
// The PTY lives in the daemon and survives independently; the renderer
// only needs to STREAM a session's live grid while the user is actually
// looking at that pane. So: a pane holds a grid-WS ONLY while visible.
// Spawning the PTY (the idempotent HTTP POST) stays decoupled from
// streaming it (the grid-WS) — a hidden pane keeps its daemon PTY warm
// without subscribing to its grid.
//
// We also stop holding (and stop reconnecting) once the child has
// exited — there's nothing left to stream, and the existing reconnect
// path already skips a real `child_exit`. Folding that into the same
// pure predicate keeps the lifecycle effect's decision in one
// exhaustively-testable place.
export interface GridWsInputs {
  /** This pane's enclosing tab + pane-item are visible (not display:none). */
  visible: boolean
  /** The session's child process has exited — nothing left to stream. */
  exited: boolean
}

/**
 * Whether this pane should currently hold an open grid-WS streaming the
 * session's live grid. True iff the pane is visible AND its child hasn't
 * exited. A hidden pane (background tab, off-screen heartbeat spawn)
 * holds no grid-WS — its daemon PTY survives untouched.
 */
export function shouldHoldGridWs(inputs: GridWsInputs): boolean {
  return inputs.visible && !inputs.exited
}

// 0.39.43 (PRD `daemon-multi-client-arbitration.md` Issue A) —
// cross-remount active-claim dedup.
//
// The per-component `lastSentActiveRef` in TerminalPane resets on every
// mount. So a BARE re-mount (e.g. AgentChatPane bumps `attachNonce`,
// remounting TerminalPane under a new React key) re-runs the initial
// claim and re-sends `set_active:true` even though the user's focus
// never changed — letting the local window re-steal the daemon's
// active-subscriber slot from a remote viewer (the "local wins on
// refresh" path). The PRD fix: persist the last-sent active value
// keyed by the canonical session id so it SURVIVES re-mounts.
//
// On a fresh TerminalPane instance's FIRST claim attempt, we consult
// this map: if the value we're about to send equals what the last
// instance already sent for this session, the re-mount changed nothing
// on the wire — skip the send (no re-steal). A genuine focus transition
// computes a DIFFERENT desired value → not deduped → claims/releases
// correctly. A genuine WS reconnect within the SAME instance bypasses
// this (it must re-prime the new daemon subscriber) — that path is
// gated separately in TerminalPane by a per-instance "first connect"
// flag, not by this map.
//
// Keyed by the daemon session id (the PTY identity): an idempotent
// re-attach across a bare re-mount returns the SAME session id, so the
// dedup window is shared; a real session switch gets a new id and a
// fresh (undefined) entry, so it always claims.
const lastSentActiveBySession = new Map<string, boolean>()

/**
 * Record the active value just sent on the wire for `sessionId`, so a
 * later re-mount can detect "nothing changed" and skip a redundant
 * re-claim. Call this every time a `set_active` frame is actually sent.
 */
export function recordSentActive(sessionId: string, active: boolean): void {
  lastSentActiveBySession.set(sessionId, active)
}

/**
 * The last active value sent for `sessionId` across ALL TerminalPane
 * instances (survives re-mounts), or `undefined` if none was ever sent
 * (brand-new / freshly-switched session). Used by a fresh instance's
 * initial claim to decide whether a re-mount actually changed anything.
 */
export function getLastSentActive(sessionId: string): boolean | undefined {
  return lastSentActiveBySession.get(sessionId)
}

/**
 * Whether a fresh TerminalPane instance's INITIAL claim for `sessionId`
 * should be suppressed: true iff the desired value equals what the
 * previous instance already sent for this session (a bare re-mount with
 * unchanged focus → re-sending would needlessly re-steal the daemon's
 * active slot). Returns false when the value differs (genuine focus
 * transition) or when nothing was ever sent (new session must claim).
 */
export function shouldSkipRemountReclaim(
  sessionId: string,
  desired: boolean,
): boolean {
  return lastSentActiveBySession.get(sessionId) === desired
}

/** Test-only: clear the cross-remount dedup map between cases. */
export function __resetActiveViewerDedupForTests(): void {
  lastSentActiveBySession.clear()
}
