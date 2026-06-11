// Alacritty_v2 Tauri thin client.
//
// Speaks the A3/A4 protocol defined in
// `.k2so/prds/alacritty-v2.md`:
//
//   1. POST /cli/sessions/v2/spawn with {agent_name, cwd, ...}
//      → {sessionId, agentName, cols, rows, reused}.
//   2. Open WS to /cli/sessions/grid?session=<uuid>&token=<token>.
//   3. Receive {event:"snapshot", payload:TermGridSnapshot} first,
//      then stream of {event:"delta", payload:TermGridDelta}.
//   4. On keystroke / paste: send {action:"input", text}.
//   5. On ResizeObserver: send {action:"resize", cols, rows}.
//   6. On unmount: close WS socket only. Session survives on
//      daemon — v2's whole point. Explicit close happens via
//      /cli/sessions/v2/close from tabs.ts removeTab (A6).
//
// No local alacritty_terminal::Term. No ANSI parser. No byte
// stream. The daemon does all of that; we render JSON-serialized
// grid deltas to DOM using the CellRun vocabulary from
// k2so-core's grid_snapshot module.
//
// Deliberately kept small (< 450 lines). The Kessel-era
// SessionStreamViewTerm was ~600 lines because it held a local
// Term + byte reader + APC filter. None of that here.

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

import { useKesselConfig } from '../kessel/config-context'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
import {
  computeDesiredActive,
  getLastSentActive,
  recordSentActive,
  shouldHoldGridWs,
} from './activeViewer'
import {
  keyEventToSequence,
  naturalTextEditingSequence,
} from '@/lib/key-mapping'
import { getDaemonWs, invalidateDaemonWs, daemonHttpBase, daemonWsBase, type DaemonWsAvailable } from '../kessel/daemon-ws'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useTabsStore } from '@/stores/tabs'
import { useWindowFocusStore } from '@/stores/window-focus'
import { useSessionLabelsStore } from '@/stores/session-labels'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { detectWorkingSignal } from '@/lib/agent-signals'
import {
  detectLinks,
  type DetectedLink,
} from '@/components/Terminal/terminalLinkDetector'
import {
  bracketPaste,
  isImagePath,
  quotePathForImageDrop,
} from '@/lib/file-drag'
import { useConnectHostStore } from '@/stores/connect-host'
import { executeRemoteDrop } from '@/lib/handle-remote-drop'

// ── Wire types (mirror k2so-core/src/terminal/grid_snapshot.rs) ───

interface CellRun {
  text: string
  fg: number | null
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
  inverse: boolean
  dim: boolean
  strikeout: boolean
}

interface CursorSnapshot {
  row: number
  col: number
  visible: boolean
}

interface TermGridSnapshot {
  paneId: string
  cols: number
  rows: number
  grid: CellRun[][]
  scrollback: CellRun[][]
  cursor: CursorSnapshot
  version: number
  displayOffset: number
}

interface DamagedRow {
  row: number
  runs: CellRun[]
}

interface TermGridDelta {
  paneId: string
  cols: number
  rows: number
  damagedRows: DamagedRow[]
  scrollbackAppended: CellRun[][]
  cursor: CursorSnapshot
  version: number
  displayOffset: number
}

type OutboundMsg =
  | { event: 'snapshot'; payload: TermGridSnapshot }
  | { event: 'delta'; payload: TermGridDelta }
  | { event: 'child_exit'; payload: { exit_code: number | null } }
  | { event: 'title'; payload: { title: string } }
  | { event: 'bell'; payload: null }
  | { event: 'error'; payload: { message: string } }
  // 0.37.4 Phase B — daemon-owned label events.
  | { event: 'label_initial'; payload: { label: string } }
  | { event: 'label_changed'; payload: { label: string } }

// ── Helpers ───────────────────────────────────────────────────────

function hexToCss(n: number): string {
  const r = (n >> 16) & 0xff
  const g = (n >> 8) & 0xff
  const b = n & 0xff
  return `rgb(${r},${g},${b})`
}

function runStyle(
  run: CellRun,
  defaultFg: string,
  defaultBg: string,
): React.CSSProperties {
  // Resolve fg/bg, falling back to terminal defaults so the
  // INVERSE flag actually produces a swap when a cell has only
  // the flag set (no explicit colors). TUIs that paint their own
  // visual cursor by inverting a default-colored cell — Cursor
  // Agent's "P" highlight, vim's normal-mode cursor, etc — rely
  // on this behavior. Without resolving defaults, an inverse
  // cell with null fg/null bg was rendering as plain text and
  // the TUI's cursor block was invisible.
  const fg = run.fg !== null ? hexToCss(run.fg) : defaultFg
  const bg = run.bg !== null ? hexToCss(run.bg) : defaultBg
  const color = run.inverse ? bg : fg
  const backgroundColor = run.inverse ? fg : bg
  const style: React.CSSProperties = {}
  // Only emit color/background when (a) inverse is on (so the
  // span actually has a visible block) or (b) the cell explicitly
  // set a non-default value. Always emitting `color: defaultFg`
  // would unnecessarily bloat the DOM and break inheritance for
  // cells that meant to use the parent's default.
  if (run.inverse) {
    style.color = color
    style.backgroundColor = backgroundColor
  } else {
    if (run.fg !== null) style.color = color
    if (run.bg !== null) style.backgroundColor = backgroundColor
  }
  if (run.bold) style.fontWeight = 'bold'
  if (run.italic) style.fontStyle = 'italic'
  if (run.underline && run.strikeout) {
    style.textDecoration = 'underline line-through'
  } else if (run.underline) {
    style.textDecoration = 'underline'
  } else if (run.strikeout) {
    style.textDecoration = 'line-through'
  }
  if (run.dim) style.opacity = 0.6
  return style
}

function renderRowRuns(
  row: CellRun[],
  absRow: number,
  defaultFg: string,
  defaultBg: string,
): React.ReactNode {
  if (row.length === 0) return '\u00a0'
  const spans: React.ReactNode[] = []
  for (let i = 0; i < row.length; i++) {
    const run = row[i]
    spans.push(
      <span key={`a${absRow}s${i}`} style={runStyle(run, defaultFg, defaultBg)}>
        {run.text || '\u00a0'}
      </span>,
    )
  }
  return spans
}

/** Join all run text in a row into a single plain string. Used
 *  for link detection (which operates on raw text). */
function rowToText(row: CellRun[]): string {
  let out = ''
  for (const run of row) out += run.text
  return out
}

/** Shell-escape a path for safe paste into a terminal input line.
 *  Mirrors the helper in AlacrittyTerminalView.tsx — duplicated
 *  rather than imported to keep v2 decoupled from v1. */
