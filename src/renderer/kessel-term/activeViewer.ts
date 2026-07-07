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
  /** Pinned-canonical-chat exemption (pinned-chat background retention):
   *  hold the grid-WS even while hidden, so switching back to this pane
   *  repaints from live in-memory state instead of re-paying the
   *  spawn/WS/snapshot chain. Only the pinned canonical Chat pane of an
   *  ACTIVE workspace ever sets this (AgentChatPane, daemon-owned path);
   *  every other consumer omits it — byte-identical to the pre-retention
   *  predicate. `exited` still wins: a dead child is never streamed,
   *  retained or not. */
  retainWhileHidden?: boolean
}

/**
 * Whether this pane should currently hold an open grid-WS streaming the
 * session's live grid. True iff the pane is visible (or exempted via
 * `retainWhileHidden`) AND its child hasn't exited. A hidden,
 * non-retained pane (background tab, off-screen heartbeat spawn) holds
 * no grid-WS — its daemon PTY survives untouched.
 */
export function shouldHoldGridWs(inputs: GridWsInputs): boolean {
  return (inputs.visible || inputs.retainWhileHidden === true) && !inputs.exited
}

// Pinned-chat retention follow-up — resize emission predicate.
//
// `sendResize` used to gate on WINDOW focus alone. A background-retained
// pane (portal container parked in PinnedChatRetainer's hidden host)
// keeps its grid-WS AND its ResizeObserver, so a container move that
// changed its box dispatched a resize with the HIDDEN host's geometry —
// and because the pane had just released its claim (`set_active:false`
// ⇒ `active_subscriber == 0`), the daemon's first-resize-wins rule for
// unclaimed sessions ACCEPTED it. The PTY genuinely reflowed to
// off-screen dimensions on switch-away and back on switch-in: the
// "zoom on workspace switch" bug.
//
// The rule: only a pane the user can actually SEE may emit resize. A
// hidden pane still records its measured dims locally (they ride its
// next `set_active` claim, which snaps the PTY on foreground) but
// sends NOTHING on the wire.
export interface ResizeEmitInputs {
  /** This pane's enclosing tab + pane-item are visible (not parked in
   *  a hidden host / background tab). */
  visible: boolean
  /** The OS window is focused. */
  windowFocused: boolean
}

/**
 * Whether this pane may emit a `resize` frame right now: true iff it
 * is the visible pane in a focused window. Deliberately does NOT
 * require the active claim — a visible-but-unclaimed pane's first
 * resize is how a fresh session gets sized (daemon first-resize-wins).
 */
export function shouldEmitResize(inputs: ResizeEmitInputs): boolean {
  return inputs.visible && inputs.windowFocused
}

// Multiplayer resurface-steal fix — deliberate-interaction claim gate.
//
// `computeDesiredActive` answers "is this pane the one the user is
// looking at?" — a NECESSARY condition for claiming, but not a
// sufficient one. Keying the claim on it alone conflates SELECTION
// with CONTROL: every ambient recompute (OS window focus regain, tab
// becoming visible, programmatic shadow-input refocus, plain mount)
// re-sent `{set_active:true}`, and the daemon is most-recent-claim-
// wins — so merely resurfacing the K2 app RECLAIMED the session from
// whoever took it over while you were away.
//
// The rule: a CLAIM may only ride a DELIBERATE interaction —
// pointerdown/typing on the pane, flipping the window mode to
// claimer, or a WS-reconnect restore of a claim THIS client already
// held (a network blip must not demote an active controller).
// RELEASES (`set_active:false`) stay ambient: losing focus/visibility
// must always free the slot promptly.
//
// The interaction window exists because the deliberate act and the
// recompute that actually fires are decoupled: pointerdown stamps the
// timestamp, then focus → React state → effect-recompute runs a tick
// or three later as an "ambient" recompute. Anything inside the
// window is attributed to that interaction; anything outside (an
// app-resurface minutes later) is not.
export const CLAIM_INTERACTION_WINDOW_MS = 600

/** What triggered this recompute of the active claim. */
export type ClaimReason = 'ambient' | 'interaction' | 'restore'

export interface ClaimSendInputs {
  /** The computed desired active value (`computeDesiredActive` && mode). */
  desired: boolean
  /** What triggered the recompute. */
  reason: ClaimReason
  /** Timestamp (same clock as `now`) of the last deliberate
   *  interaction with this pane; `-Infinity` if none ever. */
  lastInteractionAt: number
  /** Current timestamp (`performance.now()` in production). */
  now: number
  /** 'restore' only: this client's own recorded last-sent active for
   *  the session was `true` (it held the claim before the reconnect). */
  restorable?: boolean
}

/**
 * Whether a computed `set_active` value may be SENT right now.
 * Releases (`desired:false`) are always sendable. Claims
 * (`desired:true`) are sendable only when attributable to a
 * deliberate interaction ('interaction' directly, 'ambient' within
 * `CLAIM_INTERACTION_WINDOW_MS` of one) or to a reconnect restore of
 * a claim this client already held (`restorable`).
 */
export function shouldSendClaim(inputs: ClaimSendInputs): boolean {
  if (!inputs.desired) return true
  switch (inputs.reason) {
    case 'interaction':
      return true
    case 'restore':
      return inputs.restorable === true
    case 'ambient':
      return inputs.now - inputs.lastInteractionAt <= CLAIM_INTERACTION_WINDOW_MS
  }
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