function shellEscape(path: string): string {
  return path.replace(/[ '"\\()&|;<>$`!#*?[\]{}~]/g, '\\$&')
}

/** Images/PDFs skip backslash-escape so Claude Code's
 *  `[Image #N]` detection (which fs.exists()s the literal string)
 *  can resolve them. */
function formatPathForTerminal(path: string): string {
  return isImagePath(path) ? quotePathForImageDrop(path) : shellEscape(path)
}

/** Build terminal payload for a dropped/pasted set of paths.
 *  Wraps in bracketed paste if any path is an image, so Claude's
 *  paste-event handler fires. */
function buildDropPayload(paths: string[]): string {
  const formatted = paths.map(formatPathForTerminal).join(' ')
  const trailing = formatted + ' '
  return paths.some(isImagePath) ? bracketPaste(trailing) : trailing
}

/** Whether a snapshot's visible grid contains any non-blank cell.
 *  Used by the [v2-perf] instrumentation to detect when the child
 *  process actually paints something (e.g. shell prompt). Empty
 *  initial snapshots are expected on cold spawn — the daemon's Term
 *  has no content until the child writes its first bytes. */
function isGridEmpty(snap: TermGridSnapshot): boolean {
  for (const row of snap.grid) {
    for (const run of row) {
      if (run.text && run.text.trim().length > 0) return false
    }
  }
  return true
}

/** Merge a delta into a prior snapshot. Pure. Returns `prev`
 *  unchanged if no prior snapshot exists yet (delta arrived
 *  before the initial snapshot — shouldn't happen per protocol,
 *  but guard anyway). */
function mergeDelta(
  prev: TermGridSnapshot | null,
  delta: TermGridDelta,
): TermGridSnapshot | null {
  if (!prev) return prev
  const nextGrid: CellRun[][] = prev.grid.slice()
  while (nextGrid.length < delta.rows) nextGrid.push([])
  if (nextGrid.length > delta.rows) nextGrid.length = delta.rows
  for (const dr of delta.damagedRows) {
    if (dr.row < 0 || dr.row >= delta.rows) continue
    nextGrid[dr.row] = dr.runs
  }
  const nextScrollback =
    delta.scrollbackAppended.length > 0
      ? prev.scrollback.concat(delta.scrollbackAppended)
      : prev.scrollback
  return {
    paneId: prev.paneId,
    cols: delta.cols,
    rows: delta.rows,
    grid: nextGrid,
    scrollback: nextScrollback,
    cursor: delta.cursor,
    version: delta.version,
    displayOffset: delta.displayOffset,
  }
}

// ── Component ─────────────────────────────────────────────────────

export interface TerminalPaneProps {
  terminalId: string
  /** Parent tab id — used to route file-link clicks to the right
   *  sibling pane when the user's "open links in split pane"
   *  preference is on. */
  tabId?: string
  /** This pane's pane-group id, for the same split-pane routing. */
  paneGroupId?: string
  cwd: string
  command?: string
  args?: string[]
  fontSize?: number
  spawnedAt?: number
  /** Override the auto-derived `tab-${terminalId}` agent_name used by
   *  /cli/sessions/v2/spawn. Set when this tab is meant to attach to
   *  an *existing* daemon-side session whose key in `v2_session_map`
   *  is something other than `tab-...` — e.g. heartbeat-spawned
   *  sessions live under the workspace's primary agent name. Without
   *  this, /cli/sessions/v2/spawn never finds the existing session
   *  and silently spawns a fresh resume. See
   *  `.k2so/prds/heartbeat-active-session-tracking.md`. */
  attachAgentName?: string
  /** 0.37.4 Phase B — initial label seed sent to the daemon at
   *  spawn time. Used by callers that already know what this
   *  session should be called (e.g. a chat-history-restored tab
   *  knows the session name; a heartbeat fire knows the schedule
   *  name). The daemon stores this as the authoritative label and
   *  emits `LabelInitial` to all subscribers. Empty / unset ⇒ no
   *  seed; PTY title events fill the label. */
  seedLabel?: string
  /** 0.37.4 Phase B — when true, lock the daemon-owned label so
   *  PTY title events can't overwrite it (e.g. claude --resume
   *  emitting "Claude Code"). Pairs with `seedLabel` for the
   *  common case "I know the right label, don't let the PTY
   *  smudge it." */
  lockLabel?: boolean
  /** K2 #682 — fired when the daemon reports the child process
   *  exited (`child_exit`). Carries the exit code so the consumer can
   *  distinguish a clean quit from a crash, and is the signal a
   *  spawn-loop circuit breaker counts. The pinned Chat tab
   *  (`AgentChatPane`) uses this to detect RAPID REPEATED early exits
   *  (e.g. `claude --session-id <dup>` → "already in use" → exit 1) and
   *  STOP auto-respawning instead of piling up `claude` processes.
   *  Optional — most consumers don't need it. */
  onChildExit?: (exitCode: number | null) => void
}

type Phase =
  | { kind: 'idle' }
  | { kind: 'spawning' }
  | { kind: 'connecting'; sessionId: string }
  | { kind: 'ready'; sessionId: string }
  // Issue #8 (0.39.13): PTY is spawned/attached on the daemon, but this
  // pane is hidden so it holds NO grid-WS. The session is warm; we just
  // aren't streaming its grid. Transitions back to 'connecting' when the
  // pane becomes visible (the grid-WS effect opens the WS) — WITHOUT
  // re-running the spawn POST (spawn lifecycle is a separate effect now).
  | { kind: 'parked'; sessionId: string }
  | { kind: 'exited'; sessionId: string; exitCode: number | null }
  | { kind: 'error'; message: string }

// 0.37.9 — Fallback shadow textarea style for the brief window
// before snapshot/cellMetrics are available. Off-screen-far-left
// matches xterm.js's default helper-textarea CSS — focusable but
// not visible, no flash on first render. Once snapshot lands, the
// component computes a cursor-positioned style instead (via the
// `shadowInputStyle` memo inside the component).
const SHADOW_INPUT_FALLBACK_STYLE: React.CSSProperties = {
  position: 'absolute',
  left: '-9999em',
  top: 0,
  width: 0,
  height: 0,
  opacity: 0,
  zIndex: -5,
  border: 0,
  outline: 'none',
  padding: 0,
  margin: 0,
  resize: 'none',
  whiteSpace: 'nowrap',
  overflow: 'hidden',
}

export function TerminalPane(props: TerminalPaneProps): React.JSX.Element {
  const config = useKesselConfig()
  const {
    terminalId,
    tabId,
    paneGroupId,
    cwd,
    command,
    args,
    spawnedAt,
    attachAgentName,
    seedLabel,
    lockLabel,
  } = props

  // Live-subscribe to the terminal settings store so Cmd+Shift+=
  // / Cmd+Shift+- menu events (wired via listen('terminal:zoom-*')
  // in terminal-settings.ts) update this component's font size
  // immediately. Prop takes precedence for tests / ad-hoc consumers
  // that want to override.
  const storeFontSize = useTerminalSettingsStore((s) => s.fontSize)
  const fontSize = props.fontSize ?? storeFontSize
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)

  const [phase, setPhase] = useState<Phase>({ kind: 'idle' })
  // Issue #5: mid-flight WS drops (TCP reset, WebKit Networking
  // throttling, brief process pressure) used to leave the terminal
  // silently frozen on its last frame — `ws.onclose` was a no-op so
  // no reconnect path existed. `reconnectAttempt` is bumped from
  // `onclose` after a backoff timer; it's in the boot effect's dep
  // array, so the effect tears down + re-runs (fresh spawn — daemon's
  // /cli/sessions/v2/spawn is idempotent on agent_name, returns the
  // same sessionId — and fresh WS handshake). Reset to 0 when the
  // pane really unmounts.
  const [reconnectAttempt, setReconnectAttempt] = useState(0)
  const reconnectTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  // 0.39.13 — spawn ⊥ stream decoupling.
  //
  // v1 (the regression we're fixing) put `isTabVisible` in the boot
  // effect's dep array, coupling the grid-WS lifecycle to the spawn
  // effect: any poll-driven re-render that re-ran the boot effect (or
  // any transient visibility re-eval) tore down + reopened the grid-WS
  // — the visible pane's WS detached/attached every ~3s in lockstep
  // with the active-agents poll, with a redundant spawn POST each time.
  //
  // The fix splits the lifecycle into two effects:
  //   - SPAWN effect (deps: terminalId, cwd, command, args, reconnectAttempt
  //     — all STABLE; NO isTabVisible): runs the idempotent spawn POST,
  //     stashes the sessionId in `sessionIdRef`, and bumps `spawnGeneration`
  //     to announce "a fresh PTY is ready to be streamed". It never opens
  //     a grid-WS and never tears down on a visibility flip.
  //   - GRID-WS effect (deps: spawnGeneration, isTabVisible): the ONLY
  //     thing that opens/closes the grid-WS. A real visible↔hidden flip
  //     (or a fresh spawn generation) reconciles the WS; a spurious
  //     re-render with unchanged visibility is a guarded no-op
  //     (`appliedVisibleRef`), so it never drops/reopens on the poll.
  const sessionIdRef = useRef<string | null>(null)
  const [spawnGeneration, setSpawnGeneration] = useState(0)
  // Last visible state the grid-WS effect actually ACTED on. Lets the
  // effect distinguish a true visible↔hidden transition (open/close the
  // WS) from a re-run where visibility didn't change (no-op). `null`
  // until the first reconcile.
  const appliedVisibleRef = useRef<boolean | null>(null)
  // Stable handle to "open a grid-WS for the current sessionId". Lives
  // in a ref so the grid-WS effect (whose deps are spawnGeneration +
  // isTabVisible) can call the latest implementation without taking the
  // big WS-open closure as a dependency. Assigned once below.
  const openGridWsRef = useRef<() => Promise<void>>(async () => {})
  // K2 #682 — latest `onChildExit` callback in a ref so the big WS
  // closure (deps: terminalId, perfLog, reconnectAttempt) fires the
  // CURRENT consumer without re-subscribing the socket on every render.
  const onChildExitRef = useRef<((exitCode: number | null) => void) | undefined>(
    props.onChildExit,
  )
  useEffect(() => {
    onChildExitRef.current = props.onChildExit
  }, [props.onChildExit])
  // 0.39.9: phase, but as a ref. `ws.onclose` (bound inside the boot
  // effect) needs to consult the LATEST phase to know whether to
  // skip reconnect after a real `child_exit` — but the closure
  // captured `phase` from when `boot()` ran, so a phase change in
  // `onmessage` (e.g. setPhase({kind:'exited',...})) wouldn't be
  // visible inside `onclose`. Pre-0.39.9 that meant a child exit
  // followed by the daemon dropping the WS would resurrect the
  // terminal as a fresh session. We mirror `phase` into a ref via
  // a tiny useEffect below, then `onclose` reads `phaseRef.current`.
  const phaseRef = useRef<Phase>({ kind: 'idle' })
  // 0.39.9: keep `phaseRef` in lockstep with `phase` so the boot
  // effect's `ws.onclose` closure can always read the latest phase
  // when deciding whether to skip reconnect after `child_exit`.
  // Without this sync, the closure would see whatever `phase` value
  // was current when the boot effect last ran — which is stale by
  // the time `onclose` fires after a `child_exit` message updates
  // phase via the renderer's own `setPhase` call.
  useEffect(() => {
    phaseRef.current = phase
  }, [phase])
  const [snapshot, setSnapshot] = useState<TermGridSnapshot | null>(null)
  const [viewportOffset, setViewportOffset] = useState(0)
  const [isFocused, setIsFocused] = useState<boolean>(() =>
    typeof document !== 'undefined' ? document.hasFocus() : false,
  )

  const containerRef = useRef<HTMLDivElement>(null)
  // 0.37.9 — invisible focusable <textarea> sibling to the visible
  // grid. macOS Apple Dictation only fires when the focused element
  // is one AppKit recognizes as a text input (NSTextField / NSTextView
  // or a WebView <textarea>/<input>/contenteditable). The container
  // <div tabIndex={0}> isn't one of those, so Fn-Fn silently does
  // nothing. A real <textarea> overlaid invisibly on the pane gets
  // dictation working with no visible UI change. See PRD:
  // .k2so/prds/voice-dictation.md.
  const shadowInputRef = useRef<HTMLTextAreaElement>(null)
  // Tracks IME / dictation composition (Japanese/Chinese/Korean
  // candidate window, accent picker, Apple Dictation). While
  // composing, onKey + onShadowInput skip — onComposeUpdate
  // streams partials to the PTY using backspace+retype so words
  // flow into the prompt as the user speaks.
  const composingRef = useRef(false)
  // 0.37.11 — true while a mouse button is held down anywhere
  // inside this pane (the user might be in the middle of a
  // drag-select). The container's onFocus handler skips its
  // shadow-textarea delegation while this is true so we don't
  // shift focus mid-drag and cancel the in-flight selection.
  // Cleared on mouseup AND on any global mouseup (covers the
  // case where the user releases outside the pane bounds).
  const mouseDownInPaneRef = useRef(false)
  // Length (in graphemes) of the partial transcript we last
  // streamed to the PTY. Each compositionupdate replaces the prior
  // partial in the PTY with the new best-guess: we send `\x7f`
  // (DEL) backspaces equal to this length, then the new text. On
  // compositionend we reconcile to the final committed string in
  // the same way (so Dictation's autocorrect-on-stop gets applied).
  const compositionLastLengthRef = useRef(0)
  const wsRef = useRef<WebSocket | null>(null)
  const isTabVisible = useIsTabVisible()

  // Issue #8 — mirror the two render-derived inputs to the
  // active-viewer predicate (`isFocused` pane-focus state, `isTabVisible`
  // context) into refs so the window-focus store subscriber and the
  // WS-connect initial-claim path can read the latest values WITHOUT
  // re-subscribing on every change. Same pattern as `phaseRef` above:
  // the subscriber lives in a `[]`-deps effect (so it can't thrash —
  // see Issue #3) but must still see fresh pane-focus / visibility.
  // A small recompute effect (declared with the set_active effect
  // below) reacts to `isFocused` / `isTabVisible` changes and routes
  // through the same dedup-guarded `sendSetActive`.
  const paneFocusedRef = useRef(false)
  const tabVisibleRef = useRef(false)
  useEffect(() => {
    paneFocusedRef.current = isFocused
  }, [isFocused])
  useEffect(() => {
    tabVisibleRef.current = isTabVisible
  }, [isTabVisible])

  // ── A7.5 perf instrumentation (DEV-only) ─────────────────────
  // mountT0 is captured once via lazy useRef init so re-renders
  // don't reset it. Stage timings accumulate into stageMsRef so
  // SUMMARY can break down totals at first_render / tui_first_paint.
  const mountT0Ref = useRef<number | null>(null)
  if (mountT0Ref.current === null) mountT0Ref.current = performance.now()
  const stageMsRef = useRef<Record<string, number>>({})
  const firstSnapshotEmptyRef = useRef<boolean>(true)
  const firstSnapshotSeenRef = useRef<boolean>(false)
  const firstSnapshotReusedRef = useRef<boolean | null>(null)
  const firstRenderFiredRef = useRef<boolean>(false)
  const tuiFirstPaintFiredRef = useRef<boolean>(false)

  const perfLog = useCallback(
    (stage: string, extra?: Record<string, unknown>) => {
      if (!import.meta.env.DEV) return
      const t = performance.now() - (mountT0Ref.current ?? performance.now())
      stageMsRef.current[stage] = t
      let line = `[v2-perf] t=${t.toFixed(0)}ms stage=${stage}`
      if (extra) {
        for (const [k, v] of Object.entries(extra)) {
          line += ` ${k}=${v}`
        }
      }
      // eslint-disable-next-line no-console
      console.info(line)
    },
    [],
  )

  // Link detection state. Set on hover over a URL / file path
  // that `detectLinks` recognizes in the row the mouse is over.
  // Non-null → cursor becomes pointer and click opens the link.
  const [hoveredLink, setHoveredLink] = useState<{
    row: number
    link: DetectedLink
  } | null>(null)
  const cmdHeldRef = useRef(false)
  const mouseDownLinkRef = useRef<DetectedLink | null>(null)
  const lastDetectPosRef = useRef({ x: 0, y: 0 })
  const lastDetectTimeRef = useRef(0)

  // ── Activity detection ────────────────────────────────────────
  // Mirrors AlacrittyTerminalView.tsx so v2 panes drive the same
  // sidebar braille spinner / "Active" indicators as legacy. Two
  // signals feed the active-agents store:
  //   1. recordOutput(terminalId) on every grid change — the
  //      heartbeat-style "this pane just produced bytes" signal.
  //   2. detectWorkingSignal(rows) viewport scan — the stable
  //      "is a CLI LLM mid-request" hint ("esc to interrupt",
  //      "thinking…", etc.). Gated on displayOffset === 0 so a
  //      scrolled-up user can't pin the pane in 'working' state.
  // Idle transition fires from a 500ms interval that watches a
  // 1s grace window since the last working signal.
  const lastSeenWorkingAtRef = useRef<number>(0)

  // Process one snapshot/delta payload for activity-store updates.
  // Bumps the per-pane heartbeat unconditionally and runs the
  // working-signal viewport scan when the user isn't scrolled.
  const lastDetectLogAtRef = useRef(0)
  const lastWorkingStateRef = useRef(false)
  const recordActivityFromSnapshot = useCallback(
    (snap: TermGridSnapshot) => {
      useActiveAgentsStore.getState().recordOutput(terminalId)

      // Build the row→{text} map detectWorkingSignal expects from
      // the WHOLE viewport. We deliberately do NOT gate on
      // `displayOffset === 0` because some renderers / rapid output
      // can leave the daemon-side display_offset non-zero even when
      // the user is effectively at the bottom — and the false-
      // positive cost (showing 'working' while scrolled-up) is much
      // smaller than the false-negative cost (no spinner ever).
      const lines = new Map<number, { text: string }>()
      for (let r = 0; r < snap.grid.length; r++) {
        lines.set(r, { text: rowToText(snap.grid[r]) })
      }
      const isWorking = detectWorkingSignal(lines, snap.rows)
      if (isWorking) {
        lastSeenWorkingAtRef.current = Date.now()
        useActiveAgentsStore.getState().recordTitleActivity(terminalId, true)
      }

      // DEV breadcrumbs.
      //
      // LOG-1: every working-state TRANSITION (idle→working,
      // working→idle), so we can see exactly when the spinner
      // should flip. Loud log level (warn) so it's easy to spot.
      //
      // LOG-2: throttled status — at most one info-level line per
      // second showing whether detection matched + a sample of the
      // bottom rows. Lets us see what text the scanner is actually
      // looking at when the user reports "no spinner."
      // FLIP fires once per working/idle transition — kept always-on in
      // dev because it's infrequent and load-bearing for "did the
      // spinner switch?" debugging.
      // The per-second snapshot sample below is now opt-in via
      // `localStorage.K2SO_V2_ACTIVITY_VERBOSE='1'`. It used to fire
      // unconditionally and was the loudest single source of dev
      // console noise (~1/sec per active agent).
      if (import.meta.env.DEV) {
        const wasWorking = lastWorkingStateRef.current
        if (isWorking !== wasWorking) {
          lastWorkingStateRef.current = isWorking
          // eslint-disable-next-line no-console
          console.warn(
            `[v2-activity] FLIP tid=${terminalId.slice(0, 8)} ${wasWorking ? 'working→idle' : 'idle→working'}`,
          )
        }
        if (typeof localStorage !== 'undefined' && localStorage.getItem('K2SO_V2_ACTIVITY_VERBOSE') === '1') {
          const now = Date.now()
          if (now - lastDetectLogAtRef.current > 1000) {
            lastDetectLogAtRef.current = now
            const tail = Math.max(0, snap.rows - 5)
            const sample: string[] = []
            for (let r = tail; r < snap.rows; r++) {
              const t = lines.get(r)?.text ?? ''
              if (t.trim()) sample.push(t.slice(0, 90))
            }
            // eslint-disable-next-line no-console
            console.info(
              `[v2-activity] tid=${terminalId.slice(0, 8)} working=${isWorking} ` +
                `displayOffset=${snap.displayOffset} rows=${snap.rows} ` +
                `gridRows=${snap.grid.length}\n  bottom=${JSON.stringify(sample, null, 2)}`,
            )
          }
        }
      }
    },
    [terminalId],
  )

  // Drive activity detection off snapshot-state changes so it
  // re-binds cleanly across Vite HMR / React Fast Refresh. (If
  // we called recordActivityFromSnapshot from inside the
  // ws.onmessage handler — captured in the boot effect's
  // closure — HMR'd activity code wouldn't take effect on
  // already-mounted sessions until the user closed and reopened
  // the tab.) React batches setSnapshot calls so this effect
  // runs once per coalesced grid update, not once per byte.
  const activityWiredLoggedRef = useRef(false)
  useEffect(() => {
    if (!activityWiredLoggedRef.current && import.meta.env.DEV) {
      activityWiredLoggedRef.current = true
      // eslint-disable-next-line no-console
      console.warn(`[v2-activity] WIRED tid=${terminalId.slice(0, 8)} — snapshot-driven detection is active`)
    }
    if (!snapshot) return
    recordActivityFromSnapshot(snapshot)
  }, [snapshot, recordActivityFromSnapshot, terminalId])

  // ── Working-state idle watcher ────────────────────────────────
  // Working → idle transitions when no signal has been seen for
  // 1 s. Same 500 ms cadence as legacy so the transition is at
  // most ~1.5 s after the real one but never flickers on a
  // single-frame status-line gap.
  useEffect(() => {
    const IDLE_GRACE_MS = 1000
    const interval = setInterval(() => {
      const last = lastSeenWorkingAtRef.current
      if (last === 0) return
      if (Date.now() - last > IDLE_GRACE_MS) {
        useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
        lastSeenWorkingAtRef.current = 0
      }
    }, 500)
    return () => clearInterval(interval)
  }, [terminalId])

  // ── Spawn effect (0.39.13: spawn ⊥ stream) ────────────────────
  //
  // Runs the idempotent HTTP POST to /cli/sessions/v2/spawn ONLY —
  // it no longer opens the grid-WS. On success it stashes the sessionId
  // in `sessionIdRef` and bumps `spawnGeneration`; the dedicated grid-WS
  // lifecycle effect below opens/closes the stream based on visibility.
  // Deps are STABLE (no `isTabVisible`), so a poll-driven re-render can
  // never tear this down or re-issue the spawn POST — the v1 churn this
  // fix removes. Any step failing parks the component in `{error}`.
  useEffect(() => {
    let cancelled = false
    // For heartbeat-surfaced tabs, attachAgentName carries the daemon's
    // existing v2_session_map key (e.g. the workspace's primary agent
    // name). Without the override the auto-derived `tab-${terminalId}`
    // never matches a daemon-spawned session → /cli/sessions/v2/spawn
    // creates a duplicate PTY instead of attaching. See PRD.
    const agentName = attachAgentName ?? `tab-${terminalId}`

    async function boot() {
      perfLog('mount', spawnedAt
        ? { since_keystroke_ms: Math.round(performance.now() - spawnedAt) }
        : undefined)
      setPhase({ kind: 'spawning' })

      const spawnBody = {
        agent_name: agentName,
        cwd,
        command: command ?? null,
        args: args ?? null,
        // Default cols/rows matter little — ResizeObserver corrects
        // via a /cli/sessions/v2/spawn-time value AND a follow-up
        // resize message once we measure the container.
        cols: 120,
        rows: 40,
        // 0.37.4 Phase B — pass label seed + lock policy through
        // to the daemon. Daemon stores these on the session and
        // emits LabelInitial/LabelChanged accordingly.
        label: seedLabel ?? null,
        label_locked: lockLabel ?? null,
      }

      // Boot with retry. `Tauri auto-update → relaunch` produces a
      // ~2–5 s window where the renderer is back up but the daemon
      // is mid-restart (version-mismatch handshake from 0.35.0 kicks
      // it). Without retry, every v2 pane that mounts in that window
      // surfaces "spawn fetch failed: TypeError: Load failed" until
      // the user manually closes + reopens it. Legacy panes are
      // immune because they spawn in-process via Tauri IPC and never
      // hit the daemon HTTP socket; this retry brings v2 to parity.
      //
      // Strategy: retry on network-level failures and 5xx for up to
      // ~10 s with exponential backoff (250 → 500 → 1000 → 2000 ms,
      // capped at 2000). 4xx surfaces immediately — it's a real
      // request error, not a transient unreachability.
      const BOOT_DEADLINE_MS = 10_000
      const __t_boot_start = performance.now()
      let creds: DaemonWsAvailable | null = null
      let spawn: {
        sessionId: string
        agentName: string
        cols: number
        rows: number
        reused: boolean
      } | null = null
      let attempt = 0
      while (true) {
        if (cancelled) return
        attempt += 1
        const __t_attempt = performance.now()
        try {
          if (!creds) {
            perfLog('creds_start', { attempt: String(attempt) })
            creds = await getDaemonWs()
            perfLog('creds_end', { elapsed_ms: (performance.now() - __t_attempt).toFixed(1) })
          }
          perfLog('spawn_fetch_start', { attempt: String(attempt) })
          const __t_spawn_fetch = performance.now()
          const spawnRes = await fetch(
            `${daemonHttpBase(creds)}/cli/sessions/v2/spawn?token=${creds.token}`,
            {
              method: 'POST',
              headers: { 'Content-Type': 'application/json' },
              body: JSON.stringify(spawnBody),
            },
          )
          if (spawnRes.status >= 500) {
            // Daemon answered but failed — likely mid-init right
            // after restart. Retryable.
            const body = await spawnRes.text().catch(() => '')
            invalidateDaemonWs()
            throw new Error(`spawn ${spawnRes.status}: ${body || 'no body'}`)
          }
          if (!spawnRes.ok) {
            // 4xx — genuine request error, surface immediately. Bad
            // body, missing field, etc. Won't get better by waiting.
            const body = await spawnRes.text()
            if (!cancelled) {
              setPhase({ kind: 'error', message: `spawn ${spawnRes.status}: ${body}` })
            }
            return
          }
          spawn = (await spawnRes.json()) as typeof spawn
          perfLog('spawn_fetch_end', {
            elapsed_ms: (performance.now() - __t_spawn_fetch).toFixed(1),
            reused: String(spawn!.reused),
            sid: spawn!.sessionId.slice(0, 8),
            attempt: String(attempt),
          })
          break
        } catch (e) {
          // Network errors (TypeError 'Load failed' from fetch when
          // socket is closed) and 5xx land here. Daemon-creds errors
          // also land here (Tauri command failed). All are retryable
          // until the deadline.
          invalidateDaemonWs()
          creds = null
          const elapsedTotalMs = performance.now() - __t_boot_start
          if (elapsedTotalMs > BOOT_DEADLINE_MS) {
            if (!cancelled) {
              setPhase({
                kind: 'error',
                message: `spawn failed after ${Math.round(elapsedTotalMs / 1000)}s: ${String(e)}`,
              })
            }
            return
          }
          // Exponential backoff capped at 2 s.
          const delayMs = Math.min(250 * 2 ** Math.min(attempt - 1, 3), 2000)
          perfLog('spawn_retry', {
            attempt: String(attempt),
            delay_ms: String(delayMs),
            elapsed_ms: Math.round(elapsedTotalMs).toString(),
            err: String(e).slice(0, 60),
          })
          await new Promise((r) => setTimeout(r, delayMs))
        }
      }

      if (!creds || !spawn) return // unreachable; satisfies TS
      firstSnapshotReusedRef.current = spawn.reused
      if (cancelled) return

      // 0.39.13: capture the session id into a typed local. `spawn` is
      // typed `never` here (the self-referential `as typeof spawn` cast
      // at the fetch site widens it — a pre-existing quirk noted in the
      // baseline tsc errors), so reading `spawn.sessionId` in MORE
      // places would add more of those `'never'` errors. Reading it
      // once through a typed local keeps this change at zero new tsc
      // errors AND reads cleaner.
      const sessionId: string = (spawn as { sessionId: string }).sessionId

      // 0.39.13 — spawn ⊥ stream. The PTY is now spawned/attached on the
      // daemon. We do NOT open the grid-WS here. Instead we stash the
      // sessionId and bump `spawnGeneration`; the dedicated grid-WS
      // effect (deps: spawnGeneration + isTabVisible) decides whether to
      // open a WS based purely on visibility — so a poll-driven re-render
      // can never tear this spawn down or churn the stream.
      sessionIdRef.current = sessionId
      // Phase reflects spawn outcome only; the grid-WS effect will move
      // us to 'connecting'/'ready' (visible) or leave us 'parked'
      // (hidden). Reading the live visibility ref keeps the initial
      // phase honest for the first paint.
      setPhase(
        tabVisibleRef.current
          ? { kind: 'connecting', sessionId }
          : { kind: 'parked', sessionId },
      )
      if (!tabVisibleRef.current) {
        perfLog('park_hidden', { sid: sessionId.slice(0, 8) })
      }
      // Announce a fresh PTY generation. This is what wakes the grid-WS
      // effect to (re)open the stream for a visible pane — including on
      // reconnect, where the daemon's idempotent spawn returns the SAME
      // sessionId (so a sessionId-keyed effect alone would NOT re-fire;
      // the monotonically-increasing generation guarantees it does).
      setSpawnGeneration((g) => g + 1)
    }

    void boot()

    return () => {
      cancelled = true
      // Cancel any pending reconnect timer when the spawn effect tears
      // down (real unmount OR a reconnect-driven re-run via
      // reconnectAttempt). Without this, a re-run would schedule a new
      // connect on top of a pending one and we'd race two handshakes.
      if (reconnectTimerRef.current !== null) {
        clearTimeout(reconnectTimerRef.current)
        reconnectTimerRef.current = null
      }
      // Forget the spawned session for this generation; the grid-WS
      // effect's cleanup (below) owns closing the socket.
      sessionIdRef.current = null
    }
    // 0.39.13 — STABLE deps only. `isTabVisible` is deliberately NOT
    // here: visibility no longer drives spawn. `reconnectAttempt` still
    // re-runs the spawn (Issue #5: a genuine mid-flight WS drop bumps
    // it; the daemon's idempotent spawn re-attaches the same PTY and the
    // bumped spawnGeneration re-opens the grid-WS). The big WS-open
    // closure now lives in `openGridWs` below, called by the grid-WS
    // effect — not inline here.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [terminalId, cwd, command, args?.join('\0'), reconnectAttempt])

  // ── Grid-WS open routine (0.39.13) ────────────────────────────
  // The whole WS handshake + handler wiring, extracted from the old
  // boot effect so it can be invoked by the visibility-driven grid-WS
  // effect rather than re-run as part of spawn. Reads the current
  // sessionId from `sessionIdRef` (set by the spawn effect). Idempotent
  // on an already-open socket: callers guard via `wsRef.current`.
  const openGridWs = useCallback(async (): Promise<void> => {
    const sessionId = sessionIdRef.current
    if (!sessionId) return
    // Don't open a second socket on top of a live/connecting one.
    const existing = wsRef.current
    if (
      existing &&
      (existing.readyState === WebSocket.OPEN ||
        existing.readyState === WebSocket.CONNECTING)
    ) {
      return
    }

    // Staleness guard re-checked at every await point: if the pane went
    // hidden, the session changed, or unmounted while we were mid-
    // handshake, abandon this connect attempt so we never open a socket
    // for a pane that no longer wants to stream.
    const isStale = () =>
      sessionIdRef.current !== sessionId ||
      appliedVisibleRef.current !== true

    let creds: DaemonWsAvailable | null = null
    try {
      creds = await getDaemonWs()
    } catch {
      // Creds unavailable (daemon mid-restart). The next visibility
      // reconcile / reconnect will retry. Leave phase as-is.
      return
    }
    if (isStale() || !creds) return

    setPhase({ kind: 'connecting', sessionId })

    perfLog('ws_opening')
    const __t_ws = performance.now()

      // 0.37.7: WS connect-with-retry. Smooths the install-relaunch
      // race where the renderer mounts before the daemon has finished
      // binding its WS port (or before its credentials file has
      // settled). Pre-fix the renderer surfaced "ws error" on the
      // user's tab and they had to right-click → reload to recover.
      // Now we retry up to a deadline with exponential backoff —
      // most real install-relaunch races resolve in 1-2 retries
      // (~250-750ms).
      //
      // We DON'T retry forever. If after the deadline the WS still
      // can't connect, surface the error so the user knows
      // something's actually wrong — but a transient races doesn't
      // bubble up.
      const WS_BOOT_DEADLINE_MS = 8_000
      const __t_ws_boot = performance.now()
      let ws: WebSocket | null = null
      let wsAttempt = 0
      while (true) {
        if (isStale()) return
        wsAttempt += 1
        const candidate = new WebSocket(
          `${daemonWsBase(creds)}/cli/sessions/grid?session=${sessionId}&token=${creds.token}`,
        )
        // Race: open vs. close-before-open. Browser fires both
        // `onerror` then `onclose` when a connection is rejected
        // immediately (port not bound, etc.). We bind temporary
        // listeners; the real ones get attached after the open
        // resolves successfully.
        const opened = await new Promise<boolean>((resolve) => {
          const cleanup = () => {
            candidate.onopen = null
            candidate.onerror = null
            candidate.onclose = null
          }
          candidate.onopen = () => { cleanup(); resolve(true) }
          candidate.onerror = () => { cleanup(); resolve(false) }
          candidate.onclose = () => { cleanup(); resolve(false) }
        })
        if (isStale()) {
          if (candidate.readyState !== WebSocket.CLOSED) candidate.close()
          return
        }
        if (opened) {
          ws = candidate
          perfLog('ws_open', {
            elapsed_ms: (performance.now() - __t_ws).toFixed(1),
            attempts: String(wsAttempt),
          })
          break
        }
        // Connect failed — back off and retry within the boot
        // deadline. Beyond the deadline, surface the error.
        const elapsedMs = performance.now() - __t_ws_boot
        if (elapsedMs > WS_BOOT_DEADLINE_MS) {
          perfLog('ws_giveup', {
            attempts: String(wsAttempt),
            elapsed_ms: Math.round(elapsedMs).toString(),
          })
          setPhase({
            kind: 'error',
            message: 'ws error (daemon unreachable after retries)',
          })
          return
        }
        const delayMs = Math.min(250 * 2 ** Math.min(wsAttempt - 1, 3), 2000)
        perfLog('ws_retry', {
          attempt: String(wsAttempt),
          delay_ms: String(delayMs),
          elapsed_ms: Math.round(elapsedMs).toString(),
        })
        await new Promise((r) => setTimeout(r, delayMs))
      }

      if (!ws) return // unreachable; satisfies TS
      wsRef.current = ws
      // Issue #5 (re-prime active-viewer handshake on each WS
      // (re)connect): the daemon-side subscriber that opens on the
      // new WS is fresh and has no notion that we were previously
      // "active". Reset the send-level dedup so the next computed
      // value is always (re-)sent on this new connection. Without
      // this, a reconnect would leave a focused window with
      // `lastSentActiveRef === true` → the next recompute would
      // short-circuit (value unchanged) → the fresh daemon
      // subscriber never learns we're the active viewer.
      lastSentActiveRef.current = null
      // Issue #8: re-prime using the FULL predicate (visible AND
      // pane-focused AND window-focused), not window-focus alone.
      // A backgrounded pane that reconnects (e.g. WebKit dropped the
      // throttled background-tab socket) must NOT re-claim active —
      // pre-#8 it re-primed on window focus and a long-lived window
      // accumulated many such hidden claimants, flooding the grid
      // broadcast. `recomputeAndSendActiveRef` reads the live
      // visibility/focus refs and routes through the dedup guard.
      try {
        recomputeAndSendActiveRef.current()
      } catch {
        // WS could be in a half-open state right after handshake.
        // The set_active effect's focus subscriber + recompute effect
        // will recover on the next focus/visibility change via the
        // dedup-guarded path.
      }
      // Note: ws.onopen is intentionally NOT set here — the connect
      // retry loop above handled the open path and logged perf.
      // Setting onopen on an already-open socket would never fire
      // anyway (browser dispatched the event during the retry race).

      ws.onmessage = (evt) => {
        if (typeof evt.data !== 'string') return
        let parsed: OutboundMsg
        try {
          parsed = JSON.parse(evt.data) as OutboundMsg
        } catch {
          return
        }
        switch (parsed.event) {
          case 'snapshot': {
            const isFirst = !firstSnapshotSeenRef.current
            if (isFirst) {
              firstSnapshotSeenRef.current = true
              const empty = isGridEmpty(parsed.payload)
              firstSnapshotEmptyRef.current = empty
              perfLog('first_snapshot', {
                rows: parsed.payload.rows,
                cols: parsed.payload.cols,
                empty: String(empty),
                scrollback: parsed.payload.scrollback.length,
              })
            }
            setSnapshot(parsed.payload)
            // Activity detection runs in a snapshot-driven useEffect
            // below, NOT inline here. ws.onmessage is captured in the
            // boot effect's closure and does not re-bind across Vite
            // HMR / React Fast Refresh — calling activity from here
            // means HMR'd code wouldn't take effect on existing
            // sessions. Driving it from setSnapshot's downstream
            // effect avoids that whole class of bug.
            setPhase({ kind: 'ready', sessionId })
            break
          }
          case 'delta':
            setSnapshot((prev) => mergeDelta(prev, parsed.payload))
            break
          case 'title': {
            // Mirror legacy's `terminal:title:<id>` handling. Claude
            // Code uses braille-spinner glyphs in the title prefix
            // while working and the ✱-family glyphs the moment it
            // goes idle, so the title is the fastest, most reliable
            // working/idle hint we have. See
            // AlacrittyTerminalView.tsx:510-518 for the legacy
            // version. We use the SAME regex so v2 and legacy agree.
            const raw = parsed.payload.title ?? ''
            const isIdleMarker = /^[*✱✲✳✴✵✶✷✸✹⚹⁎∗※]/.test(raw)
            const isWorkingMarker = /^[\u2800-\u28FF]/.test(raw)
            // Per-title-change log. Fires every ~1s for any active
            // agent. Opt-in via `localStorage.K2SO_V2_ACTIVITY_VERBOSE='1'`.
            if (
              import.meta.env.DEV &&
              typeof localStorage !== 'undefined' &&
              localStorage.getItem('K2SO_V2_ACTIVITY_VERBOSE') === '1'
            ) {
              // eslint-disable-next-line no-console
              console.warn(
                `[v2-activity] TITLE tid=${terminalId.slice(0, 8)} raw=${JSON.stringify(raw.slice(0, 60))} idleMarker=${isIdleMarker} workingMarker=${isWorkingMarker}`,
              )
            }
            if (isIdleMarker) {
              lastSeenWorkingAtRef.current = 0
              useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
            } else if (isWorkingMarker) {
              lastSeenWorkingAtRef.current = Date.now()
              useActiveAgentsStore.getState().recordTitleActivity(terminalId, true)
            }
            // Strip the leading marker chars + collapse whitespace
            // so the user-visible title doesn't have spinner noise
            // in it. Mirrors the legacy substitution.
            const cleanTitle = raw
              .replace(/^[\u2800-\u28FF*✱✲✳✴✵✶✷✸✹⚹⁎∗※·•●◦‣⏺]\s*/g, '')
              .trim()
            if (cleanTitle && tabId) {
              // 0.37.4 Phase B: do NOT push the cleaned PTY title
              // back to the tab — daemon owns labels now. The
              // cleanTitle calc stays so other code that reads it
              // (none currently) keeps working; we just stop
              // mutating tab.title from this side. The daemon's
              // `label_changed` event is the only thing that
              // updates the visible label.
              void cleanTitle
            }
            break
          }
          case 'label_initial':
          case 'label_changed': {
            // 0.37.4 Phase B — daemon-authoritative label.
            // Mirror into the session-labels store keyed by
            // sessionId so any UI surface (tab bar, agent panes,
            // mobile companion) can read via
            // `useSessionLabel(sessionId)`. Also write through to
            // `Tab.title` for backwards-compat with components
            // that read tab.title directly.
            const newLabel = parsed.payload.label ?? ''
            useSessionLabelsStore
              .getState()
              .setSessionLabel(sessionId, newLabel)
            if (newLabel && tabId) {
              useTabsStore.getState().setTabTitle(tabId, newLabel)
            }
            break
          }
          case 'bell': {
            // Bell — same signal iTerm uses for "agent waiting"
            // notifications. Claude / Codex ring the bell when
            // they're done and ready for input. Use it as a
            // definitive idle transition.
            if (import.meta.env.DEV) {
              // eslint-disable-next-line no-console
              console.warn(`[v2-activity] BELL tid=${terminalId.slice(0, 8)}`)
            }
            lastSeenWorkingAtRef.current = 0
            useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
            break
          }
          case 'child_exit': {
            // 0.39.9: update `phaseRef.current` SYNCHRONOUSLY with
            // the setPhase call. The phase-sync useEffect runs after
            // React commits, but the daemon typically closes the WS
            // in the same JS task after sending `child_exit` — so
            // `ws.onclose` fires before the useEffect has a chance
            // to update the ref. Writing the ref inline here lets
            // `ws.onclose` correctly see phase=exited and skip the
            // reconnect. Without this, the closure-capture fix from
            // 0.39.8 → 0.39.9 still has a synchronous-event race
            // that resurrects the exited terminal.
            const next: Phase = {
              kind: 'exited',
              sessionId,
              exitCode: parsed.payload.exit_code,
            }
            phaseRef.current = next
            setPhase(next)
            // K2 #682 — surface the exit (with code/timing) so a
            // consumer can run a spawn-loop circuit breaker. Fired via
            // the ref to avoid stale-closure / re-subscribe churn.
            onChildExitRef.current?.(parsed.payload.exit_code)
            break
          }
          case 'error':
            setPhase({ kind: 'error', message: parsed.payload.message })
            break
        }
      }

      ws.onerror = () => {
        // Ignore events from a socket we've already replaced/closed.
        if (wsRef.current !== ws) return
        // If we already received child_exit, the daemon initiated the
        // teardown and any onerror that follows is a concurrent TCP
        // close, not a real failure. Don't clobber the 'exited' state.
        setPhase((prev) =>
          prev.kind === 'exited' ? prev : { kind: 'error', message: 'ws error' },
        )
      }
      ws.onclose = (ev) => {
        // Ignore close events from a socket we've already replaced (the
        // grid-WS effect closed it on hide, or a newer connect superseded
        // it). Only the live socket may schedule a reconnect.
        if (wsRef.current !== ws) return
        wsRef.current = null
        // Issue #5 + Issue #8 (0.39.13): a genuine mid-flight WS drop
        // (TCP reset, WebKit Networking quirk, App Nap) on a VISIBLE,
        // not-yet-exited pane schedules a reconnect by bumping
        // `reconnectAttempt` — the spawn effect re-runs (idempotent
        // spawn re-attaches the same daemon PTY) and the bumped
        // `spawnGeneration` re-opens a fresh grid-WS.
        //
        // `shouldHoldGridWs` is the single pure predicate for "should
        // this pane be streaming right now". It folds in BOTH gates:
        //   - exited ⇒ don't reconnect (child really exited; 0.39.9
        //     phaseRef-synchronous fix prevents resurrecting it).
        //   - hidden ⇒ don't reconnect (we deliberately closed the WS
        //     on hide; the grid-WS effect reopens on the next show, and
        //     reconnecting a stream nobody watches is the #8 pile-up).
        if (!shouldHoldGridWs({
          visible: tabVisibleRef.current,
          exited: phaseRef.current.kind === 'exited',
        })) {
          return
        }
        // Coalesce: if a timer is already pending, don't double-schedule.
        if (reconnectTimerRef.current !== null) return
        // Backoff between attempts. Caps at 5s so a sustained outage
        // doesn't spin forever, but the first reconnect after a
        // single-shot drop is fast (~500ms) so the user barely sees it.
        const delayMs = Math.min(500 * 2 ** Math.min(reconnectAttempt, 4), 5000)
        if (import.meta.env.DEV) {
          // eslint-disable-next-line no-console
          console.warn(
            `[v2-reconnect] tid=${terminalId.slice(0, 8)} ws closed (code=${ev.code}) — reconnect in ${delayMs}ms (attempt #${reconnectAttempt + 1})`,
          )
        }
        // Phase → 'connecting' so the UI shows we're recovering,
        // not stuck in 'ready' with a dead WS underneath.
        setPhase((prev) =>
          prev.kind === 'exited' ? prev : { kind: 'connecting', sessionId },
        )
        reconnectTimerRef.current = setTimeout(() => {
          reconnectTimerRef.current = null
          setReconnectAttempt((n) => n + 1)
        }, delayMs)
      }
  }, [terminalId, perfLog, reconnectAttempt])

  // Keep the ref pointing at the latest `openGridWs` so the grid-WS
  // lifecycle effect (which doesn't take the big closure as a dep) always
  // invokes the current implementation.
  useEffect(() => {
    openGridWsRef.current = openGridWs
  }, [openGridWs])

  // ── Grid-WS lifecycle effect (0.39.13) ────────────────────────
  // The ONLY place the grid-WS opens or closes. Keyed on
  // `spawnGeneration` (a fresh PTY became available — spawn / reconnect)
  // and `isTabVisible` (real visible↔hidden transitions). A spurious
  // re-render with unchanged visibility is a guarded no-op via
  // `appliedVisibleRef`, so a steadily-visible idle pane holds exactly
  // ONE grid-WS that never churns on the active-agents poll cycle —
  // the v1 regression this fix removes.
  useEffect(() => {
    const visible = isTabVisible
    const haveSession = sessionIdRef.current !== null
    // Whether we should be streaming right now. `exited` is read from
    // the live phase ref so an exited pane never (re)opens.
    const wantOpen =
      haveSession &&
      shouldHoldGridWs({ visible, exited: phaseRef.current.kind === 'exited' })

    appliedVisibleRef.current = visible

    if (wantOpen) {
      // `openGridWs` is a guarded no-op if a socket is already
      // open/connecting, so a re-run that didn't actually change
      // visibility won't churn the stream.
      void openGridWsRef.current()
    } else {
      // Not streaming: park (visible but no session yet / exited) or
      // hidden. Close any live socket — PTY survives on the daemon
      // (never /cli/sessions/v2/close). Cancel a pending reconnect so a
      // hide-while-reconnecting doesn't reopen behind our back.
      if (reconnectTimerRef.current !== null) {
        clearTimeout(reconnectTimerRef.current)
        reconnectTimerRef.current = null
      }
      const ws = wsRef.current
      if (ws) {
        // Detach onclose first so closing here doesn't trip the
        // reconnect scheduler (`wsRef.current !== ws` would also catch
        // it, but null the ref explicitly to be unambiguous).
        wsRef.current = null
        if (ws.readyState !== WebSocket.CLOSED) ws.close()
        // A pane that is still visible but lost its session (shouldn't
        // happen) stays in its current phase; a hidden pane with a known
        // session parks so the UI reflects "warm, not streaming".
        const sid = sessionIdRef.current
        if (!visible && sid) {
          setPhase((prev) =>
            prev.kind === 'exited' ? prev : { kind: 'parked', sessionId: sid },
          )
        }
      }
    }
    // No cleanup that closes the socket: closing is driven by the
    // reconcile above (on a real hide) and by the unmount effect below.
    // Closing in cleanup would tear the WS down on every benign re-run.
  }, [spawnGeneration, isTabVisible])

  // ── Grid-WS unmount teardown (0.39.13) ────────────────────────
  // Real unmount only ([] deps): close the WS (PTY survives — never
  // /cli/sessions/v2/close; deliberate tab-close teardown is in A6 via
  // tabs.ts::removeTab) and cancel any pending reconnect. Split from the
  // reconcile effect so a visibility/generation re-run never closes the
  // socket as a side effect.
  useEffect(() => {
    return () => {
      if (reconnectTimerRef.current !== null) {
        clearTimeout(reconnectTimerRef.current)
        reconnectTimerRef.current = null
      }
      const ws = wsRef.current
      if (ws && ws.readyState !== WebSocket.CLOSED) ws.close()
      wsRef.current = null
    }
  }, [])

  // ── A7.5 perf: first_render + tui_first_paint + SUMMARY ──────
  // first_render fires once after `setSnapshot` causes a paint.
  // tui_first_paint fires once when the grid transitions from
  // empty → non-empty (cold spawn — child wrote its first bytes)
  // OR collapses with first_render when the initial snapshot was
  // already non-empty (reattach).
  useEffect(() => {
    if (!import.meta.env.DEV) return
    if (!snapshot) return

    if (!firstRenderFiredRef.current) {
      firstRenderFiredRef.current = true
      perfLog('first_render')
      const stages = stageMsRef.current
      const total = Math.round(
        performance.now() - (mountT0Ref.current ?? 0),
      )
      const reused = firstSnapshotReusedRef.current
      // eslint-disable-next-line no-console
      console.info(
        `[v2-perf] SUMMARY total_render_ms=${total} reused=${reused}` +
          ` mount=${Math.round(stages.mount ?? 0)}` +
          ` creds_end=${Math.round(stages.creds_end ?? 0)}` +
          ` spawn_fetch_end=${Math.round(stages.spawn_fetch_end ?? 0)}` +
          ` ws_open=${Math.round(stages.ws_open ?? 0)}` +
          ` first_snapshot=${Math.round(stages.first_snapshot ?? 0)}` +
          ` first_render=${Math.round(stages.first_render ?? 0)}`,
      )
      // Reattach scenario: initial snapshot already had content.
      // Collapse tui_first_paint with first_render.
      if (
        !firstSnapshotEmptyRef.current &&
        !tuiFirstPaintFiredRef.current
      ) {
        tuiFirstPaintFiredRef.current = true
        perfLog('tui_first_paint', { collapsed: 'true' })
        // eslint-disable-next-line no-console
        console.info(
          `[v2-perf] TUI_SUMMARY total_tui_ms=${total} reused=${reused} collapsed=true`,
        )
      }
    }

    // Cold spawn path: wait for the first non-empty grid update.
    if (
      !tuiFirstPaintFiredRef.current &&
      firstSnapshotEmptyRef.current &&
      !isGridEmpty(snapshot)
    ) {
      tuiFirstPaintFiredRef.current = true
      perfLog('tui_first_paint')
      const stages = stageMsRef.current
      const total = Math.round(
        performance.now() - (mountT0Ref.current ?? 0),
      )
      const renderToTui = Math.round(
        (stages.tui_first_paint ?? 0) - (stages.first_render ?? 0),
      )
      // eslint-disable-next-line no-console
      console.info(
        `[v2-perf] TUI_SUMMARY total_tui_ms=${total}` +
          ` reused=${firstSnapshotReusedRef.current}` +
          ` render_to_tui_ms=${renderToTui}`,
      )
    }
  }, [snapshot, perfLog])

  // ── Focus tracking ────────────────────────────────────────────
  // 0.37.9 — focus tracking moved to the shadow input. Visible
  // "this pane is focused" state (border highlights, etc.) keys on
  // shadow input focus, since that's where keystrokes actually land.
  useEffect(() => {
    const el = shadowInputRef.current
    if (!el) return
    const on = () => setIsFocused(true)
    const off = () => setIsFocused(false)
    el.addEventListener('focus', on)
    el.addEventListener('blur', off)
    return () => {
      el.removeEventListener('focus', on)
      el.removeEventListener('blur', off)
    }
  }, [])

  // Auto-focus when tab becomes visible — focus the shadow input
  // so dictation/typed input both work without an extra click.
  useEffect(() => {
    if (!isTabVisible) return
    const el = shadowInputRef.current
    if (!el) return
    const raf = requestAnimationFrame(() => el.focus())
    return () => cancelAnimationFrame(raf)
  }, [isTabVisible])

  // Re-focus terminal when the OS window regains focus (e.g.,
  // switching back from another app). Only re-focuses if the
  // shadow input held focus before the window blur — prevents
  // stealing focus from a sidebar input the user clicked into.
  // Mirrors AlacrittyTerminalView.tsx's pattern.
  useEffect(() => {
    const shadow = shadowInputRef.current
    const container = containerRef.current
    if (!shadow || !container) return
    let wasFocused = false
    const onBlur = () => {
      wasFocused =
        document.activeElement === shadow ||
        document.activeElement === container ||
        container.contains(document.activeElement)
    }
    const onFocus = () => {
      if (!wasFocused) return
      requestAnimationFrame(() => shadow.focus())
    }
    window.addEventListener('blur', onBlur)
    window.addEventListener('focus', onFocus)
    return () => {
      window.removeEventListener('blur', onBlur)
      window.removeEventListener('focus', onFocus)
    }
  }, [])

  // ── Cell metrics (for cursor positioning + wheel math) ────────
  const [cellMetrics, setCellMetrics] = useState({ width: 0, height: 0 })
  useLayoutEffect(() => {
    const span = document.createElement('span')
    span.style.cssText = `font-family: ${config.font.family}; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    span.textContent = 'W'
    document.body.appendChild(span)
    const rect = span.getBoundingClientRect()
    document.body.removeChild(span)
    setCellMetrics({
      width: rect.width,
      height: Math.ceil(fontSize * config.font.lineHeightMultiplier),
    })
  }, [fontSize, config.font.family, config.font.lineHeightMultiplier])

  // ── Send input / resize ───────────────────────────────────────
  const sendInput = useCallback((text: string) => {
    const ws = wsRef.current
    if (!ws || ws.readyState !== WebSocket.OPEN) return
    ws.send(JSON.stringify({ action: 'input', text }))
  }, [])

  // 0.37.11 — active-viewer resize protocol.
  //
  // Two layers cooperate here:
  //
  //   (1) Renderer-side: gate `sendResize` on this window's OS focus.
  //       Only the focused window emits resize at all. Keeps the
  //       wire quiet when multiple windows view the same session.
  //
  //   (2) Daemon-side: every WS connection gets a `subscriber_id`
  //       on accept. The renderer sends `{action:"set_active",
  //       active:true}` on window focus and `false` on blur. Daemon
  //       stamps `session.active_subscriber` accordingly. Resize
  //       frames are accepted only from the active subscriber —
  //       even if a non-active viewer accidentally emits one, the
  //       daemon drops it. Hard enforcement, no toe-stepping.
  //
  // Generalizes naturally to mobile companion: any subscriber that
  // sends `set_active:true` becomes the resize authority for that
  // session until another claims or it disconnects.
  const lastResizeRef = useRef<{ cols: number; rows: number } | null>(null)
  const sendResize = useCallback((cols: number, rows: number) => {
    lastResizeRef.current = { cols, rows }
    if (!useWindowFocusStore.getState().isFocused) return
    const ws = wsRef.current
    if (!ws || ws.readyState !== WebSocket.OPEN) return
    ws.send(JSON.stringify({ action: 'resize', cols, rows }))
  }, [])

  // Emit `set_active` on focus changes + re-emit latest dimensions
  // when this window regains focus so the daemon snaps the PTY to
  // our grid size. Also emits an initial claim/release at mount
  // time based on current focus state — without it, a freshly-
  // mounted pane in a non-focused window would never tell the
  // daemon it exists, leaving `active_subscriber` stale until the
  // next focus transition.
  // Tracks the last `set_active` value we sent over THIS pane's WS so
  // we can short-circuit duplicate emissions. Ref (not state) — the
  // value is wire-protocol state, not React state; we never want a
  // re-render from updating it. Reset to `null` (== "no value sent
  // yet") whenever `wsRef.current` changes identity (a new WS = a new
  // dedup window). See the effect below.
  const lastSentActiveRef = useRef<boolean | null>(null)
  // 0.39.43 (PRD Issue A) — has THIS component instance sent its first
  // `set_active` yet? The cross-remount dedup (skip a re-claim when a
  // bare re-mount didn't change focus) only applies to the FIRST send
  // of a fresh instance. Subsequent sends within the same instance —
  // genuine focus transitions and WS-reconnect re-primes — must always
  // go out (a reconnect's daemon subscriber is new and needs the claim).
  const hasSentActiveThisInstanceRef = useRef(false)
  // Issue #8 — stable handle to the "recompute desired active state and
  // send it (dedup-guarded)" routine. Lives in a ref so the boot
  // effect's WS-connect path (a different `[]`-deps effect) can call
  // the latest implementation without taking it as a dep. The set_active
  // effect below assigns it once on mount.
  const recomputeAndSendActiveRef = useRef<() => void>(() => {})

  useEffect(() => {
    // The active-viewer handshake is a feature, not noise — it tells
    // the daemon WHICH connected client is the live viewer so it can
    // size the grid and route focus events correctly when multiple
    // clients share one PTY (desktop + mobile, or split panes). In
    // the single-viewer case it should be silent: one initial claim
    // when the WS opens, then nothing until window focus genuinely
    // changes.
    //
    // The send-level dedup (`lastSentActiveRef`) makes that
    // single-viewer silence robust regardless of how often upstream
    // re-renders this effect — a defense against the thrash filed as
    // Issue #3 where the daemon's grid broadcast overran by 3409
    // events because the renderer was emitting `set_active` in a
    // tight loop. (Caused by `phase.kind` churn re-firing the effect
    // and the focus subscriber re-emitting on each transition with
    // no idempotence guard.)
    const sendSetActive = (active: boolean): void => {
      // Idempotent: skip the WS write if the daemon already saw
      // this exact value from us. Closes Issue #3 even if upstream
      // (`isFocused`, `isTabVisible`, window focus, `phase.kind`)
      // ever flaps again — this dedup is what makes recomputing on
      // every source change safe (no re-run thrash reaches the wire).
      if (lastSentActiveRef.current === active) return
      const ws = wsRef.current
      if (!ws || ws.readyState !== WebSocket.OPEN) return
      const sessionId = sessionIdRef.current
      // 0.39.43 (PRD Issue A) — cross-remount re-claim suppression.
      // On the FIRST send from this fresh component instance, if the
      // previous instance already sent this exact value for this
      // session, the re-mount changed nothing on the wire: skip it so
      // the local window doesn't re-steal the daemon's active slot from
      // a remote viewer on a bare re-mount (attachNonce bump). Genuine
      // focus transitions compute a DIFFERENT value → not skipped.
      // WS-reconnect re-primes (same instance, hasSent=true) bypass
      // this — their daemon subscriber is new and needs the claim.
      if (
        !hasSentActiveThisInstanceRef.current &&
        sessionId &&
        getLastSentActive(sessionId) === active
      ) {
        // Adopt the persisted value as our dedup baseline so later
        // recomputes in this instance still short-circuit correctly,
        // but emit nothing now.
        lastSentActiveRef.current = active
        hasSentActiveThisInstanceRef.current = true
        return
      }
      // 0.39.43 (PRD Issue A) — carry the active viewer's current
      // viewport dims on the CLAIM so the daemon snaps the PTY to our
      // size the instant we become active (no waiting for the follow-up
      // Resize frame). Release (`active:false`) carries no dims. Dims
      // are optional on the wire — an older daemon ignores them.
      const payload: {
        action: 'set_active'
        active: boolean
        cols?: number
        rows?: number
      } = { action: 'set_active', active }
      if (active && lastResizeRef.current) {
        payload.cols = lastResizeRef.current.cols
        payload.rows = lastResizeRef.current.rows
      }
      ws.send(JSON.stringify(payload))
      lastSentActiveRef.current = active
      hasSentActiveThisInstanceRef.current = true
      if (sessionId) recordSentActive(sessionId, active)
    }

    // Issue #8 — single source of truth for the active claim. The
    // desired state is visible AND pane-focused AND window-focused.
    // We read all three from refs/store (no React deps) so every
    // caller — the window-focus subscriber, the WS-connect re-prime,
    // and the visibility/focus recompute effect — converges on the
    // same dedup-guarded send. A pane that is hidden or blurred sends
    // `set_active(false)`; only the visible+focused pane claims.
    const recomputeAndSendActive = (): void => {
      const windowFocused = useWindowFocusStore.getState().isFocused
      const desired = computeDesiredActive({
        visible: tabVisibleRef.current,
        paneFocused: paneFocusedRef.current,
        windowFocused,
      })
      sendSetActive(desired)
    }
    // Expose the latest implementation to the boot effect's WS-connect
    // re-prime path (a separate effect can't close over this one).
    recomputeAndSendActiveRef.current = recomputeAndSendActive

    let wasFocused = useWindowFocusStore.getState().isFocused
    // Initial claim — happens after WS is open. The boot effect
    // wires `wsRef.current` once the v2 spawn completes; if the
    // WS isn't open yet when this fires, the send is a no-op.
    // It's fine — the recompute paths below will claim when the
    // user interacts / the pane becomes visible+focused.
    recomputeAndSendActive()

    const unsub = useWindowFocusStore.subscribe((state) => {
      const nowFocused = state.isFocused
      if (wasFocused !== nowFocused) {
        // Recompute against the full predicate — window focus is only
        // one of the three inputs now. The dedup short-circuits if the
        // combined result didn't actually change (e.g. window regains
        // focus but this pane is hidden → still `false`, no send).
        recomputeAndSendActive()
        // On focus-gain, re-emit the latest dimensions so the PTY
        // snaps to this window's grid — but only if we're actually
        // the active viewer now (visible+focused). A hidden pane must
        // not ping-pong resizes on window focus (the #8 flood path).
        if (
          !wasFocused &&
          nowFocused &&
          lastSentActiveRef.current === true &&
          lastResizeRef.current
        ) {
          const ws = wsRef.current
          if (ws && ws.readyState === WebSocket.OPEN) {
            ws.send(JSON.stringify({
              action: 'resize',
              cols: lastResizeRef.current.cols,
              rows: lastResizeRef.current.rows,
            }))
          }
        }
      }
      wasFocused = nowFocused
    })
    return () => {
      unsub()
      recomputeAndSendActiveRef.current = () => {}
      // Symmetric cleanup — if we claimed active on mount, release
      // on unmount so the daemon's `active_subscriber` tracking
      // doesn't think a torn-down pane is still the active viewer.
      // The send-level dedup means this is a no-op when we never
      // claimed (initial `isFocused` was false), so it's safe to
      // call unconditionally.
      const ws = wsRef.current
      if (ws && ws.readyState === WebSocket.OPEN && lastSentActiveRef.current === true) {
        ws.send(JSON.stringify({ action: 'set_active', active: false }))
        lastSentActiveRef.current = false
        // 0.39.43 (PRD Issue A): deliberately do NOT
        // `recordSentActive(sessionId, false)` here. This release is a
        // teardown artifact, not a focus decision. On a bare re-mount
        // (attachNonce bump) the new instance must see the last GENUINE
        // active decision (still `true` if the user remained focused)
        // so it can skip a redundant re-claim. Recording `false` here
        // would poison that baseline and make every re-mount re-claim
        // — exactly the local-window-re-steals bug we're fixing.
      }
    }
    // NOTE: `phase.kind` was previously in this dep array — pre-Issue
    // #3 it caused this effect to tear down + re-mount on every phase
    // transition (mount, ready, exited, error, …), each time re-firing
    // the initial `sendSetActive(wasFocused)` and starting a fresh
    // focus subscriber. The effect body doesn't actually read
    // `phase.kind` (it reads `wsRef.current` + the focus store), so
    // the dep was load-bearing for nothing and amplified the thrash.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // Issue #8 — recompute the active claim when pane-focus or tab
  // visibility changes (window-focus changes are handled by the
  // store subscriber inside the effect above). These two inputs are
  // React render values, so a dep-driven effect is the natural
  // trigger. It runs AFTER the two mirror-ref effects (declared
  // earlier, so they commit first) — meaning `paneFocusedRef` /
  // `tabVisibleRef` already hold the new values when we recompute.
  //
  // This is the release-on-hidden + release-on-blur path (Part 1/2):
  // when this pane loses visibility or pane-focus the recompute
  // yields `false` and `sendSetActive(false)` releases the claim.
  // It does NOT reintroduce Issue #3: the effect only re-runs when
  // `isFocused`/`isTabVisible` genuinely change (real user-visible
  // transitions, not a tight loop), and every send still passes
  // through the `lastSentActiveRef` dedup — so even a spurious
  // re-run that computes the same value writes nothing to the wire.
  useEffect(() => {
    recomputeAndSendActiveRef.current()
  }, [isFocused, isTabVisible])

  // ── Keyboard input ────────────────────────────────────────────
  // 0.37.9 — handlers attach to the shadow <textarea> instead of the
  // container <div>. Same key→escape sequence pipeline; the textarea
  // is where AppKit looks for a text input target (so Fn-Fn
  // Dictation engages here), while the visible grid stays
  // pointer-events-driven below for selection + link hover. See PRD:
  // .k2so/prds/voice-dictation.md.
  useEffect(() => {
    if (phase.kind !== 'ready') return
    const el = shadowInputRef.current
    if (!el) return

    const onKey = (e: KeyboardEvent) => {
      // Don't intercept keystrokes mid-IME composition. The textarea
      // absorbs them; compositionend commits the final string in one
      // sendInput call.
      if (composingRef.current) return
      const natural = naturalTextEditingSequence(e)
      if (natural !== null) {
        e.preventDefault()
        setViewportOffset(0)
        sendInput(natural)
        // Clear so the textarea never accumulates.
        if (shadowInputRef.current) shadowInputRef.current.value = ''
        return
      }
      const seq = keyEventToSequence(e, 0)
      if (seq === null) return
      e.preventDefault()
      setViewportOffset(0)
      sendInput(seq)
      if (shadowInputRef.current) shadowInputRef.current.value = ''
    }
    const onPaste = (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData('text') ?? ''
      e.preventDefault()
      setViewportOffset(0)

      // Finder's Cmd+C copies file refs via NSFilenamesPboardType,
      // which WKWebView doesn't expose through the web clipboard
      // API. Query the native pasteboard: if file paths are
      // present, paste them shell-escaped (matching v1's drag-drop
      // behavior). Fall back to text paste otherwise.
      daemonCliGet<string[]>('fs/clipboard-paths')
        .then((paths) => {
          if (paths && paths.length > 0) {
            sendInput(buildDropPayload(paths))
            return
          }
          if (text) sendInput(text)
        })
        .catch(() => {
          if (text) sendInput(text)
        })
      // Always clear; preventDefault blocks the browser's own insert,
      // but onInput fires after paste — clearing here keeps the
      // textarea empty so the input handler's `text.length === 0`
      // guard short-circuits cleanly.
      if (shadowInputRef.current) shadowInputRef.current.value = ''
    }

    // Apple Dictation, IME final commits, and any non-keystroke text
    // delivery (drag-drop into textarea, accessibility text input)
    // all fire `input`. `keydown` already handled normal keystrokes
    // above with preventDefault, so the textarea was never given the
    // chance to insert their characters. What's left here is dictated
    // / IME-committed text only.
    const onInput = () => {
      if (composingRef.current) return
      const text = el.value
      if (text.length === 0) return
      setViewportOffset(0)
      sendInput(text)
      el.value = ''
    }

    // 0.37.9 — composition handling matches xterm.js's strategy:
    // do NOT write to the PTY during compositionupdate. Apple
    // Dictation, IME candidate windows, and accent pickers all
    // deliver progressive best-guesses via compositionupdate that
    // get autocorrected at compositionend. Streaming with
    // backspace+retype during update events is what every
    // WebView-based terminal avoids — it causes lag spikes on
    // dictation engage (AppKit's rect query stalls if updates fire
    // while it's polling), and it interacts badly with TUI apps
    // that interpret \x7f differently.
    //
    // We commit only at compositionend. Future enhancement: render
    // a visible preview overlay at the cursor (xterm.js's
    // `_compositionView`) so the user sees recognized text as they
    // speak. The text doesn't reach the PTY until they stop, but
    // they get visual feedback in the meantime.
    const onComposeStart = () => {
      composingRef.current = true
      compositionLastLengthRef.current = 0
    }
    // Stream the running transcript into the PTY on every update.
    // Apple Dictation delivers a full best-guess string (not a
    // delta) each time — "Hello" → "Hello world" → "Hello world,
    // how" — so we backspace away the prior partial and retype the
    // new one. Words appear at the prompt as the user speaks; the
    // cursor advances naturally; brief flicker on autocorrect is
    // the only side effect.
    const onComposeUpdate = (e: CompositionEvent) => {
      const text = e.data ?? ''
      const prevLen = compositionLastLengthRef.current
      // \x7f (DEL) is what readline + claude TUI + most line
      // editors accept as "delete previous character." \x08 (BS,
      // Ctrl+H) is intercepted by some apps for help-back.
      if (prevLen > 0) {
        sendInput('\x7f'.repeat(prevLen))
      }
      if (text.length > 0) {
        sendInput(text)
      }
      // Grapheme count, not utf-16 — Dictation can produce emoji /
      // multi-codepoint clusters where the surrogate pair is one
      // visible character (one DEL).
      compositionLastLengthRef.current = [...text].length
    }
    const onComposeEnd = (e: CompositionEvent) => {
      composingRef.current = false
      const committed = e.data ?? ''
      const prevLen = compositionLastLengthRef.current
      // Reconcile partial → final. If they're identical, this
      // backspace-and-retype is wasteful but harmless. If
      // Dictation autocorrected on stop ("their" → "there"), this
      // is what makes the PTY content match what the user said.
      if (prevLen > 0) {
        sendInput('\x7f'.repeat(prevLen))
      }
      if (committed) {
        setViewportOffset(0)
        sendInput(committed)
      }
      compositionLastLengthRef.current = 0
      el.value = ''
    }

    el.addEventListener('keydown', onKey)
    el.addEventListener('paste', onPaste)
    el.addEventListener('input', onInput)
    el.addEventListener('compositionstart', onComposeStart)
    el.addEventListener('compositionupdate', onComposeUpdate)
    el.addEventListener('compositionend', onComposeEnd)
    el.focus()
    return () => {
      el.removeEventListener('keydown', onKey)
      el.removeEventListener('paste', onPaste)
      el.removeEventListener('input', onInput)
      el.removeEventListener('compositionstart', onComposeStart)
      el.removeEventListener('compositionupdate', onComposeUpdate)
      el.removeEventListener('compositionend', onComposeEnd)
    }
  }, [phase.kind, sendInput])

  // ── Compose visible rows ──────────────────────────────────────
  //
  // Declared before the link-detection handlers below because
  // `handleMouseMove` closes over `visibleRows` and JS temporal-
  // dead-zone rules reject the closure at render time if the
  // `const` is declared later. (Same class of fix as the
  // cellMetrics hoist that happened earlier in the Kessel-T0
  // work.)
  // Visible rows + their absolute (scrollback-anchored) row indices.
  // Keying the rendered row divs by absolute index — instead of by
  // visual 0..N position — keeps the same DOM node attached to the
  // same logical row across scrolls. The browser's text selection is
  // anchored to text nodes inside those divs; if the divs survive
  // (just move position), native selection follows the content as
  // expected. Without this, scrolling reused row divs with new
  // content and the highlight visually "stayed" while text moved.
  const { visibleRows, visibleRowAbsRows } = useMemo(() => {
    if (!snapshot) {
      return { visibleRows: [] as CellRun[][], visibleRowAbsRows: [] as number[] }
    }
    const { scrollback, grid, rows: r } = snapshot
    const totalLen = scrollback.length + grid.length
    const windowEnd = totalLen - viewportOffset
    const windowStart = windowEnd - r
    const rows: CellRun[][] = []
    const abs: number[] = []
    for (let i = 0; i < r; i++) {
      const a = windowStart + i
      abs.push(a)
      if (a < 0) rows.push([])
      else if (a < scrollback.length) rows.push(scrollback[a])
      else rows.push(grid[a - scrollback.length])
    }
    return { visibleRows: rows, visibleRowAbsRows: abs }
  }, [viewportOffset, snapshot])

  // ── Link detection: Cmd key tracking ──────────────────────────
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Meta') cmdHeldRef.current = true
    }
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === 'Meta') {
        cmdHeldRef.current = false
        if (linkClickMode === 'cmd-click') setHoveredLink(null)
      }
    }
    const onBlur = () => {
      cmdHeldRef.current = false
      setHoveredLink(null)
    }
    document.addEventListener('keydown', onKeyDown)
    document.addEventListener('keyup', onKeyUp)
    window.addEventListener('blur', onBlur)
    return () => {
      document.removeEventListener('keydown', onKeyDown)
      document.removeEventListener('keyup', onKeyUp)
      window.removeEventListener('blur', onBlur)
    }
  }, [linkClickMode])

  // ── Link detection: hover → {row, link} state ─────────────────
  const handleMouseMove = useCallback(
    (e: React.MouseEvent) => {
      if (linkClickMode === 'cmd-click' && !cmdHeldRef.current) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      // Throttle: skip if mouse moved < 4px and < 80ms since last.
      const now = Date.now()
      const dx = e.clientX - lastDetectPosRef.current.x
      const dy = e.clientY - lastDetectPosRef.current.y
      if (dx * dx + dy * dy < 16 && now - lastDetectTimeRef.current < 80) return
      lastDetectPosRef.current = { x: e.clientX, y: e.clientY }
      lastDetectTimeRef.current = now

      const el = containerRef.current
      if (!el || !snapshot) return
      const rect = el.getBoundingClientRect()
      const { width: cw, height: ch } = cellMetrics
      if (cw === 0 || ch === 0) return
      // The 4px padding on the container biases cell positions —
      // subtract before dividing.
      const row = Math.floor((e.clientY - rect.top - 4) / ch)
      const col = Math.floor((e.clientX - rect.left - 4) / cw)
      const visibleRow = visibleRows[row]
      if (!visibleRow) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      const text = rowToText(visibleRow)
      if (!text.trim()) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      const links = detectLinks(text, cwd)
      const hit = links.find((l) => col >= l.start && col < l.end)
      if (hit) {
        if (
          !hoveredLink ||
          hoveredLink.row !== row ||
          hoveredLink.link.start !== hit.start
        ) {
          setHoveredLink({ row, link: hit })
        }
      } else if (hoveredLink) {
        setHoveredLink(null)
      }
    },
    [linkClickMode, hoveredLink, cellMetrics, snapshot, visibleRows, cwd],
  )

  const handleMouseLeave = useCallback(() => {
    if (hoveredLink) setHoveredLink(null)
  }, [hoveredLink])

  const handleMouseDown = useCallback(() => {
    mouseDownLinkRef.current = hoveredLink?.link ?? null
    mouseDownInPaneRef.current = true
  }, [hoveredLink])

  // 0.37.11 — global mouseup safety net. Catches the case where the
  // user starts a drag inside the pane and releases outside its
  // bounds; without this the pane-level onMouseUp would never fire
  // and `mouseDownInPaneRef` would stay stuck at true.
  useEffect(() => {
    const onGlobalMouseUp = (): void => {
      mouseDownInPaneRef.current = false
    }
    window.addEventListener('mouseup', onGlobalMouseUp)
    return () => window.removeEventListener('mouseup', onGlobalMouseUp)
  }, [])

  const handleClick = useCallback(
    (e: React.MouseEvent) => {
      if (linkClickMode === 'cmd-click' && !e.metaKey) return
      if (!hoveredLink) return
      // Validate: mouse-down must have been on the same link so a
      // drag-to-link doesn't false-click.
      const downLink = mouseDownLinkRef.current
      mouseDownLinkRef.current = null
      if (
        !downLink ||
        downLink.start !== hoveredLink.link.start ||
        downLink.target !== hoveredLink.link.target
      ) {
        return
      }

      const clicked = hoveredLink.link
      e.preventDefault()
      e.stopPropagation()

      if (clicked.type === 'url') {
        daemonCliPost('fs/open-external', { target: clicked.target }).catch((err) =>
          console.warn('[terminal-v2/link]', err),
        )
      } else if (clicked.type === 'file' && clicked.filePath) {
        const tabsStore = useTabsStore.getState()
        const openInSplit =
          useTerminalSettingsStore.getState().openLinksInSplitPane

        if (openInSplit && tabId && paneGroupId) {
          const tab = tabsStore.tabs.find((t) => t.id === tabId)
          if (tab && tab.paneGroups.size > 1) {
            const siblingId = [...tab.paneGroups.keys()].find(
              (id) => id !== paneGroupId,
            )
            if (siblingId) {
              tabsStore.openFileInPaneGroup(tabId, siblingId, clicked.filePath)
              return
            }
          }
        }
        tabsStore.openFileInNewTab(clicked.filePath)
      }
    },
    [linkClickMode, hoveredLink, tabId, paneGroupId],
  )

  // ── Drag + drop of files (from Finder or K2 files tab) ──────
  //
  // V2 needs TWO drop entry points because Tauri intercepts external
  // (Finder → window) drops at the webview level — the React onDrop
  // never fires for those:
  //
  //   1. `tauri://drag-drop` window-level event (from Finder /
  //      external apps). Mirrors `AlacrittyTerminalView` (legacy).
  //      Hit-tests the drop position against this terminal's
  //      container so split layouts only inject into the pane the
  //      drop actually landed on.
  //
  //   2. `k2so:terminal-write` CustomEvent dispatched by
  //      `lib/file-drag.ts` on mouseup over a v2 container.
  //      Internal FileTree drags never leave the webview so they
  //      don't generate `tauri://drag-drop` — the file-drag helper
  //      tracks the drag manually and dispatches this event when
  //      mouseup is over `data-terminal-kind="v2"`.
  //
  // The React-level `onDrop` handler stays as a no-op fallback
  // (handles the rare case where Tauri's dragDropEnabled is off).
  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    e.stopPropagation()
  }, [])

  const handleDrop = useCallback(
    (e: React.DragEvent) => {
      e.preventDefault()
      e.stopPropagation()
      const files = e.dataTransfer.files
      if (files.length > 0) {
        const paths: string[] = []
        for (let i = 0; i < files.length; i++) {
          // Tauri exposes full path via .path (non-standard field).
          const p = (files[i] as unknown as { path?: string }).path
          if (p) paths.push(p)
        }
        if (paths.length > 0) {
          // REMOTE host: local paths → upload + inject remote (rare DOM
          // fallback under Tauri; native drag-drop is the usual path).
          if (useConnectHostStore.getState().activeHost !== 'local') {
            void executeRemoteDrop(
              paths,
              { kind: 'terminal' },
              { workspacePath: cwd },
              buildDropPayload,
            ).then((payload) => {
              if (payload) sendInput(payload)
            })
          } else {
            sendInput(buildDropPayload(paths))
          }
          return
        }
      }
      const text = e.dataTransfer.getData('text/plain')
      if (text) sendInput(text)
    },
    [sendInput, cwd],
  )

  // External drag-drop from Finder / other apps. Window-level event,
  // hit-test against container so split-pane drops route correctly.
  useEffect(() => {
    let unlisten: (() => void) | undefined
    let cancelled = false
    import('@tauri-apps/api/event').then(({ listen }) => {
      if (cancelled) return
      listen<{ paths: string[]; position: { x: number; y: number } }>(
        'tauri://drag-drop',
        (event) => {
          const { paths, position } = event.payload
          if (!paths || paths.length === 0) return
          if (!position) return
          const el = document.elementFromPoint(position.x, position.y)
          if (!el) return
          // File tree handles its own internal drops.
          if ((el as HTMLElement).closest?.('[data-path]')) return
          // Only accept if the drop landed inside *this* container.
          const container = containerRef.current
          if (!container || !container.contains(el)) return

          // REMOTE host (K2 Connect): `paths` are LOCAL paths with no bytes
          // on the daemon. Upload to the workspace's `.k2so/downloads/` then
          // inject the returned REMOTE paths so the agent can resolve them.
          // buildDropPayload is this pane's existing builder (same quoting),
          // just fed the remote paths.
          if (useConnectHostStore.getState().activeHost !== 'local') {
            void executeRemoteDrop(
              paths,
              { kind: 'terminal' },
              { workspacePath: cwd },
              buildDropPayload,
            ).then((payload) => {
              if (payload) sendInput(payload)
            })
            return
          }
          sendInput(buildDropPayload(paths))
        },
      ).then((fn) => {
        if (cancelled) fn()
        else unlisten = fn
      })
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [sendInput, cwd])

  // Internal drag-drop from K2's file tree. file-drag.ts dispatches
  // this CustomEvent on the v2 container when mouseup lands here.
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const onWrite = (e: Event) => {
      const detail = (e as CustomEvent<{ data: string }>).detail
      if (detail?.data) sendInput(detail.data)
    }
    el.addEventListener('k2so:terminal-write', onWrite)
    return () => el.removeEventListener('k2so:terminal-write', onWrite)
  }, [sendInput])

  // ── ResizeObserver → send resize ──────────────────────────────
  useEffect(() => {
    if (phase.kind !== 'ready') return
    const el = containerRef.current
    if (!el) return
    if (!cellMetrics.width || !cellMetrics.height) return

    let lastCols = 0
    let lastRows = 0
    let timer: ReturnType<typeof setTimeout> | null = null
    const observer = new ResizeObserver((entries) => {
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        const rect = entries[0]?.contentRect
        if (!rect || rect.width === 0 || rect.height === 0) return
        const availW = Math.max(0, rect.width - 8)
        const availH = Math.max(0, rect.height - 8)
        const newCols = Math.floor(availW / cellMetrics.width)
        const newRows = Math.floor(availH / cellMetrics.height)
        if (newCols < 10 || newRows < 3) return
        if (newCols === lastCols && newRows === lastRows) return
        lastCols = newCols
        lastRows = newRows
        sendResize(newCols, newRows)
      }, 100)
    })
    observer.observe(el)
    return () => {
      if (timer) clearTimeout(timer)
      observer.disconnect()
    }
  }, [phase.kind, cellMetrics.width, cellMetrics.height, sendResize])

  // ── Wheel scroll (client-side viewport offset) ────────────────
  const scrollAccumRef = useRef(0)
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const FLUSH_MS = 50
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0) return
      e.preventDefault()
      const cellH = cellMetrics.height || 20
      const pixelDelta =
        e.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? e.deltaY * cellH
          : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? e.deltaY * cellH * (snapshot?.rows ?? 24)
            : e.deltaY
      scrollAccumRef.current += pixelDelta
      if (!scrollTimerRef.current) {
        scrollTimerRef.current = setTimeout(() => {
          scrollTimerRef.current = null
          const accum = scrollAccumRef.current
          scrollAccumRef.current = 0
          if (accum === 0) return
          const lines = Math.round(
            (accum * config.scrolling.multiplier) / cellH,
          )
          if (lines === 0) return
          const maxOffset = snapshot?.scrollback.length ?? 0
          setViewportOffset((o) => {
            const next = o - lines
            if (next <= 0) return 0
            if (next >= maxOffset) return maxOffset
            return next
          })
        }, FLUSH_MS)
      }
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      el.removeEventListener('wheel', onWheel)
      if (scrollTimerRef.current) {
        clearTimeout(scrollTimerRef.current)
        scrollTimerRef.current = null
      }
    }
  }, [config.scrolling.multiplier, cellMetrics.height, snapshot])

  // ── Styles ────────────────────────────────────────────────────
  const containerStyle: React.CSSProperties = useMemo(
    () => ({
      fontFamily: config.font.family,
      fontSize: `${fontSize}px`,
      lineHeight: `${Math.ceil(fontSize * config.font.lineHeightMultiplier)}px`,
      color: `rgb(${(config.colors.foreground >> 16) & 0xff},${(config.colors.foreground >> 8) & 0xff},${config.colors.foreground & 0xff})`,
      backgroundColor: `rgb(${(config.colors.background >> 16) & 0xff},${(config.colors.background >> 8) & 0xff},${config.colors.background & 0xff})`,
      whiteSpace: 'pre',
      padding: '4px',
      position: 'relative',
      overflow: 'hidden',
      flex: 1,
      width: '100%',
      height: '100%',
      outline: 'none',
    }),
    [
      fontSize,
      config.font.family,
      config.font.lineHeightMultiplier,
      config.colors.foreground,
      config.colors.background,
    ],
  )

  // Default fg/bg as CSS strings — passed to runStyle so cells
  // with `inverse=true` and null colors render the proper swap
  // (default-bg text on default-fg block) instead of looking
  // like ordinary text. Used by TUI-drawn cursors.
  const defaultFgCss = useMemo(
    () => hexToCss(config.colors.foreground),
    [config.colors.foreground],
  )
  const defaultBgCss = useMemo(
    () => hexToCss(config.colors.background),
    [config.colors.background],
  )

  // 0.37.9 — Cursor-following shadow textarea position with
  // freeze-during-composition. Mirrors xterm.js's `_syncTextArea`:
  // when not composing, position the textarea AT the visible
  // cursor cell (1 cell wide, 1 row tall). AppKit's
  // `firstRectForCharacterRange:` query then returns the cursor's
  // on-screen rect, so the dictation indicator anchors there.
  // While composing, hold the prior style so AppKit doesn't see
  // the rect move mid-engagement (xterm.js uses the same guard).
  const shadowInputStyleStableRef = useRef<React.CSSProperties | null>(null)
  const shadowInputStyle = useMemo<React.CSSProperties>(() => {
    if (composingRef.current && shadowInputStyleStableRef.current) {
      return shadowInputStyleStableRef.current
    }
    if (snapshot && cellMetrics.width > 0 && cellMetrics.height > 0) {
      const next: React.CSSProperties = {
        position: 'absolute',
        left: `${4 + cellMetrics.width * snapshot.cursor.col}px`,
        top: `${
          4 + cellMetrics.height * (snapshot.cursor.row + viewportOffset)
        }px`,
        width: `${cellMetrics.width}px`,
        height: `${cellMetrics.height}px`,
        lineHeight: `${cellMetrics.height}px`,
        opacity: 0,
        zIndex: -5,
        border: 0,
        outline: 'none',
        padding: 0,
        margin: 0,
        resize: 'none',
        whiteSpace: 'nowrap',
        overflow: 'hidden',
        color: 'transparent',
        background: 'transparent',
        caretColor: 'transparent',
      }
      shadowInputStyleStableRef.current = next
      return next
    }
    shadowInputStyleStableRef.current = SHADOW_INPUT_FALLBACK_STYLE
    return SHADOW_INPUT_FALLBACK_STYLE
  }, [
    snapshot,
    snapshot?.cursor.col,
    snapshot?.cursor.row,
    cellMetrics.width,
    cellMetrics.height,
    viewportOffset,
  ])

  const cursorOverlay: {
    style: React.CSSProperties
    char?: string
  } | null = useMemo(() => {
    if (!snapshot || !cellMetrics.width) return null
    const caretColor = 'rgb(224, 224, 224)'

    // Scenario A — DECTCEM on (regular shell): overlay a block at
    // alacritty's reported cursor position. Focused = solid fill,
    // unfocused = hollow outline. No character needed; the cell
    // span underneath already renders it.
    if (snapshot.cursor.visible && viewportOffset === 0) {
      const cursorVisibleRow = snapshot.cursor.row + viewportOffset
      if (cursorVisibleRow >= 0 && cursorVisibleRow < snapshot.rows) {
        const baseStyle: React.CSSProperties = {
          position: 'absolute',
          left: `${4 + cellMetrics.width * snapshot.cursor.col}px`,
          top: `${4 + cellMetrics.height * cursorVisibleRow}px`,
          width: `${cellMetrics.width}px`,
          height: `${cellMetrics.height}px`,
          pointerEvents: 'none',
          boxSizing: 'border-box',
        }
        // `border` not `box-shadow inset` for the same reason as
        // scenario B — uniform 1px rendering on retina without
        // the half-pixel snapping that thickens the top edge.
        if (isFocused) {
          return {
            style: {
              ...baseStyle,
              backgroundColor: caretColor,
            },
          }
        }
        return {
          style: {
            ...baseStyle,
            backgroundColor: 'transparent',
            border: `1px solid ${caretColor}`,
          },
        }
      }
      return null
    }

    // Scenario B — DECTCEM off (TUI), unfocused. The TUI drew a
    // solid white inverse-cell block at the cursor position with
    // the character rendered in default-bg color (black-on-white).
    // To turn that into a HOLLOW cursor where the character also
    // inverts back to its normal foreground color, we overlay a
    // div with default-bg fill + caret-color hollow outline + the
    // character redrawn in default-fg color. Net effect: the cell
    // visually flips from solid-block-with-inverted-char to
    // outlined-rect-with-normal-char. Skip when focused — the
    // TUI's bright solid block is the cursor we want to see.
    if (!isFocused && !snapshot.cursor.visible && viewportOffset === 0) {
      let found: { row: number; col: number; char: string } | null = null
      for (let r = 0; r < snapshot.grid.length && !found; r++) {
        const row = snapshot.grid[r]
        let cellCol = 0
        for (const run of row) {
          if (run.inverse) {
            // Use the first character of the run — TUI cursors
            // are single-cell so the run's text is one char (or
            // empty for a cursor-on-blank-cell).
            found = {
              row: r,
              col: cellCol,
              char: run.text.charAt(0) || '',
            }
            break
          }
          cellCol += run.text.length
        }
      }
      if (found) {
        // The underlying inverse-cell paints its white bg over
        // the line-box, which on retina + this font extends ~1px
        // above the row's nominal top (font ascender + half-
        // leading). If we sit the overlay exactly on the row's
        // top, that leftover 1px of white peeks above and looks
        // like a 2px top border. Bumping the overlay 1px upward
        // and growing height by 1px absorbs the bleed without
        // disturbing the bottom edge.
        return {
          style: {
            position: 'absolute',
            left: `${4 + cellMetrics.width * found.col}px`,
            top: `${4 + cellMetrics.height * found.row - 1}px`,
            width: `${cellMetrics.width}px`,
            height: `${cellMetrics.height + 1}px`,
            backgroundColor: defaultBgCss,
            color: defaultFgCss,
            border: `1px solid ${caretColor}`,
            pointerEvents: 'none',
            boxSizing: 'border-box',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center',
            padding: 0,
            margin: 0,
            lineHeight: 1,
          },
          char: found.char,
        }
      }
    }

    return null
  }, [snapshot, cellMetrics, viewportOffset, isFocused, defaultBgCss, defaultFgCss])

  // ── Render ────────────────────────────────────────────────────
  if (phase.kind === 'error') {
    return (
      <div
        style={{
          padding: 16,
          color: '#ff6666',
          fontFamily: 'monospace',
          fontSize: 12,
          whiteSpace: 'pre-wrap',
        }}
      >
        Alacritty v2: {phase.message}
      </div>
    )
  }

  const isReady = phase.kind === 'ready' || phase.kind === 'exited'
  const debugSessionId =
    phase.kind === 'ready' || phase.kind === 'connecting' || phase.kind === 'exited'
      ? phase.sessionId
      : null

  // Container cursor hints at link-clickability without rewriting
  // the row DOM (simpler than overlaying underlines per hovered
  // link). Matches v1's affordance.
  const finalContainerStyle: React.CSSProperties = {
    ...containerStyle,
    cursor: hoveredLink ? 'pointer' : 'text',
  }

  return (
    <div
      ref={containerRef}
      className="alacritty-v2-pane"
      data-session-id={debugSessionId}
      // App.tsx's global click + refocus-poll use these two data
      // attributes to find the active terminal and keep it focused
      // after (a) clicks on blank canvas, (b) Cmd+K / Cmd+L
      // palette close, (c) any overlay Esc-out. Matches v1.
      data-terminal-container=""
      data-terminal-visible="true"
      // file-drag.ts (internal FileTree drag) hit-tests for these
      // attributes on mouseup. `data-terminal-id` matches the
      // contract legacy AlacrittyTerminalView established;
      // `data-terminal-kind="v2"` tells file-drag.ts to dispatch a
      // CustomEvent (which TerminalPane's effect routes to sendInput
      // over the WS) instead of calling the legacy `terminal_write`
      // Tauri command — that command only knows about the legacy
      // terminal_manager and would 404 on a v2 session id.
      data-terminal-id={debugSessionId ?? undefined}
      data-terminal-kind="v2"
      tabIndex={0}
      style={finalContainerStyle}
      onFocus={() => {
        // 0.37.9 — App.tsx's global click handler + 200ms refocus
        // poll target [data-terminal-container][data-terminal-visible]
        // and call .focus() on the matched container <div>. We
        // immediately delegate to the shadow textarea so dictation
        // stays addressable. App.tsx already short-circuits if the
        // active element is a TEXTAREA (line 321), so once the
        // shadow input has focus it stays put. See PRD:
        // voice-dictation.md.
        //
        // 0.37.11 — also skip if a mouse drag is in progress. The
        // browser focuses the container <div tabIndex={0}> on
        // mousedown BEFORE the selection range starts being built.
        // If we redirect focus to the shadow textarea at that
        // moment, the in-flight drag's selection gets cancelled.
        // The 0.37.9 onMouseUp guard catches the post-selection
        // case; this catches the mid-drag case.
        if (mouseDownInPaneRef.current) return
        const sel = window.getSelection()
        const hasSelection =
          sel !== null && !sel.isCollapsed && sel.toString().length > 0
        if (
          !hasSelection &&
          shadowInputRef.current &&
          document.activeElement !== shadowInputRef.current
        ) {
          shadowInputRef.current.focus({ preventScroll: true })
        }
      }}
      onMouseMove={handleMouseMove}
      onMouseLeave={handleMouseLeave}
      onMouseDown={handleMouseDown}
      onMouseUp={() => {
        // 0.37.11 — drag ended. Clear the mousedown flag first so
        // the next focus check (or any global handler) sees the
        // user is no longer dragging. Then re-focus the shadow
        // textarea ONLY if there's no live selection — leaving
        // focus on the container preserves the highlighted range.
        // (The container's onFocus handler also guards against
        // mid-drag interruptions so dictation re-engagement
        // doesn't race against selection.)
        mouseDownInPaneRef.current = false
        const sel = window.getSelection()
        const hasSelection =
          sel !== null && !sel.isCollapsed && sel.toString().length > 0
        if (
          !hasSelection &&
          shadowInputRef.current &&
          document.activeElement !== shadowInputRef.current
        ) {
          shadowInputRef.current.focus({ preventScroll: true })
        }
      }}
      onClick={handleClick}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
    >
      {/* 0.37.9 — shadow input. Position pinned at the cursor cell
          AND memoized + frozen-during-composition so the rect AppKit
          queries for `firstRectForCharacterRange:` is stable. xterm.js
          uses this same pattern (their `_syncTextArea` skips when
          `isComposing`); without the guard, every shell-echo
          repositions the textarea, AppKit's Dictation rect query
          races the React re-render, and Dictation aborts with the
          "ending dictation" chime. See PRD: .k2so/prds/voice-dictation.md. */}
      <textarea
        ref={shadowInputRef}
        autoComplete="off"
        autoCorrect="off"
        autoCapitalize="off"
        spellCheck={false}
        data-1p-ignore="true"
        data-k2so-shadow-input=""
        aria-label="Terminal input"
        aria-multiline="false"
        style={shadowInputStyle}
      />
      {visibleRows.map((row, rowIdx) => {
        const absRow = visibleRowAbsRows[rowIdx] ?? rowIdx
        return (
          <div key={`abs-${absRow}`}>
            {renderRowRuns(row, absRow, defaultFgCss, defaultBgCss)}
          </div>
        )
      })}
      {cursorOverlay && (
        <div aria-hidden="true" style={cursorOverlay.style}>
          {cursorOverlay.char ?? ''}
        </div>
      )}
      {/* 0.37.9 — composition overlay removed: text now streams
          straight into the PTY on each compositionupdate via
          backspace+retype, so the prompt itself shows the running
          transcript and the cursor advances naturally. */}
      {import.meta.env.DEV && (
        <div
          style={{
            position: 'absolute',
            top: 2,
            right: 2,
            padding: '2px 6px',
            background: 'rgba(0,0,0,0.8)',
            color: '#ff0',
            fontSize: '10px',
            fontFamily: 'monospace',
            zIndex: 999,
            pointerEvents: 'none',
            borderRadius: '3px',
          }}
        >
          <strong style={{ color: '#fff' }}>Alacritty</strong>
          {' '}· phase:{phase.kind}
          {' '}cells:{snapshot?.cols ?? '?'}x{snapshot?.rows ?? '?'}
          {' '}cursor:{snapshot?.cursor.col ?? 0},{snapshot?.cursor.row ?? 0}
          {' '}off:{viewportOffset}
          {' '}scr:{snapshot?.scrollback.length ?? 0}
          {' '}v:{snapshot?.version ?? 0}
          {!isReady && phase.kind !== 'idle' && ' · loading'}
        </div>
      )}
    </div>
  )
}
