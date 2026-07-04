// Kessel Tauri thin client.
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
// Deliberately kept small (< 450 lines). The Kessel-T0-era
// SessionStreamViewTerm (the retired JSON-stream beta) was ~600 lines because it held a local
// Term + byte reader + APC filter. None of that here.

import React, { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'

import { useKesselConfig } from '../kessel/config-context'
import { useIsTabVisible } from '@/contexts/TabVisibilityContext'
// [v2-perf] show_to_painted — dev-only workspace-switch t0 (projects.ts
// stamps it; no store import so kessel-term stays decoupled).
import { readWorkspaceSwitchMark } from '@/lib/ws-switch-mark'
import {
  computeDesiredActive,
  getLastSentActive,
  recordSentActive,
  shouldEmitResize,
  shouldHoldGridWs,
} from './activeViewer'
import {
  keyEventToSequence,
  naturalTextEditingSequence,
} from '@/lib/key-mapping'
import {
  createFrameCoalescer,
  type FrameCoalescer,
} from './frameCoalescer'
import { getDaemonWs, invalidateDaemonWs, daemonHttpBase, daemonWsBase, type DaemonWsAvailable } from '../kessel/daemon-ws'
import { isPossibleAuthFailure, reviveRemoteSession } from '@/lib/remote-session'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useTabsStore } from '@/stores/tabs'
import { useWindowFocusStore } from '@/stores/window-focus'
import { useSessionLabelsStore } from '@/stores/session-labels'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { detectWorkingSignal, GROK_PERMISSION_TITLE_RE } from '@/lib/agent-signals'
import {
  detectLinks,
  type DetectedLink,
} from '@/components/Terminal/terminalLinkDetector'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import {
  bracketPaste,
  isImagePath,
  quotePathForImageDrop,
} from '@/lib/file-drag'
import { useConnectHostStore } from '@/stores/connect-host'
import { executeRemoteDrop } from '@/lib/handle-remote-drop'
import {
  clampScrollPx,
  computeScrollbarThumb,
  computeStripLayout,
  scrollPxFromThumbTopFrac,
} from './scrollMath'
import { decodeGridFrame, type WireFrame } from './gridWire'
import { colToTextIndex, runColSpan } from './runCols'
import { hexToCss, TerminalRow } from './rowRender'
import { createWebglPainter } from './webgl/webglPainter'
import type { SelectionRange, TerminalPainter } from './webgl/painterTypes'
import {
  normalizeSelection,
  wordRangeAtCol,
  type SelectionPoint,
} from './webgl/selection'
import {
  cellChanged,
  encodeSgrMouse,
  mouseRoute,
  sgrButtonCode,
  type Cell,
} from './sgrMouse'
import {
  buildCopyText,
  copySelectionText,
  modelRowAt,
} from './copyText'
import { shouldApplyOsc52 } from './oscClipboard'
import { pickSeamColor } from './seamColor'

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
  /** Present (true) on a row's LAST run when the row soft-wraps into
   *  the next one. Daemons that predate the field never send it —
   *  the copy handler then treats every row as unwrapped, which
   *  matches the old behavior. */
  wrapped?: boolean
  /** Terminal-column span, present only when it differs from the
   *  run's char count (double-width CJK/emoji, zero-width combining
   *  chars). Drives (a) an explicit rendered width so grid alignment
   *  doesn't depend on the webfont's CJK advance and (b) pixel-col ↔
   *  text-offset mapping (runCols.ts). Absent ⇒ one column per char
   *  — daemons that predate the field behave exactly as before. */
  cols?: number
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
  /** True when the child app has any mouse-reporting mode active
   *  (?1000h / ?1002h / ?1003h). When set, the wheel handler must
   *  forward wheel ticks to the PTY as encoded mouse events instead
   *  of doing local-viewport scroll (TUIs on the alt screen have no
   *  scrollback to move). Only present on full snapshots — deltas
   *  carry it forward from the previous snapshot. */
  mouseReport?: boolean
  /** True when the child requested SGR extended mouse encoding
   *  (?1006h) → emit `\x1b[<…M` rather than legacy X10 `\x1b[M`. */
  sgrMouse?: boolean
  /** True when the child is on the alternate screen (?1049h / ?47h). */
  altScreen?: boolean
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

/** One snapshot/delta WS message queued for the next animation-frame
 *  flush. Applying frames per-rAF instead of per-message coalesces a
 *  burst (e.g. `cat` of a big file → hundreds of deltas) into one
 *  React render per display refresh. Legacy v1 had the same batching
 *  (`scheduleRender`); v2 dropped it and re-rendered per message. */
type PendingFrame =
  | { kind: 'snapshot'; payload: TermGridSnapshot }
  | { kind: 'delta'; payload: TermGridDelta }

type OutboundMsg =
  | { event: 'snapshot'; payload: TermGridSnapshot }
  | { event: 'delta'; payload: TermGridDelta }
  | { event: 'child_exit'; payload: { exit_code: number | null } }
  | { event: 'title'; payload: { title: string } }
  | { event: 'bell'; payload: null }
  // OSC 52 clipboard STORE from the child app, base64-decoded and
  // size-capped daemon-side. Broadcast to every viewer; each pane
  // applies it locally only while it is the active viewer.
  | { event: 'clipboard'; payload: { text: string } }
  | { event: 'error'; payload: { message: string } }
  // 0.37.4 Phase B — daemon-owned label events.
  | { event: 'label_initial'; payload: { label: string } }
  | { event: 'label_changed'; payload: { label: string } }

// ── Helpers ───────────────────────────────────────────────────────
// (hexToCss / runStyle / renderRowRuns / TerminalRow moved to
// rowRender.tsx — the column-anchored row renderer — so the row
// contract is unit-testable without the pane's I/O mocks.)

/** Join all run text in a row into a single plain string. Used
 *  for link detection (which operates on raw text). */
function rowToText(row: CellRun[]): string {
  let out = ''
  for (const run of row) out += run.text
  return out
}

/** Nearest enclosing rendered-row element for a DOM node inside the
 *  grid, or null when the node isn't inside a row (e.g. the
 *  selection boundary sits on the pane container). */
function rowDivFor(node: Node | null): HTMLElement | null {
  if (!node) return null
  const el =
    node.nodeType === Node.ELEMENT_NODE
      ? (node as Element)
      : node.parentElement
  return (el?.closest('[data-abs-row]') as HTMLElement | null) ?? null
}

/** Text-column offset of a DOM boundary (node, offset) measured from
 *  the start of a row div. Range.toString() walks the row's text
 *  nodes for us, so this stays correct however the runs are split. */
function colWithin(rowDiv: Element, node: Node, offset: number): number {
  const r = document.createRange()
  r.selectNodeContents(rowDiv)
  try {
    r.setEnd(node, offset)
  } catch {
    return 0
  }
  return r.toString().length
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
    // Mouse-mode bits are sticky state the daemon only re-sends on
    // full snapshots; carry the last-known values forward so the
    // wheel handler keeps routing correctly across delta ticks.
    mouseReport: prev.mouseReport,
    sgrMouse: prev.sgrMouse,
    altScreen: prev.altScreen,
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
  /** 2026-07-03 lazy-spawn gate — the CLI/daemon session id the client
   *  knows backs this tab (creation-stamped for chat-history restores,
   *  reconcile-stamped from the daemon's live-session list). NOT sent
   *  on the spawn POST (the daemon resolves resume state by
   *  agent_name); its presence marks the tab as backed by real work,
   *  so it keeps today's EAGER spawn-on-mount and stays warm while
   *  hidden. Absent + no `command` + hidden ⇒ the spawn POST defers
   *  until the pane is first visible. */
  sessionId?: string
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
  /** D9 — sandbox REQUEST intent. When true, the spawn POST carries
   *  `sandbox: true` so the daemon resolves a sandbox backend; the
   *  resolved backend name echoes back in the response and is stamped
   *  onto the tab (drives the orange marker). Omitted from the request
   *  when falsy → byte-identical to today for every normal tab. */
  sandbox?: boolean
  /** K2 #682 — fired when the daemon reports the child process
   *  exited (`child_exit`). Carries the exit code so the consumer can
   *  distinguish a clean quit from a crash, and is the signal a
   *  spawn-loop circuit breaker counts. The pinned Chat tab
   *  (`AgentChatPane`) uses this to detect RAPID REPEATED early exits
   *  (e.g. `claude --session-id <dup>` → "already in use" → exit 1) and
   *  STOP auto-respawning instead of piling up `claude` processes.
   *  Optional — most consumers don't need it. */
  onChildExit?: (exitCode: number | null) => void
  /** Pinned-chat background retention — hold the grid-WS while HIDDEN.
   *  Set only by the pinned canonical Chat pane (AgentChatPane,
   *  daemon-owned path) when its workspace is in the Active section, so
   *  switching back paints from live in-memory state instead of
   *  re-paying spawn POST + WS handshake + full-snapshot transfer.
   *  Threads into `shouldHoldGridWs` (activeViewer.ts) at the three
   *  gates that consult it: the grid-WS lifecycle effect, the
   *  `ws.onclose` reconnect gate, and `openGridWs`'s mid-handshake
   *  staleness check. Never affects the active-viewer claim — a hidden
   *  retained pane still sends `set_active:false` and the daemon
   *  ignores its resizes. Omitted/false ⇒ byte-identical park-on-hidden
   *  behavior for every other consumer. */
  retainWhileHidden?: boolean
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
    sessionId: knownSessionId,
    seedLabel,
    lockLabel,
    sandbox,
    retainWhileHidden,
  } = props

  // Live-subscribe to the terminal settings store so Cmd+Shift+=
  // / Cmd+Shift+- menu events (wired via listen('terminal:zoom-*')
  // in terminal-settings.ts) update this component's font size
  // immediately. Prop takes precedence for tests / ad-hoc consumers
  // that want to override.
  const storeFontSize = useTerminalSettingsStore((s) => s.fontSize)
  const fontSize = props.fontSize ?? storeFontSize
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)

  // ── WebGL painter flag ────────────────────────────────────────
  // Read ONCE at mount (the same affects-new-panes contract as
  // `renderer`): a store change mid-session never rebuilds a live
  // pane. `painterFatal` demotes THIS pane instance to the DOM strip
  // permanently — missing WebGL2, failed sanity readback, or an
  // unrestored context loss all land there. The DOM path below is
  // byte-identical when the flag is 'dom' (the default).
  const storePainter = useTerminalSettingsStore((s) => s.painter)
  const [painterKind] = useState(storePainter)
  const [painterFatal, setPainterFatal] = useState<string | null>(null)
  const useWebgl = painterKind === 'webgl' && painterFatal === null
  // Canvas-selection model (webgl only). ALWAYS null in DOM mode, so
  // the shared copy/key handlers can gate on it with zero DOM-path
  // impact. Version state re-fires the painter render effect; the
  // model itself lives outside React (per-mousemove updates).
  const webglSelectionRef = useRef<SelectionRange | null>(null)
  const [selectionVersion, setSelectionVersion] = useState(0)

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
  // Latest snapshot as a ref, for handlers that need the current
  // grid without re-binding on every frame (wheel listener, copy
  // handler). Same mirror pattern as `phaseRef`.
  const snapshotRef = useRef<TermGridSnapshot | null>(null)
  useEffect(() => {
    snapshotRef.current = snapshot
  }, [snapshot])
  // Authoritative merged grid, owned by applyFrameBatch (the sole
  // frame applier). Rendered `snapshot` mirrors it except while a
  // resize hold parks a blank intermediate here — see the resize-hold
  // block below.
  const liveGridRef = useRef<TermGridSnapshot | null>(null)
  // Scroll position in PIXELS above the bottom of the buffer. 0 =
  // pinned to the live grid (the heavy-output path — every input
  // handler snaps here); max = scrollback.length * cellHeight. Pixel
  // precision (not whole lines) is what lets the row strip translate
  // sub-row instead of jumping in cellHeight quanta; all row-unit
  // consumers derive their window via `computeStripLayout`.
  const [scrollPx, setScrollPx] = useState(0)
  // Current scroll position for handlers that must read it without
  // re-binding per frame (scrollbar drag). Same mirror pattern as
  // `phaseRef` / `snapshotRef`.
  const scrollPxRef = useRef(0)
  useEffect(() => {
    scrollPxRef.current = scrollPx
  }, [scrollPx])

  // ── rAF frame coalescing ──────────────────────────────────────
  // WS snapshot/delta messages queue here and apply once per
  // animation frame (one setSnapshot per display refresh, however
  // many messages arrived). A queued full snapshot supersedes
  // everything before it. The size cap flushes synchronously if rAF
  // is starved (occluded window) so the queue can't grow unbounded.
  // DEV-only FPS instrumentation. Counters live in refs and the badge
  // span updates imperatively (textContent) so measuring can never
  // cause renders and inflate its own numbers. ui = component commits
  // per second (scroll + frames + everything), grid = WS frames
  // applied per second, scroll = wheel flushes per second.
  const commitCountRef = useRef(0)
  const framesAppliedRef = useRef(0)
  const scrollFlushCountRef = useRef(0)
  const fpsSpanRef = useRef<HTMLSpanElement | null>(null)
  useEffect(() => {
    if (import.meta.env.DEV) commitCountRef.current++
  })
  useEffect(() => {
    if (!import.meta.env.DEV) return
    let last = performance.now()
    const id = setInterval(() => {
      const now = performance.now()
      const dt = (now - last) / 1000
      last = now
      if (dt <= 0) return
      const ui = Math.round(commitCountRef.current / dt)
      const grid = Math.round(framesAppliedRef.current / dt)
      const scroll = Math.round(scrollFlushCountRef.current / dt)
      commitCountRef.current = 0
      framesAppliedRef.current = 0
      scrollFlushCountRef.current = 0
      const el = fpsSpanRef.current
      if (el) el.textContent = ` · fps ui:${ui} grid:${grid} scroll:${scroll}`
    }, 500)
    return () => clearInterval(id)
  }, [])

  // Applies one drained batch. The queue/flush POLICY (per-rAF
  // coalescing, snapshot-supersedes, 60-frame starvation cap) lives
  // in createFrameCoalescer — pure, pinned by frameCoalescer.test.ts;
  // this callback is the injected applier: called exactly once per
  // flush, so everything below runs once per display refresh no
  // matter how many WS messages arrived.
  const applyFrameBatch = useCallback((pending: PendingFrame[]) => {
    if (import.meta.env.DEV) framesAppliedRef.current += pending.length
    // A full snapshot replace starts a new grid generation — absolute
    // row coords no longer map, so the canvas selection clears
    // (webgl painter only; the ref is always null in DOM mode).
    if (
      webglSelectionRef.current &&
      pending.some((f) => f.kind === 'snapshot')
    ) {
      webglSelectionRef.current = null
      setSelectionVersion((v) => v + 1)
    }
    // Single-writer grid model: `liveGridRef` is the authoritative
    // merged grid (every frame applies here, always); the rendered
    // `snapshot` state normally mirrors it. The two diverge in
    // exactly one case — the resize hold below — so merges never run
    // inside the state updater (a StrictMode double-invoke would
    // re-apply deltas onto an already-merged base).
    let next: TermGridSnapshot | null = liveGridRef.current
    for (const f of pending) {
      next = f.kind === 'snapshot' ? f.payload : mergeDelta(next, f.payload)
    }
    liveGridRef.current = next
    // Resize hold, content half: a resize's clear-then-repaint gap can
    // arrive as a BLANK grid (an old daemon broadcasts the cleared
    // intermediate; a new daemon's settle timeout can still fire
    // before a slow child repaints). Painting it would black-flash —
    // so while a resize we sent is in flight, a blank merge result
    // stays off-screen: the last non-blank frame keeps rendering
    // (stretched by the hold-and-scale layout) and the true grid
    // advances in `liveGridRef` until it has content again or the
    // hold times out (which promotes whatever is live).
    const rendered = snapshotRef.current
    if (
      resizeHoldActiveRef.current &&
      rendered &&
      !isGridEmpty(rendered) &&
      next &&
      isGridEmpty(next)
    ) {
      // Keep showing `rendered`; nothing to commit this flush.
    } else {
      setSnapshot(next)
    }
    // k1 flow control: one ack per APPLIED batch, carrying the
    // highest applied version — sent from the rAF flush, never per
    // WS message, so ack volume tracks render cadence. The daemon
    // uses it to bound this connection's unacked backlog; while we
    // fall behind it stops forwarding deltas and resyncs us with a
    // fresh full snapshot on our next ack. Gated on the daemon
    // actually speaking k1 (see k1WireActiveRef).
    if (k1WireActiveRef.current) {
      let maxVersion = 0
      for (const f of pending) {
        if (f.payload.version > maxVersion) maxVersion = f.payload.version
      }
      const ws = wsRef.current
      if (maxVersion > 0 && ws && ws.readyState === WebSocket.OPEN) {
        ws.send(JSON.stringify({ action: 'ack', version: maxVersion }))
      }
    }
  }, [])
  // The coalescer instance: rAF is the scheduler, applyFrameBatch the
  // applier. Ref-initialized once per mount (StrictMode double-render
  // safe via the null guard); applyFrameBatch has empty deps so the
  // captured reference never goes stale.
  const frameCoalescerRef = useRef<FrameCoalescer<PendingFrame> | null>(null)
  if (frameCoalescerRef.current === null) {
    frameCoalescerRef.current = createFrameCoalescer<PendingFrame>({
      schedule: (flush) => requestAnimationFrame(flush),
      cancel: (id) => cancelAnimationFrame(id),
      apply: applyFrameBatch,
    })
  }
  const enqueueFrame = useCallback((frame: PendingFrame) => {
    frameCoalescerRef.current?.enqueue(frame)
  }, [])
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
  // True once the CURRENT socket has delivered a binary (k1) frame —
  // i.e. the daemon honored our `&proto=k1` opt-in. Gates the ack
  // sends from the rAF flush: an older JSON-only daemon never sees
  // acks it would just log as malformed inbound. Reset on every
  // (re)connect; the daemon's per-connection pacing state resets with
  // the socket too.
  const k1WireActiveRef = useRef(false)
  // Last OSC 52 payload this pane actually wrote to the OS clipboard.
  // The `clipboard` WS event dedupes against it (claude re-emits the
  // same OSC 52 on every repaint while a selection stays live), and
  // only an APPLIED payload updates it — a frame skipped for being
  // non-active must not poison the baseline.
  const lastOsc52AppliedRef = useRef<string | null>(null)
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
  // F4 — the moment this pane is visible AND its window has OS focus, the
  // user has "seen" it: clear any unseen-done mark (extinguishes the
  // Active-bar amber dot). getState/subscribe (not a hook selector) so a
  // focus flip clears the mark without re-rendering the pane — same
  // discipline as the resize-gating subscriber below.
  useEffect(() => {
    if (!isTabVisible) return
    const maybeMarkSeen = (): void => {
      if (!useWindowFocusStore.getState().isFocused) return
      useActiveAgentsStore.getState().markSeen(terminalId)
    }
    maybeMarkSeen()
    return useWindowFocusStore.subscribe(maybeMarkSeen)
  }, [isTabVisible, terminalId])
  // Pinned-chat retention — same ref-mirror pattern: `ws.onclose` and
  // `openGridWs.isStale` (both bound inside long-lived closures) must
  // read the LATEST retention flag without re-subscribing the socket.
  const retainWhileHiddenRef = useRef(false)
  useEffect(() => {
    retainWhileHiddenRef.current = retainWhileHidden === true
  }, [retainWhileHidden])

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

  // [v2-perf] stage=show_to_painted (dev-only) — the pinned-chat
  // retention win metric. Measures a hidden→visible transition to the
  // next painted frame, plus end-to-end since the last workspace-switch
  // t0 when one is fresh (projects.ts stamps it via ws-switch-mark). A
  // retained pane reports sub-frame numbers (portal move + one commit);
  // a parked pane reports the full spawn/WS/snapshot reattach chain.
  // Initial mount is excluded — the existing first_render stages cover it.
  const prevShowRef = useRef<boolean | null>(null)
  useEffect(() => {
    const was = prevShowRef.current
    prevShowRef.current = isTabVisible
    // No import.meta.env gate here (a NEW occurrence would add to the
    // baseline tsc errors — untypeable under the web tsconfig); perfLog
    // no-ops in prod, leaving only one idle rAF per show transition.
    if (was !== false || !isTabVisible) return
    const t0 = performance.now()
    const switchT0 = readWorkspaceSwitchMark()
    requestAnimationFrame(() => {
      const painted = performance.now()
      perfLog('show_to_painted', {
        paint_ms: (painted - t0).toFixed(1),
        ...(switchT0 !== null
          ? { since_switch_ms: (painted - switchT0).toFixed(1) }
          : {}),
      })
    })
  }, [isTabVisible, perfLog])

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
    return () => {
      clearInterval(interval)
      // Activity-detection study, FM1: this watcher (plus the title/bell
      // handlers) is the only thing that ever writes working=false for
      // this pane — and it dies here. A pane unmounting mid-work
      // (workspace switch, tab close, retainer eviction) would strand
      // its 'working' entry as a forever-spinner. The pane can no longer
      // observe the session, so idle is the only honest value to leave
      // behind; if the session is genuinely still working, a re-mount
      // (or a retained pane's next frame) re-arms within a second.
      // Never clobbers 'permission'/'review' (recordTitleActivity
      // guards those).
      useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
    }
  }, [terminalId])

  // ── Lazy-spawn gate (2026-07-03: workspace-switch latency) ────
  //
  // A workspace mount renders EVERY saved tab's pane (retained-view
  // model — mounting hidden for instant tab-switch is fine), and
  // pre-fix each pane fired its spawn POST immediately: a restored
  // layout with N never-attached bare tabs cost N sequential
  // round-trips per workspace entry (each one refused by the daemon's
  // bare-tab cap anyway — pure latency, zero value). A restored bare
  // tab now spawns LAZILY: the POST defers until first visible, one
  // on-demand round-trip when the user actually switches to it.
  //
  // MUST-STAY-EAGER — for these, spawning while hidden is the point:
  //   - `command`         — a real program must start running
  //                         unwatched (BackgroundTerminalSpawner's
  //                         heartbeat spawns, AgentPane/AgentChatPane
  //                         fallback claude launches, AIFileEditor);
  //   - `sessionId`       — the client KNOWS real work backs this tab
  //                         (creation- or reconcile-stamped): keep it
  //                         warm so switching to it never pays the
  //                         spawn round-trip;
  //   - `attachAgentName` — attaches to an EXISTING daemon session
  //                         (surfaced heartbeat tabs, the pinned
  //                         canonical chat — PinnedChatRetainer's
  //                         retention presupposes the attach);
  //   - `sandbox`         — cell provisioning is deliberately not
  //                         deferred behind first view;
  //   - `retainWhileHidden` — holds the grid-WS while hidden, which
  //                         presupposes a spawned session.
  //
  // Arming is ONE-WAY (false → true, exactly once), so the 0.39.13
  // stable-deps guarantee below survives: later visibility flips
  // never tear down or re-issue the spawn.
  const spawnEager = Boolean(
    command || knownSessionId || attachAgentName || sandbox || retainWhileHidden,
  )
  const [spawnArmed, setSpawnArmed] = useState(() => spawnEager || isTabVisible)
  useEffect(() => {
    // Arms on first-visible — and also when a hidden pane's props later
    // gain an eager marker (e.g. reconcile stamps a live session's
    // command/sessionId), so "session survives, tab reattaches" keeps
    // working without the tab ever being viewed.
    if (!spawnArmed && (spawnEager || isTabVisible)) setSpawnArmed(true)
  }, [spawnArmed, spawnEager, isTabVisible])

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
    // Lazy-spawn gate: an unarmed pane skips the WHOLE effect body —
    // no POST, no daemon side effects, phase stays 'idle'. The arming
    // flip re-runs this effect exactly once (see spawnArmed above).
    if (!spawnArmed) return
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
        // D9 — only emit the sandbox key when the tab requested it, so
        // the request/response stay byte-identical to today for every
        // normal (non-sandbox) tab. Default-OFF.
        sandbox: sandbox ? true : undefined,
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
        // D9 — resolved sandbox backend, echoed ONLY when the caller
        // asked for sandbox. 'microvm' | 'passthrough' | undefined.
        sandbox?: string
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
            const body = await spawnRes.text()
            // Stale remote session: a remote daemon restart wiped its
            // in-memory connect-sessions, so this spawn was rejected with the
            // token-gate 401/403 — not a bad request. Revive (single-flight
            // re-login with the remembered password) and retry with fresh
            // creds; bounded because a second rejection finds the session
            // 'still-valid' (or in cooldown) and falls through to the error.
            if (isPossibleAuthFailure(spawnRes.status, body)) {
              const active = useConnectHostStore.getState().activeHost
              if (active !== 'local') {
                const outcome = await reviveRemoteSession(active.id)
                if (cancelled) return
                if (outcome === 'revived') {
                  creds = null // re-resolve so the retry carries the NEW token
                  continue
                }
              }
            }
            // 4xx — genuine request error, surface immediately. Bad
            // body, missing field, etc. Won't get better by waiting.
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

      // D9 — stamp the resolved sandbox backend onto the tab. The
      // daemon echoes `sandbox` ONLY when this pane asked for it
      // (gated `if sandbox_echo.is_some()` server-side), and the echo
      // fires on BOTH the fresh and reuse branches — so this runs for
      // every successful spawn/attach. Read through the same `as` cast
      // used for sessionId above (the `spawn` local is typed `never`
      // here, a pre-existing quirk). Normal tabs never request sandbox
      // ⇒ `sandbox` is undefined ⇒ the marker stays off. Truthful: a
      // degraded passthrough stamps 'passthrough', which renders no
      // orange (TabBar gates strictly on === 'microvm').
      useTabsStore.getState().setTerminalSandboxBackend(
        terminalId,
        (spawn as { sandbox?: string }).sandbox,
      )

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
    // effect — not inline here. `spawnArmed` (2026-07-03) keeps the
    // stability contract: it flips false→true exactly once (the lazy
    // spawn's deferred first run), never back.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [terminalId, cwd, command, args?.join('\0'), reconnectAttempt, spawnArmed])

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
    // for a pane that no longer wants to stream. A hidden-but-RETAINED
    // pane (pinned-chat retention) still wants the stream — the retain
    // ref keeps a mid-handshake connect alive across a hide.
    const isStale = () =>
      sessionIdRef.current !== sessionId ||
      (appliedVisibleRef.current !== true && !retainWhileHiddenRef.current)

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
        // `proto=k1` opts into the binary grid wire (gridWire.ts).
        // An older daemon ignores the param and keeps sending JSON
        // text frames — both message paths below stay live.
        const candidate = new WebSocket(
          `${daemonWsBase(creds)}/cli/sessions/grid?session=${sessionId}&token=${creds.token}&proto=k1`,
        )
        candidate.binaryType = 'arraybuffer'
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
      // Fresh socket — the daemon's per-connection pacing state is
      // new too, so re-detect k1 from its first binary frame before
      // resuming acks.
      k1WireActiveRef.current = false
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
      // Fresh daemon subscriber ⇒ we hold no claim until the re-prime
      // below lands one; render passively meanwhile.
      setIsActiveViewer(false)
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

      // Snapshot arrival handling shared by the JSON text path and
      // the k1 binary path — the decoded k1 payload is the exact
      // object shape JSON.parse yields (pinned by gridWire.test.ts),
      // so everything downstream is transport-blind.
      const applySnapshotFrame = (payload: TermGridSnapshot) => {
        const isFirst = !firstSnapshotSeenRef.current
        if (isFirst) {
          firstSnapshotSeenRef.current = true
          const empty = isGridEmpty(payload)
          firstSnapshotEmptyRef.current = empty
          perfLog('first_snapshot', {
            rows: payload.rows,
            cols: payload.cols,
            empty: String(empty),
            scrollback: payload.scrollback.length,
          })
        }
        enqueueFrame({ kind: 'snapshot', payload })
        // Activity detection runs in a snapshot-driven useEffect
        // below, NOT inline here. ws.onmessage is captured in the
        // boot effect's closure and does not re-bind across Vite
        // HMR / React Fast Refresh — calling activity from here
        // means HMR'd code wouldn't take effect on existing
        // sessions. Driving it from setSnapshot's downstream
        // effect avoids that whole class of bug.
        //
        // Functional update returning the SAME object when phase
        // is already ready — full snapshots recur (any full-
        // damage frame), and rebuilding the phase object each
        // time forced a redundant re-render per snapshot.
        setPhase((prev) =>
          prev.kind === 'ready' && prev.sessionId === sessionId
            ? prev
            : { kind: 'ready', sessionId },
        )
      }

      ws.onmessage = (evt) => {
        if (evt.data instanceof ArrayBuffer) {
          // k1 binary wire — snapshot/delta only; every other event
          // still arrives as JSON text below.
          let frame: WireFrame
          try {
            frame = decodeGridFrame(evt.data)
          } catch (e) {
            // A frame that fails to decode is a protocol violation —
            // surface it rather than silently desyncing the mirror.
            // eslint-disable-next-line no-console
            console.error('[kessel-term] k1 frame decode failed:', e)
            return
          }
          k1WireActiveRef.current = true
          if (frame.kind === 'snapshot') {
            applySnapshotFrame(frame.payload)
          } else {
            enqueueFrame({ kind: 'delta', payload: frame.payload })
          }
          return
        }
        if (typeof evt.data !== 'string') return
        let parsed: OutboundMsg
        try {
          parsed = JSON.parse(evt.data) as OutboundMsg
        } catch {
          return
        }
        switch (parsed.event) {
          case 'snapshot':
            applySnapshotFrame(parsed.payload)
            break
          case 'delta':
            enqueueFrame({ kind: 'delta', payload: parsed.payload })
            break
          case 'title': {
            // Mirror legacy's `terminal:title:<id>` handling. Claude
            // Code uses braille-spinner glyphs in the title prefix
            // while working and the ✱-family glyphs the moment it
            // goes idle, so the title is the fastest, most reliable
            // working/idle hint we have. See
            // AlacrittyTerminalView.tsx:510-518 for the legacy
            // version. We use the SAME regex so v2 and legacy agree.
            //
            // Per-agent title inventory (2026-07 TUI signal studies):
            //  - claude: braille prefix while working (working rule);
            //    ✱-family prefix on idle (idle rule).
            //  - grok:   `⠙ - Thinking - grok` while working (the same
            //    braille working rule catches it); idle title carries
            //    NO glyph (`grok` / `<session title> - grok`), so idle
            //    comes from the 1s idle watcher; `⚠ Action Required - `
            //    prefix while a permission gate is open (dedicated
            //    rule below — grok has no lifecycle hooks, so the
            //    title is its ONLY permission source).
            //  - codex:  `⠋ <cwd-basename>` while working (the braille
            //    rule); idle is the bare basename (no glyph — the idle
            //    watcher clears).
            //  - cursor-agent: no busy title (flips to a conversation
            //    summary at turn completion) — phrase scan + cadence.
            //  - hermes: NO titles ever — phrase scan + output cadence
            //    only (the safe unknown-agent default).
            //  - gemini/pi: working-state titles unverified; both ride
            //    the phrase scan.
            const raw = parsed.payload.title ?? ''
            const isIdleMarker = /^[*✱✲✳✴✵✶✷✸✹⚹⁎∗※]/.test(raw)
            const isWorkingMarker = /^[\u2800-\u28FF]/.test(raw)
            const isGrokPermissionMarker = GROK_PERMISSION_TITLE_RE.test(raw)
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
            if (isGrokPermissionMarker) {
              // Grok permission gate open — drive the SAME 'permission'
              // pane state Claude's lifecycle hook drives (toast +
              // sidebar red). recordTitlePermission tracks the pane as
              // title-owned so the title can also CLEAR it below — a
              // hook-set permission (claude) is never clearable from
              // here.
              useActiveAgentsStore
                .getState()
                .recordTitlePermission(terminalId, true)
            } else {
              // Any non-⚠ title clears a TITLE-owned permission (the
              // grok gate resolved); no-op for every other pane —
              // including hook-owned permission states.
              useActiveAgentsStore
                .getState()
                .recordTitlePermission(terminalId, false)
              if (isIdleMarker) {
                lastSeenWorkingAtRef.current = 0
                useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
              } else if (isWorkingMarker) {
                lastSeenWorkingAtRef.current = Date.now()
                useActiveAgentsStore.getState().recordTitleActivity(terminalId, true)
              }
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
            // notifications. Claude rings the bell when it's done and
            // ready for input (empirically verified) — use it as a
            // definitive idle transition. Codex does NOT bell on its
            // default config (the 2026-07 TUI study disproved the old
            // "Claude / Codex" claim: zero BELs, including turn-ends
            // with focus lost). No studied agent (grok/hermes/cursor/
            // gemini/pi) bells spuriously either, so bell→idle stays
            // safe without per-agent gating.
            if (import.meta.env.DEV) {
              // eslint-disable-next-line no-console
              console.warn(`[v2-activity] BELL tid=${terminalId.slice(0, 8)}`)
            }
            lastSeenWorkingAtRef.current = 0
            useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
            break
          }
          case 'clipboard': {
            // OSC 52 copy from the child app (payload decoded and
            // size-capped daemon-side; read-back is never
            // implemented). Apply to the OS clipboard only while
            // this pane holds the ACTIVE-viewer claim — wezterm's
            // attached-client model rather than tmux's stomp-every-
            // client, so a passive second window / phone viewing the
            // same session never has its clipboard replaced. Dedupe
            // against the last APPLIED value via shouldApplyOsc52.
            const text = parsed.payload.text ?? ''
            if (lastSentActiveRef.current !== true) break
            if (!shouldApplyOsc52(lastOsc52AppliedRef.current, text)) break
            lastOsc52AppliedRef.current = text
            navigator.clipboard.writeText(text).catch((err) =>
              console.warn('[kessel-term/osc52] clipboard write failed:', err),
            )
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
        //     reconnecting a stream nobody watches is the #8 pile-up)
        //     — UNLESS this is a retained pinned chat (retention exists
        //     precisely to keep the hidden stream warm, so a genuine
        //     mid-flight drop on a retained hidden pane reconnects).
        if (!shouldHoldGridWs({
          visible: tabVisibleRef.current,
          exited: phaseRef.current.kind === 'exited',
          retainWhileHidden: retainWhileHiddenRef.current,
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
  }, [terminalId, perfLog, reconnectAttempt, enqueueFrame])

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
    // the live phase ref so an exited pane never (re)opens. A retained
    // pinned chat (retainWhileHidden) streams while hidden; a retention
    // flip (workspace left Active) re-runs this effect and closes the
    // socket exactly like a hide.
    const wantOpen =
      haveSession &&
      shouldHoldGridWs({
        visible,
        exited: phaseRef.current.kind === 'exited',
        retainWhileHidden,
      })

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
  }, [spawnGeneration, isTabVisible, retainWhileHidden])

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
      frameCoalescerRef.current?.clear()
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

  // ── Passive scale-to-fit state ────────────────────────────────
  // Whether THIS pane last told the daemon it is the active viewer.
  // React-state mirror of `lastSentActiveRef` (declared with the
  // set_active effect below) so the scale layout re-derives when the
  // claim changes — the ref alone wouldn't re-render. `false` when
  // the claim state is unknown (fresh socket): an unclaimed pane is
  // treated as passive until its claim goes out.
  const [isActiveViewer, setIsActiveViewer] = useState(false)
  // Container content size in px, updated by the ResizeObserver on
  // EVERY fire (no debounce — the scale must track the box live even
  // though PTY resizes are debounced). 0×0 until first measure.
  const [containerSize, setContainerSize] = useState({ width: 0, height: 0 })

  // ── Device pixel ratio ─────────────────────────────────────────
  // Tracks monitor moves / OS zoom. matchMedia's resolution query
  // fires `change` when devicePixelRatio departs the queried value;
  // re-arm against the new value each time. Drives the WebGL
  // painter's quantized cell metrics + atlas rebuild, AND the DOM
  // painter's synthetic box/block glyph geometry (rowRender rounds
  // strokes to device pixels), so it is tracked for both painters.
  const [dpr, setDpr] = useState(() =>
    typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1,
  )
  useEffect(() => {
    // jsdom (tests) has no matchMedia; dpr just stays at its mount
    // value there, same as any environment without the API.
    if (typeof window.matchMedia !== 'function') return
    const mq = window.matchMedia(`(resolution: ${dpr}dppx)`)
    const onChange = () => setDpr(window.devicePixelRatio || 1)
    mq.addEventListener('change', onChange)
    return () => mq.removeEventListener('change', onChange)
  }, [dpr])

  // ── Cell metrics (for cursor positioning + wheel math) ────────
  const [cellMetrics, setCellMetrics] = useState({ width: 0, height: 0 })
  // Ref mirror for once-bound handlers (canvas selection drag) —
  // same pattern as `snapshotRef`.
  const cellMetricsRef = useRef(cellMetrics)
  useEffect(() => {
    cellMetricsRef.current = cellMetrics
  }, [cellMetrics])
  useLayoutEffect(() => {
    const span = document.createElement('span')
    span.style.cssText = `font-family: ${config.font.family}; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    span.textContent = 'W'
    document.body.appendChild(span)
    const rect = span.getBoundingClientRect()
    document.body.removeChild(span)
    // WebGL painter: quantize the cell width to the device grid —
    // floor(css × dpr) / dpr — so EVERY cellMetrics consumer (cursor
    // overlay, shadow IME textarea, hit tests, resize col math)
    // shares the painter's exact device cell width (brief §1.3: the
    // painter must floor to integer device px; quantizing the shared
    // metric keeps the DOM overlays pixel-aligned with the canvas
    // instead of drifting sub-pixel-per-column). DOM path keeps the
    // fractional measurement byte-identically.
    const width = useWebgl ? Math.floor(rect.width * dpr) / dpr : rect.width
    setCellMetrics({
      width,
      height: Math.ceil(fontSize * config.font.lineHeightMultiplier),
    })
  }, [fontSize, config.font.family, config.font.lineHeightMultiplier, useWebgl, dpr])

  // ── WebGL painter lifecycle (useWebgl only) ───────────────────
  // The painter is a pure consumer downstream of the rAF coalescer:
  // it sees the same merged `snapshot` + `scrollPx` the DOM strip
  // renders, once per commit, and owns nothing else — WS, merge,
  // input, overlays all stay with the pane. All three effects are
  // layout effects in dependency order (create → metrics → render)
  // so the first snapshot paints in the same commit that mounts the
  // canvas.
  const webglCanvasRef = useRef<HTMLCanvasElement | null>(null)
  const painterRef = useRef<TerminalPainter | null>(null)
  const painterTheme = useMemo(
    () => ({
      fg: config.colors.foreground,
      bg: config.colors.background,
      selection: config.colors.selection.background,
    }),
    [
      config.colors.foreground,
      config.colors.background,
      config.colors.selection.background,
    ],
  )
  useLayoutEffect(() => {
    if (!useWebgl) return
    const canvas = webglCanvasRef.current
    if (!canvas) return
    const painter = createWebglPainter()
    painter.onFatal((reason) => {
      // Permanent per-pane demotion. The canvas unmounts and the DOM
      // strip (the proven path) takes over on the next render.
      setPainterFatal(reason)
    })
    painter.mount(canvas)
    painterRef.current = painter
    return () => {
      painterRef.current = null
      painter.dispose()
    }
  }, [useWebgl])
  useLayoutEffect(() => {
    if (!useWebgl) return
    const painter = painterRef.current
    if (!painter) return
    if (!cellMetrics.width || !cellMetrics.height) return
    painter.setMetrics({
      cssCellW: cellMetrics.width,
      cssCellH: cellMetrics.height,
      dpr,
      fontFamily: config.font.family,
      fontSize,
    })
  }, [useWebgl, cellMetrics, config.font.family, fontSize, dpr])
  useLayoutEffect(() => {
    if (!useWebgl) return
    const painter = painterRef.current
    if (!painter || !snapshot) return
    if (!cellMetrics.width || !cellMetrics.height) return
    painter.render({
      snapshot,
      scrollPx,
      selection: webglSelectionRef.current,
      theme: painterTheme,
    })
  }, [useWebgl, snapshot, scrollPx, painterTheme, cellMetrics, selectionVersion])

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
  // Dims most recently WRITTEN to the wire (vs `lastResizeRef` = most
  // recently measured). The two diverge only while hidden — sendResize
  // records without emitting — and the foreground catch-up effect
  // below flushes the difference exactly once on show.
  const lastSentResizeRef = useRef<{ cols: number; rows: number } | null>(
    null,
  )

  // ── Resize hold-and-scale bookkeeping (black-flash fix, client
  // half) ─────────────────────────────────────────────────────────
  // While a resize we sent is in flight — the container reshaped but
  // incoming frames still carry the OLD cols/rows — the scale layout
  // keeps rendering the last grid stretched/letterboxed to the new
  // box instead of drawing old-geometry content 1:1 (clipped or
  // undersized) and then flashing. Cleared the moment a frame with
  // the requested dims arrives, or by a hard timeout after which we
  // render whatever we have unscaled (the daemon may have coalesced
  // the request away).
  const RESIZE_HOLD_TIMEOUT_MS = 500
  const [pendingResize, setPendingResize] = useState<{
    cols: number
    rows: number
  } | null>(null)
  const pendingResizeTimerRef = useRef<ReturnType<typeof setTimeout> | null>(
    null,
  )
  // Mirror of `pendingResize !== null` for the frame-flush path (which
  // must not close over render state). While true, blank merge results
  // stay in `liveGridRef` instead of being rendered — see
  // applyFrameBatch.
  const resizeHoldActiveRef = useRef(false)
  const notePendingResize = useCallback((cols: number, rows: number) => {
    // Already at the target: the daemon's same-dims skip means no new
    // frame is coming — nothing to hold for.
    const snap = snapshotRef.current
    if (snap && snap.cols === cols && snap.rows === rows) return
    setPendingResize({ cols, rows })
    resizeHoldActiveRef.current = true
    if (pendingResizeTimerRef.current) {
      clearTimeout(pendingResizeTimerRef.current)
    }
    pendingResizeTimerRef.current = setTimeout(() => {
      pendingResizeTimerRef.current = null
      setPendingResize(null)
      resizeHoldActiveRef.current = false
      // Show the truth on expiry, blank or not — an indefinite hold
      // would freeze a terminal the child legitimately cleared.
      if (liveGridRef.current !== snapshotRef.current) {
        setSnapshot(liveGridRef.current)
      }
    }, RESIZE_HOLD_TIMEOUT_MS)
  }, [])
  // Release the hold as soon as a NON-BLANK frame at the requested
  // dims lands (dims alone aren't enough — the daemon's cleared-grid
  // intermediate arrives at the new dims too, and painting it is the
  // black flash this hold exists to prevent; blank frames park in
  // liveGridRef until content or the timeout).
  useEffect(() => {
    if (!pendingResize || !snapshot) return
    if (
      snapshot.cols === pendingResize.cols &&
      snapshot.rows === pendingResize.rows &&
      !isGridEmpty(snapshot)
    ) {
      if (pendingResizeTimerRef.current) {
        clearTimeout(pendingResizeTimerRef.current)
        pendingResizeTimerRef.current = null
      }
      setPendingResize(null)
      resizeHoldActiveRef.current = false
    }
  }, [snapshot, pendingResize])
  useEffect(
    () => () => {
      if (pendingResizeTimerRef.current) {
        clearTimeout(pendingResizeTimerRef.current)
        pendingResizeTimerRef.current = null
      }
    },
    [],
  )

  const sendResize = useCallback(
    (cols: number, rows: number) => {
      // Recorded unconditionally — these dims ride the next `set_active`
      // claim (the daemon snaps the PTY to them on foreground), so a
      // hidden pane's measurements must stay current even though it
      // emits no resize frames.
      lastResizeRef.current = { cols, rows }
      // Only the visible pane in the focused window emits (pinned-chat
      // retention: a background pane parked in the retainer's hidden
      // host sends NOTHING — after its own release the session is
      // unclaimed, and the daemon's first-resize-wins rule would ACCEPT
      // a background resize, reflowing the PTY to off-screen geometry:
      // the workspace-switch zoom bug).
      if (
        !shouldEmitResize({
          visible: tabVisibleRef.current,
          windowFocused: useWindowFocusStore.getState().isFocused,
        })
      ) {
        return
      }
      const ws = wsRef.current
      if (!ws || ws.readyState !== WebSocket.OPEN) return
      ws.send(JSON.stringify({ action: 'resize', cols, rows }))
      lastSentResizeRef.current = { cols, rows }
      notePendingResize(cols, rows)
    },
    [notePendingResize],
  )

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
        setIsActiveViewer(active)
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
      setIsActiveViewer(active)
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
            lastSentResizeRef.current = { ...lastResizeRef.current }
            // This resize is in flight like any other — hold-and-
            // scale until frames reflow to our geometry.
            notePendingResize(
              lastResizeRef.current.cols,
              lastResizeRef.current.rows,
            )
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

  // Foreground catch-up (pinned-chat retention): a hidden pane records
  // measurements without emitting (see sendResize), so if its box
  // changed while backgrounded (window resized behind another
  // workspace) the wire is stale the moment it shows again — and the
  // ResizeObserver won't re-fire because the mirrored hidden host
  // already matched the slot. Flush the recorded dims once on show.
  // With an unchanged window this is a no-op (recorded == sent): the
  // zero-resize foreground path the retention feature promises.
  useEffect(() => {
    if (!isTabVisible) return
    const measured = lastResizeRef.current
    if (!measured) return
    const sent = lastSentResizeRef.current
    if (sent && sent.cols === measured.cols && sent.rows === measured.rows) {
      return
    }
    sendResize(measured.cols, measured.rows)
  }, [isTabVisible, sendResize])

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
      // Canvas-selection copy (webgl painter): with no native
      // selection, WKWebView may never fire a `copy` event — catch
      // Cmd+C here while a model selection exists. The ref is always
      // null in DOM mode, so this adds nothing to that path.
      if (
        webglSelectionRef.current &&
        e.metaKey &&
        !e.ctrlKey &&
        !e.altKey &&
        (e.key === 'c' || e.key === 'C')
      ) {
        e.preventDefault()
        const snap = snapshotRef.current
        if (snap) {
          navigator.clipboard
            .writeText(copySelectionText(snap, webglSelectionRef.current))
            .catch((err) =>
              console.warn('[kessel-term/webgl] clipboard write failed:', err),
            )
        }
        return
      }
      const natural = naturalTextEditingSequence(e)
      if (natural !== null) {
        e.preventDefault()
        setScrollPx(0)
        sendInput(natural)
        // Clear so the textarea never accumulates.
        if (shadowInputRef.current) shadowInputRef.current.value = ''
        return
      }
      const seq = keyEventToSequence(e, 0)
      if (seq === null) return
      e.preventDefault()
      setScrollPx(0)
      sendInput(seq)
      if (shadowInputRef.current) shadowInputRef.current.value = ''
    }
    const onPaste = (e: ClipboardEvent) => {
      const text = e.clipboardData?.getData('text') ?? ''
      e.preventDefault()
      setScrollPx(0)

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
      setScrollPx(0)
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
        setScrollPx(0)
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

  // ── Compose the row strip ─────────────────────────────────────
  //
  // Declared before the link-detection handlers below because
  // `handleMouseMove` closes over `stripRows` and JS temporal-
  // dead-zone rules reject the closure at render time if the
  // `const` is declared later. (Same class of fix as the
  // cellMetrics hoist that happened earlier in the Kessel-T0
  // work.)
  //
  // Vertical quantum for every px↔row conversion in this component.
  // The 20px fallback (metrics not measured yet) matches the wheel
  // handler's; snapshot is normally still null at that point.
  const cellHeightPx = cellMetrics.height || 20
  // Where the strip sits for the current pixel scroll position:
  // which absolute row it starts at, how many rows it holds
  // (viewport + overscan, clamped at the buffer edges), and the
  // translateY that puts the sub-row fraction on screen. Recomputed
  // every scroll frame — but `stripStart`/`rowCount` only change on
  // a row-boundary crossing, so the row-slice memo below (and with
  // it every row element) stays cache-hit during sub-row scrolls.
  const stripLayout = useMemo(() => {
    const totalRows = snapshot
      ? snapshot.scrollback.length + snapshot.grid.length
      : 0
    return computeStripLayout(scrollPx, totalRows, snapshot?.rows ?? 0, cellHeightPx)
  }, [scrollPx, snapshot, cellHeightPx])
  // Strip rows + their absolute (scrollback-anchored) row indices.
  // Keying the rendered row divs by absolute index — instead of by
  // visual 0..N position — keeps the same DOM node attached to the
  // same logical row across scrolls. The browser's text selection is
  // anchored to text nodes inside those divs; if the divs survive
  // (just move position), native selection follows the content as
  // expected. Without this, scrolling reused row divs with new
  // content and the highlight visually "stayed" while text moved.
  // (The copy handler's `data-abs-row` mapping rides on the same
  // keys, so overscan rows resolve to their model rows too.)
  const { stripRows, stripAbsRows } = useMemo(() => {
    if (!snapshot) {
      return { stripRows: [] as CellRun[][], stripAbsRows: [] as number[] }
    }
    const { scrollback, grid } = snapshot
    const rows: CellRun[][] = []
    const abs: number[] = []
    for (let i = 0; i < stripLayout.rowCount; i++) {
      const a = stripLayout.stripStart + i
      abs.push(a)
      if (a < 0) rows.push([])
      else if (a < scrollback.length) rows.push(scrollback[a])
      else rows.push(grid[a - scrollback.length] ?? [])
    }
    return { stripRows: rows, stripAbsRows: abs }
  }, [snapshot, stripLayout.stripStart, stripLayout.rowCount])

  // iTerm-style "extend background color to edges": when the visible
  // strip's boundary cells share one explicit bg (fullscreen TUI),
  // the container paints it so the sub-cell remainder at the right/
  // bottom edges doesn't show the theme background as a seam. Null
  // (plain shell / no majority) keeps the theme. Recomputed per
  // committed frame but the RESULT is a primitive — downstream memos
  // (containerStyle) only rebuild when the picked color actually
  // changes, so a TUI steadily repainting its own bg causes zero
  // style churn. Applies to both painters: the WebGL canvas is sized
  // to exactly cols×rows cells, so the seam is this same container
  // showing through (the painter's clear color stays theme-bg — grid
  // cells with default bg ride it).
  const seamBg = useMemo(
    () => pickSeamColor(stripRows, snapshot?.cols),
    [stripRows, snapshot?.cols],
  )

  // ── Passive scale-to-fit (kessel-hard-learnings §2.7 / §Wave 3) ─
  //
  // When ANOTHER viewer owns the PTY size (this pane never claimed
  // active, or lost the claim), the grid can be bigger than our box.
  // The only lossless treatments of a width-committed grid are scale,
  // letterbox or clip — NEVER re-wrap (1:1 grid row → display row is
  // preserved: we scale the whole strip uniformly). Scale factor is
  // min(fitW, fitH, 1), centered/letterboxed, floored at 0.4 after
  // which we clip instead (unreadably small is worse than clipped).
  // An active pane renders 1:1 — its resizes drive the PTY, so any
  // mismatch is transient (the hold-and-scale path below covers it).
  const PASSIVE_SCALE_FLOOR = 0.4
  const snapCols = snapshot?.cols ?? 0
  const snapRows = snapshot?.rows ?? 0
  const scaleLayout = useMemo(() => {
    const identity = { scale: 1, offsetX: 0, offsetY: 0, passive: false }
    const cw = cellMetrics.width
    const ch = cellMetrics.height
    if (!snapCols || !snapRows || !cw || !ch) return identity
    // Same available-box formula as the ResizeObserver's cols/rows
    // fit, so a grid sized to THIS pane always computes fit ≥ 1 and
    // renders unscaled. Width subtracts only the LEFT 4px pad — the
    // right edge is padding-free so the column math gets that width
    // (the centered remainder supplies the visual right breathing room).
    const availW = Math.max(0, containerSize.width - 4)
    const availH = Math.max(0, containerSize.height - 4)
    if (!availW || !availH) return identity
    const gridW = snapCols * cw
    const gridH = snapRows * ch
    const fit = Math.min(availW / gridW, availH / gridH)
    const letterboxed = (scale: number, passive: boolean) => ({
      scale,
      offsetX: Math.max(0, (availW - gridW * scale) / 2),
      offsetY: Math.max(0, (availH - gridH * scale) / 2),
      passive,
    })
    // Unscaled (scale-1) rendering centers the sub-cell quantization
    // remainder: cols = floor(availW / cellW) leaves up to one cell
    // of slack, and anchoring the grid at the left padding parked ALL
    // of it on the right — users read that as a bigger right gap than
    // left. Splitting it (floored to whole px so scale-1 text stays
    // on the pixel grid) makes the gutters symmetric. These offsets
    // ARE the content origin: the scale wrapper translates the DOM
    // strip AND the WebGL canvas by them, and toGridXY subtracts them
    // for every pointer→cell mapping (selection, link hover, SGR
    // forwarding), so grid pixels and hit-testing move together. The
    // cursor overlay + IME shadow textarea add them explicitly via
    // contentOriginX/Y below. The remainder gutter shows the
    // container's own background (same fill as the 4px padding), so
    // no seam appears on either side.
    const centered = {
      scale: 1,
      offsetX: Math.floor(Math.max(0, availW - gridW) / 2),
      offsetY: Math.floor(Math.max(0, availH - gridH) / 2),
      passive: false,
    }
    if (!isActiveViewer) {
      if (fit >= 1) return centered
      return letterboxed(Math.max(fit, PASSIVE_SCALE_FLOOR), true)
    }
    // Active pane, resize in flight (hold-and-scale): frames still
    // carry the OLD geometry — stretch the last grid to the new box
    // (scale may exceed 1 when the box grew) until the first frame at
    // the requested dims lands or the hold times out. This is what
    // turns the container-resize window from a flash into a smooth
    // reflow.
    if (
      pendingResize &&
      (snapCols !== pendingResize.cols || snapRows !== pendingResize.rows)
    ) {
      return letterboxed(Math.max(fit, PASSIVE_SCALE_FLOOR), false)
    }
    return centered
  }, [
    snapCols,
    snapRows,
    cellMetrics.width,
    cellMetrics.height,
    containerSize.width,
    containerSize.height,
    isActiveViewer,
    pendingResize,
  ])
  // Ref mirror for handlers that bind once (wheel listener) — same
  // pattern as `snapshotRef`.
  const scaleLayoutRef = useRef(scaleLayout)
  useEffect(() => {
    scaleLayoutRef.current = scaleLayout
  }, [scaleLayout])

  // Portal-move settle guard (pinned-chat retention): the scale
  // wrapper's 120ms transform transition exists for GENUINE runtime
  // scale changes (claim takeover, resize hold-and-scale). A retained
  // pane being re-parented between the hidden host and a slot must not
  // animate whatever one-frame settle follows the move — it reads as a
  // zoom on every workspace switch. Disarmed while hidden and for the
  // first frames after a show; re-armed on the second rAF, after the
  // ResizeObserver's post-move measurement has committed and painted.
  const [scaleTransitionArmed, setScaleTransitionArmed] = useState(false)
  useEffect(() => {
    if (!isTabVisible) {
      setScaleTransitionArmed(false)
      return
    }
    let raf2 = 0
    const raf1 = requestAnimationFrame(() => {
      raf2 = requestAnimationFrame(() => setScaleTransitionArmed(true))
    })
    return () => {
      cancelAnimationFrame(raf1)
      if (raf2 !== 0) cancelAnimationFrame(raf2)
    }
  }, [isTabVisible])

  // Pointer → unscaled grid-content coordinates (px past the 4px
  // padding, in the grid's own pixel space). THE one place scale
  // enters pointer math: divide by the scale after removing the
  // centering/letterbox offsets (the shared content origin from
  // scaleLayout), then all existing cell math applies unchanged.
  const toGridXY = useCallback(
    (clientX: number, clientY: number): { x: number; y: number } | null => {
      const el = containerRef.current
      if (!el) return null
      const rect = el.getBoundingClientRect()
      const { scale, offsetX, offsetY } = scaleLayoutRef.current
      return {
        x: (clientX - rect.left - 4 - offsetX) / scale,
        y: (clientY - rect.top - 4 - offsetY) / scale,
      }
    },
    [],
  )

  // ── SGR mouse-button forwarding (mouse-reporting TUIs) ────────
  // When the child has ?1000/?1002 + ?1006 active (claude fullscreen),
  // plain press/drag/release belongs to the APP: forward it as SGR
  // reports over the same input channel the wheel branch proved out.
  // Shift/Option-drag bypasses forwarding for K2-native selection and
  // cmd-click stays the local link gesture — precedence lives in
  // `mouseRoute` (sgrMouse.ts). Both bits false (claude's normal
  // mode) ⇒ every path below is inert and K2 selection is untouched.
  //
  // Gesture state for a forwarded press→drag→release. Null when no
  // forwarded button is held. `lastCol/lastRow` = the cell most
  // recently reported (press or flushed motion) — the cell-change
  // gate's anchor.
  const sgrDragRef = useRef<{
    button: number
    lastCol: number
    lastRow: number
  } | null>(null)
  // Latest unsent drag cell (+ its ctrl state at event time). Motion
  // is idempotent-latest — unlike wheel notches — so coalescing to
  // the newest cell per flush tick is lossless.
  const sgrPendingMotionRef = useRef<(Cell & { ctrl: boolean }) | null>(null)
  const sgrMotionRafRef = useRef<number | null>(null)
  const sgrMotionWindowStartRef = useRef(0)
  const sgrMotionBudgetRef = useRef(0)
  // True while the in-flight click gesture was forwarded to the app.
  // `handleClick` consults it so a forwarded plain click never ALSO
  // activates a hovered link (cmd-click is never forwarded, so the
  // local link path keeps working).
  const gestureForwardedRef = useRef(false)

  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    // Same flood ceiling as the wheel bucket: ≤8 motion reports per
    // 50ms window; between flushes the latest cell wins.
    const MOTION_WINDOW_MS = 50
    const MOTIONS_PER_WINDOW = 8

    // Pointer → 1-based SGR cell. Same scale-aware math as the wheel
    // branch (including the scrollPx subtraction — identity on the
    // alt screen, where mouse-report mode virtually always lives),
    // plus an upper clamp so a drag that leaves the pane keeps
    // reporting the nearest edge cell instead of coordinates past
    // the grid (alacritty clamps its reports the same way).
    const sgrCellAt = (clientX: number, clientY: number): Cell | null => {
      const snap = snapshotRef.current
      if (!snap) return null
      const { width: cw, height: ch } = cellMetricsRef.current
      if (!cw || !ch) return null
      const pos = toGridXY(clientX, clientY)
      if (!pos) return null
      const col = Math.min(
        Math.max(1, Math.floor(pos.x / cw) + 1),
        Math.max(1, snap.cols),
      )
      const row = Math.min(
        Math.max(1, Math.floor((pos.y - scrollPxRef.current) / ch) + 1),
        Math.max(1, snap.rows),
      )
      return { col, row }
    }

    const stopMotionFlush = (): void => {
      if (sgrMotionRafRef.current !== null) {
        cancelAnimationFrame(sgrMotionRafRef.current)
        sgrMotionRafRef.current = null
      }
      sgrPendingMotionRef.current = null
    }

    const flushMotion = (): void => {
      sgrMotionRafRef.current = null
      const drag = sgrDragRef.current
      const pending = sgrPendingMotionRef.current
      if (!drag || !pending) return
      const now = performance.now()
      if (now - sgrMotionWindowStartRef.current >= MOTION_WINDOW_MS) {
        sgrMotionWindowStartRef.current = now
        sgrMotionBudgetRef.current = MOTIONS_PER_WINDOW
      }
      if (sgrMotionBudgetRef.current === 0) {
        // Window budget exhausted; keep the latest cell and re-check
        // next frame (a fresh 50ms window refills the bucket).
        sgrMotionRafRef.current = requestAnimationFrame(flushMotion)
        return
      }
      // Re-check the cell gate at flush time — the pointer may have
      // circled back onto the last-reported cell while parked.
      if (!cellChanged({ col: drag.lastCol, row: drag.lastRow }, pending)) {
        sgrPendingMotionRef.current = null
        return
      }
      sgrMotionBudgetRef.current -= 1
      drag.lastCol = pending.col
      drag.lastRow = pending.row
      sgrPendingMotionRef.current = null
      sendInput(
        encodeSgrMouse(drag.button, 'motion', pending.ctrl, pending.col, pending.row),
      )
    }

    const onDragMove = (ev: MouseEvent): void => {
      const drag = sgrDragRef.current
      if (!drag) return
      const cell = sgrCellAt(ev.clientX, ev.clientY)
      if (!cell) return
      // Cell-change gate (all three reference emulators do this): a
      // same-cell pixel wiggle never even schedules a flush, and a
      // pointer returning to the last-reported cell cancels the
      // stale intermediate (latest-cell-wins).
      if (!cellChanged({ col: drag.lastCol, row: drag.lastRow }, cell)) {
        sgrPendingMotionRef.current = null
        return
      }
      sgrPendingMotionRef.current = { ...cell, ctrl: ev.ctrlKey }
      if (sgrMotionRafRef.current === null) {
        sgrMotionRafRef.current = requestAnimationFrame(flushMotion)
      }
    }

    const onDragUp = (ev: MouseEvent): void => {
      const drag = sgrDragRef.current
      if (!drag) return
      sgrDragRef.current = null
      // The release report carries the final cell itself — a pending
      // intermediate motion is superseded, drop it. Press/release
      // always send immediately and never coalesce.
      stopMotionFlush()
      const cell = sgrCellAt(ev.clientX, ev.clientY) ?? {
        col: drag.lastCol,
        row: drag.lastRow,
      }
      sendInput(
        encodeSgrMouse(drag.button, 'release', ev.ctrlKey, cell.col, cell.row),
      )
      window.removeEventListener('mousemove', onDragMove)
      window.removeEventListener('mouseup', onDragUp)
    }

    const onDown = (ev: MouseEvent): void => {
      const snap = snapshotRef.current
      const route = mouseRoute(
        { mouseReport: snap?.mouseReport, sgrMouse: snap?.sgrMouse },
        {
          shift: ev.shiftKey,
          alt: ev.altKey,
          ctrl: ev.ctrlKey,
          meta: ev.metaKey,
        },
      )
      if (route !== 'forward') {
        gestureForwardedRef.current = false
        return
      }
      // The overlay scrollbar owns its own drag gesture.
      if (
        (ev.target as HTMLElement | null)?.closest?.(
          '[data-terminal-scrollbar]',
        )
      ) {
        gestureForwardedRef.current = false
        return
      }
      const cell = sgrCellAt(ev.clientX, ev.clientY)
      if (!cell) {
        gestureForwardedRef.current = false
        return
      }
      // Forwarded gesture: the app owns it. preventDefault stops the
      // browser from starting a text selection underneath the TUI's
      // own highlight — which also suppresses the container's
      // focus-on-mousedown, so focus the shadow textarea directly
      // (no native selection can exist for a forwarded gesture, so
      // the mouseDownInPaneRef mid-drag focus guard is moot here and
      // typing keeps landing in this pane after a TUI click).
      ev.preventDefault()
      gestureForwardedRef.current = true
      if (
        shadowInputRef.current &&
        document.activeElement !== shadowInputRef.current
      ) {
        shadowInputRef.current.focus({ preventScroll: true })
      }
      const button = sgrButtonCode(ev.button)
      sgrDragRef.current = { button, lastCol: cell.col, lastRow: cell.row }
      // Fresh bucket per gesture so a new drag never starts starved
      // by the previous one.
      sgrMotionWindowStartRef.current = performance.now()
      sgrMotionBudgetRef.current = MOTIONS_PER_WINDOW
      sendInput(encodeSgrMouse(button, 'press', ev.ctrlKey, cell.col, cell.row))
      // Window-level listeners so a drag that leaves the pane keeps
      // reporting (same pattern as the canvas-selection drag).
      window.addEventListener('mousemove', onDragMove)
      window.addEventListener('mouseup', onDragUp)
    }

    // Capture phase: the routing decision must precede the canvas-
    // selection mousedown (bubble phase) regardless of effect order.
    // No stopPropagation — React's synthetic handlers (link tracking,
    // mouseDownInPaneRef) still see the event; the canvas-selection
    // handler applies the same `mouseRoute` gate independently.
    el.addEventListener('mousedown', onDown, true)
    return () => {
      el.removeEventListener('mousedown', onDown, true)
      stopMotionFlush()
      sgrDragRef.current = null
      window.removeEventListener('mousemove', onDragMove)
      window.removeEventListener('mouseup', onDragUp)
    }
  }, [sendInput, toGridXY])

  // ── Canvas selection (webgl painter only) ─────────────────────
  // Native selection cannot exist over a canvas, so a grid-coordinate
  // model drives the painter's selection pass instead. Pixel→cell
  // uses the SAME pointer math as link hover (toGridXY + strip
  // window), so scale-to-fit and scroll are respected for free.
  // Handlers bind natively (not via the React props the DOM path
  // uses) and only when the flag is on — flag-off panes never run a
  // byte of this.
  useEffect(() => {
    if (!useWebgl) return
    const el = containerRef.current
    if (!el) return

    const setSelection = (sel: SelectionRange | null): void => {
      if (!webglSelectionRef.current && !sel) return
      webglSelectionRef.current = sel
      setSelectionVersion((v) => v + 1)
    }

    // Half-cell x rounding for drag boundaries (xterm Mouse.ts:44):
    // the left half of a cell selects it, the right half selects
    // from the next boundary. Word/line hits use plain floor.
    const pointToCell = (
      clientX: number,
      clientY: number,
      halfCell: boolean,
    ): SelectionPoint | null => {
      const snap = snapshotRef.current
      if (!snap) return null
      const { width: cw, height: ch } = cellMetricsRef.current
      if (!cw || !ch) return null
      const pos = toGridXY(clientX, clientY)
      if (!pos) return null
      const totalRows = snap.scrollback.length + snap.grid.length
      if (totalRows === 0) return null
      const layout = computeStripLayout(
        scrollPxRef.current,
        totalRows,
        snap.rows,
        ch,
        0,
      )
      const visualRow = Math.floor((pos.y + layout.fraction) / ch)
      const abs = Math.max(
        0,
        Math.min(totalRows - 1, layout.stripStart + visualRow),
      )
      const col = Math.max(
        0,
        Math.min(snap.cols, Math.floor(pos.x / cw + (halfCell ? 0.5 : 0))),
      )
      return { abs, col }
    }

    let anchor: SelectionPoint | null = null
    let lastClient = { x: 0, y: 0 }
    let autoScrollTimer: ReturnType<typeof setInterval> | null = null

    const stopAutoScroll = (): void => {
      if (autoScrollTimer !== null) {
        clearInterval(autoScrollTimer)
        autoScrollTimer = null
      }
    }

    const updateFocus = (): void => {
      if (!anchor) return
      const focus = pointToCell(lastClient.x, lastClient.y, true)
      if (!focus) return
      setSelection(normalizeSelection(anchor, focus))
    }

    const onDragMove = (ev: MouseEvent): void => {
      lastClient = { x: ev.clientX, y: ev.clientY }
      updateFocus()
      // Drag auto-scroll: pointer above/below the pane nudges the
      // viewport one line per tick while held (xterm's drag-scroll
      // interval); the focus recompute pins the selection end to the
      // moving window edge.
      const rect = el.getBoundingClientRect()
      const dir =
        ev.clientY < rect.top ? 1 : ev.clientY > rect.bottom ? -1 : 0
      if (dir === 0) {
        stopAutoScroll()
        return
      }
      if (autoScrollTimer !== null) return
      autoScrollTimer = setInterval(() => {
        const snap = snapshotRef.current
        const ch = cellMetricsRef.current.height || 20
        const scrollbackLen = snap?.scrollback.length ?? 0
        setScrollPx((px) => clampScrollPx(px + dir * ch, scrollbackLen, ch))
        updateFocus()
      }, 50)
    }

    const onDragUp = (): void => {
      stopAutoScroll()
      anchor = null
      window.removeEventListener('mousemove', onDragMove)
      window.removeEventListener('mouseup', onDragUp)
    }

    const onMouseDown = (ev: MouseEvent): void => {
      if (ev.button !== 0) return
      // Mouse-report gating: a plain gesture belongs to the TUI (the
      // SGR forwarding effect owns it); shift/option-drag keeps K2's
      // canvas selection per the universal override convention. Same
      // `mouseRoute` predicate as the forwarding effect so the two
      // can never both claim a gesture.
      const snapGate = snapshotRef.current
      if (
        mouseRoute(
          { mouseReport: snapGate?.mouseReport, sgrMouse: snapGate?.sgrMouse },
          {
            shift: ev.shiftKey,
            alt: ev.altKey,
            ctrl: ev.ctrlKey,
            meta: ev.metaKey,
          },
        ) === 'forward'
      ) {
        return
      }
      // The overlay scrollbar owns its own drag gesture.
      if (
        (ev.target as HTMLElement | null)?.closest?.(
          '[data-terminal-scrollbar]',
        )
      ) {
        return
      }
      const snap = snapshotRef.current
      if (!snap) return
      if (ev.detail === 2) {
        const p = pointToCell(ev.clientX, ev.clientY, false)
        if (!p) return
        const range = wordRangeAtCol(modelRowAt(snap, p.abs), p.col)
        setSelection(
          range
            ? {
                startAbs: p.abs,
                startCol: range.startCol,
                endAbs: p.abs,
                endCol: range.endCol,
              }
            : null,
        )
        return
      }
      if (ev.detail >= 3) {
        const p = pointToCell(ev.clientX, ev.clientY, false)
        if (!p) return
        setSelection({
          startAbs: p.abs,
          startCol: 0,
          endAbs: p.abs,
          endCol: snap.cols,
        })
        return
      }
      // Plain click collapses any prior selection; a drag rebuilds
      // from the anchor.
      setSelection(null)
      anchor = pointToCell(ev.clientX, ev.clientY, true)
      if (!anchor) return
      lastClient = { x: ev.clientX, y: ev.clientY }
      window.addEventListener('mousemove', onDragMove)
      window.addEventListener('mouseup', onDragUp)
    }

    el.addEventListener('mousedown', onMouseDown)
    return () => {
      el.removeEventListener('mousedown', onMouseDown)
      stopAutoScroll()
      window.removeEventListener('mousemove', onDragMove)
      window.removeEventListener('mouseup', onDragUp)
    }
  }, [useWebgl, toGridXY])

  // ── Link detection: Cmd key tracking ──────────────────────────
  // Also tracks Shift/Option as STATE (not a ref): in mouse-report
  // mode the pointer is an arrow because plain clicks belong to the
  // app — but holding the local-selection override must flip it back
  // to the I-beam so the pointer always tells the truth about who
  // owns the click (alacritty/xterm.js do the same). That flip is a
  // render concern, hence state; key events are low-frequency.
  const [selectionOverrideHeld, setSelectionOverrideHeld] = useState(false)
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Meta') cmdHeldRef.current = true
      if (e.key === 'Shift' || e.key === 'Alt') setSelectionOverrideHeld(true)
    }
    const onKeyUp = (e: KeyboardEvent) => {
      if (e.key === 'Meta') {
        cmdHeldRef.current = false
        if (linkClickMode === 'cmd-click') setHoveredLink(null)
      }
      if (e.key === 'Shift' || e.key === 'Alt') {
        setSelectionOverrideHeld(e.shiftKey || e.altKey)
      }
    }
    const onBlur = () => {
      cmdHeldRef.current = false
      setSelectionOverrideHeld(false)
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
      const { width: cw, height: ch } = cellMetrics
      if (cw === 0 || ch === 0) return
      // `toGridXY` removes the 4px padding, the letterbox offsets and
      // the scale, yielding grid-space pixels. Rows live in a strip
      // translated by -(fraction + overscanTop·cellH); adding
      // `fraction` back and offsetting by `overscanTop` inverts that
      // transform, so `row` indexes into `stripRows`.
      const pos = toGridXY(e.clientX, e.clientY)
      if (!pos) return
      const row =
        stripLayout.overscanTop +
        Math.floor((pos.y + stripLayout.fraction) / ch)
      const col = Math.floor(pos.x / cw)
      const visibleRow = stripRows[row]
      if (!visibleRow) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      const text = rowToText(visibleRow)
      if (!text.trim()) {
        if (hoveredLink) setHoveredLink(null)
        return
      }
      // Soft-wrap join: a URL/path that wraps spans several grid rows,
      // and scanning only the hovered row truncates the match at the
      // row edge (only the pre-wrap fragment was detected/opened).
      // Rebuild the LOGICAL line around the hovered row using the same
      // `wrapped` last-run flag the copy handler joins on: walk up
      // while the row ABOVE wraps into us, down while WE wrap onward.
      // Wrapped joins are mid-content (no trim, no separator) —
      // identical semantics to buildCopyText. Bounded to 8 rows each
      // way: a single link longer than that is pathological.
      const isRowWrapped = (r: CellRun[] | undefined): boolean =>
        !!r && r.length > 0 && r[r.length - 1].wrapped === true
      let firstRow = row
      while (row - firstRow < 8 && isRowWrapped(stripRows[firstRow - 1])) {
        firstRow--
      }
      let lastRow = row
      while (lastRow - row < 8 && isRowWrapped(stripRows[lastRow])) {
        lastRow++
      }
      let joined = ''
      let prefixLen = 0
      for (let r = firstRow; r <= lastRow; r++) {
        const t = rowToText(stripRows[r] ?? [])
        if (r < row) prefixLen += t.length
        joined += t
      }
      const links = detectLinks(joined, cwd)
      // Terminal column → UTF-16 text offset before comparing against
      // detectLinks ranges (string indices): a wide char occupies two
      // columns but one text position, so raw column compare would
      // skew every hit right of CJK/emoji content. Offset by the
      // joined-line prefix so the index addresses the logical line.
      const textIdx = prefixLen + colToTextIndex(visibleRow, col)
      const hit = links.find((l) => textIdx >= l.start && textIdx < l.end)
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
    [
      linkClickMode,
      hoveredLink,
      cellMetrics,
      snapshot,
      stripLayout,
      stripRows,
      cwd,
      toGridXY,
    ],
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
      // A forwarded gesture belongs to the TUI — never double-handle
      // it as a link activation. cmd-click is never forwarded
      // (mouseRoute routes meta to 'local'), so the cmd-click link
      // path below is unaffected in mouse-report mode.
      if (gestureForwardedRef.current) return
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
          console.warn('[kessel-term/link]', err),
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

  // ── Copy: rebuild selected text from the grid model ───────────
  // Native selection stays (the DOM rows are the selection surface),
  // but the copied TEXT is reconstructed from the CellRun model:
  //   - per-line trailing whitespace is trimmed (the padded-row wire
  //     format used to hand every selection dozens of phantom
  //     trailing spaces per line),
  //   - empty rows contribute a bare newline instead of the
  //     placeholder the renderer paints,
  //   - soft-wrapped rows join WITHOUT a newline (daemon marks them
  //     via the `wrapped` run flag), so a long command copies as one
  //     line the way iTerm/xterm.js do.
  // Selections that reach outside the row grid fall through to the
  // browser's default copy.
  //
  // Wide-char safety: the boundaries here are TEXT offsets, not
  // terminal columns — `colWithin` measures Range.toString().length
  // (UTF-16 units of the DOM text) and the model rows are sliced in
  // the same unit. With WIDE_CHAR_SPACER cells excluded from the wire
  // the DOM text IS the model text, so this path needs no column
  // mapping; column↔offset conversion (runCols.ts) is only for
  // pixel-derived positions (link hit-testing).
  const handleCopy = useCallback((e: React.ClipboardEvent) => {
    const snap = snapshotRef.current
    if (!snap || !e.clipboardData) return
    // WebGL painter path: no native selection exists over the canvas
    // — the grid-coordinate model is the source of truth. The ref is
    // always null in DOM mode, so this branch is unreachable there.
    const gpuSel = webglSelectionRef.current
    if (gpuSel) {
      e.preventDefault()
      e.clipboardData.setData('text/plain', copySelectionText(snap, gpuSel))
      return
    }
    const sel = window.getSelection()
    if (!sel || sel.isCollapsed || sel.rangeCount === 0) return
    const range = sel.getRangeAt(0)
    const container = containerRef.current
    if (!container || !container.contains(range.commonAncestorContainer)) {
      return
    }
    const startDiv = rowDivFor(range.startContainer)
    const endDiv = rowDivFor(range.endContainer)
    if (!startDiv || !endDiv) return
    const startAbs = Number(startDiv.dataset.absRow)
    const endAbs = Number(endDiv.dataset.absRow)
    if (!Number.isFinite(startAbs) || !Number.isFinite(endAbs)) return
    const startCol = colWithin(startDiv, range.startContainer, range.startOffset)
    const endCol = colWithin(endDiv, range.endContainer, range.endOffset)

    // Text extraction (wrapped-line join + trailing-trim) lives in
    // buildCopyText — shared verbatim with the painter's copy path.
    e.preventDefault()
    e.clipboardData.setData(
      'text/plain',
      buildCopyText(snap, startAbs, startCol, endAbs, endCol),
    )
  }, [])

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
      // Live box measurement for the scale-to-fit layout — updated on
      // EVERY fire, ahead of the debounce below: the hold-and-scale
      // rendering must track the box each frame of a drag while the
      // PTY resize itself stays debounced.
      const liveRect = entries[0]?.contentRect
      if (liveRect && liveRect.width > 0 && liveRect.height > 0) {
        setContainerSize((prev) =>
          prev.width === liveRect.width && prev.height === liveRect.height
            ? prev
            : { width: liveRect.width, height: liveRect.height },
        )
      }
      if (timer) clearTimeout(timer)
      timer = setTimeout(() => {
        timer = null
        const rect = entries[0]?.contentRect
        if (!rect || rect.width === 0 || rect.height === 0) return
        const availW = Math.max(0, rect.width - 4)
        const availH = Math.max(0, rect.height - 4)
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
  // Reads the grid through `snapshotRef` (not the `snapshot` state)
  // so the listener binds once per font/config change instead of
  // re-attaching on every frame — under heavy output the old
  // snapshot-keyed effect tore down and re-added the wheel listener
  // (and cancelled its pending flush timer) many times per second,
  // which is part of why scrolling felt dead while an agent was
  // streaming.
  const scrollAccumRef = useRef(0)
  const scrollRafRef = useRef<number | null>(null)
  // Mouse-reporting (fullscreen TUI) wheel: accumulate + flush per
  // animation frame, capped by a token bucket so a trackpad's momentum
  // flood can't storm the PTY (or a long-distance K2 Connect link).
  // The bucket keeps the ORIGINAL flood ceiling — 8 notches per 50ms —
  // but lets notches leave smoothly each frame; the old single 50ms
  // flush timer quantized TUI scrolling to 20Hz, which read as "locked
  // at 20fps" inside claude/Ink surfaces.
  const mouseWheelAccumRef = useRef(0)
  const mouseWheelRafRef = useRef<number | null>(null)
  const mouseWheelPosRef = useRef({ col: 1, row: 1 })
  const notchWindowStartRef = useRef(0)
  const notchBudgetRef = useRef(0)
  useEffect(() => {
    const el = containerRef.current
    if (!el) return
    const NOTCH_WINDOW_MS = 50
    const NOTCHES_PER_WINDOW = 8
    // Higher = less sensitive: one SGR notch per ~this many
    // cell-heights of accumulated movement. Tune to taste.
    const CELLS_PER_NOTCH = 1.0
    const flushMouseWheelNotches = () => {
      mouseWheelRafRef.current = null
      const ch2 = cellMetrics.height
      if (ch2 <= 0) return
      const now = performance.now()
      if (now - notchWindowStartRef.current >= NOTCH_WINDOW_MS) {
        notchWindowStartRef.current = now
        notchBudgetRef.current = NOTCHES_PER_WINDOW
      }
      const accum = mouseWheelAccumRef.current
      const notchPx = ch2 * CELLS_PER_NOTCH
      // Sub-notch remainder stays accumulated until more input arrives
      // — a momentum tail below one cell-height emits nothing (the old
      // min-1-tick flush over-scrolled on every 50ms tail tick).
      let ticks = Math.floor(Math.abs(accum) / notchPx)
      if (ticks === 0) return
      if (ticks > notchBudgetRef.current) ticks = notchBudgetRef.current
      if (ticks === 0) {
        // Window budget exhausted; keep the accumulation and re-check
        // next frame (a fresh 50ms window refills the bucket).
        mouseWheelRafRef.current = requestAnimationFrame(flushMouseWheelNotches)
        return
      }
      notchBudgetRef.current -= ticks
      mouseWheelAccumRef.current -= Math.sign(accum) * ticks * notchPx
      // SGR button: wheel-up = 64 (deltaY<0, toward older content),
      // wheel-down = 65.
      const btn = accum < 0 ? 64 : 65
      const { col: c, row: r } = mouseWheelPosRef.current
      const seq = `\x1b[<${btn};${c};${r}M`
      sendInput(seq.repeat(ticks))
      if (import.meta.env.DEV) scrollFlushCountRef.current++
      if (Math.abs(mouseWheelAccumRef.current) >= notchPx) {
        mouseWheelRafRef.current = requestAnimationFrame(flushMouseWheelNotches)
      }
    }
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0) return
      const snap = snapshotRef.current

      // ── Mouse-reporting apps (e.g. Claude `/tui fullscreen`) ────
      // When the child has DECSET mouse reporting on, it paints its
      // own scrollable surface (typically on the alt screen, which
      // has NO alacritty scrollback). Local-viewport scroll would be
      // a no-op, so instead forward the wheel as encoded mouse events
      // to the PTY and let the app scroll itself. SGR encoding only
      // (Claude uses ?1006h); legacy X10 wheel is left to local
      // scroll below (see commit note) since we can't reliably emit
      // the high-bit byte form over the JSON text-input channel.
      if (snap?.mouseReport && snap?.sgrMouse) {
        const cw = cellMetrics.width
        const ch2 = cellMetrics.height
        if (cw > 0 && ch2 > 0) {
          e.preventDefault()
          // Same 0-based cell math as the link/hover handler (shared
          // scale-aware pointer helper); SGR mouse coordinates are
          // 1-based, so add 1 and clamp to ≥1. SGR rows address GRID
          // cells: with the local viewport scrolled up, grid rows sit
          // `scrollPx` px lower on screen, so subtract it. (Mouse-
          // report mode virtually always means alt screen ⇒ no
          // scrollback ⇒ scrollPx re-clamped to 0 ⇒ identity.)
          const pos = toGridXY(e.clientX, e.clientY)
          if (!pos) return
          const col = Math.max(1, Math.floor(pos.x / cw) + 1)
          const row = Math.max(
            1,
            Math.floor((pos.y - scrollPxRef.current) / ch2) + 1,
          )
          mouseWheelPosRef.current = { col, row }
          const pixelDelta =
            e.deltaMode === WheelEvent.DOM_DELTA_LINE
              ? e.deltaY * ch2
              : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
                ? e.deltaY * ch2 * (snap?.rows ?? 24)
                : e.deltaY
          mouseWheelAccumRef.current += pixelDelta
          if (mouseWheelRafRef.current === null) {
            mouseWheelRafRef.current = requestAnimationFrame(
              flushMouseWheelNotches,
            )
          }
          return
        }
      }

      // ── Local viewport scroll ────────────────────────────────
      // Flushes once per animation frame instead of the old 50ms
      // timer, which hard-capped scrolling at 20Hz — the single
      // biggest "low refresh rate" feel. Deltas accumulate between
      // frames and apply as PIXELS (the strip renders fractional
      // positions), so nothing is quantized away per flush.
      e.preventDefault()
      const cellH = cellMetrics.height || 20
      const pixelDelta =
        e.deltaMode === WheelEvent.DOM_DELTA_LINE
          ? e.deltaY * cellH
          : e.deltaMode === WheelEvent.DOM_DELTA_PAGE
            ? e.deltaY * cellH * (snap?.rows ?? 24)
            : e.deltaY
      scrollAccumRef.current += pixelDelta
      if (scrollRafRef.current === null) {
        scrollRafRef.current = requestAnimationFrame(() => {
          scrollRafRef.current = null
          const accum = scrollAccumRef.current
          scrollAccumRef.current = 0
          if (accum === 0) return
          const deltaPx = accum * config.scrolling.multiplier
          const scrollbackLen = snapshotRef.current?.scrollback.length ?? 0
          if (import.meta.env.DEV) scrollFlushCountRef.current++
          // deltaY > 0 scrolls toward the bottom (scrollPx → 0).
          setScrollPx((px) => clampScrollPx(px - deltaPx, scrollbackLen, cellH))
        })
      }
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => {
      el.removeEventListener('wheel', onWheel)
      if (scrollRafRef.current !== null) {
        cancelAnimationFrame(scrollRafRef.current)
        scrollRafRef.current = null
      }
      if (mouseWheelRafRef.current !== null) {
        cancelAnimationFrame(mouseWheelRafRef.current)
        mouseWheelRafRef.current = null
      }
    }
  }, [
    config.scrolling.multiplier,
    cellMetrics.height,
    cellMetrics.width,
    sendInput,
    toGridXY,
  ])

  // ── Re-clamp scroll position on snapshot change ───────────────
  // A smaller-scrollback resend (resize / restart / alt-screen
  // switch) can strand `scrollPx` past the new scrollback length,
  // which would freeze the viewport on a blank window. Clamp it
  // down whenever the scrollback shrinks below the position.
  // `clampScrollPx` is identity for in-range values, so the
  // functional update bails without a re-render on the common path.
  useEffect(() => {
    const scrollbackLen = snapshot?.scrollback.length ?? 0
    setScrollPx((px) => clampScrollPx(px, scrollbackLen, cellHeightPx))
  }, [snapshot, cellHeightPx])

  // ── Overlay scrollbar ─────────────────────────────────────────
  // Thin right-edge overlay: proportional thumb, positioned from the
  // pixel scroll state. Shown while the position is changing, while
  // the pointer is over the bar, or during a thumb drag; fades out
  // ~1s after the last movement (CSS opacity transition). The track
  // is the ONLY element that takes pointer events, so terminal
  // selection is unaffected outside its 8px column.
  const scrollbarTrackRef = useRef<HTMLDivElement>(null)
  const [scrollbarActive, setScrollbarActive] = useState(false)
  const [scrollbarHover, setScrollbarHover] = useState(false)
  const [scrollbarDragging, setScrollbarDragging] = useState(false)
  const scrollbarHideTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const prevScrollPxRef = useRef(0)
  useEffect(() => {
    // Only genuine position CHANGES arm the bar — a re-run with the
    // same value (mount, unrelated re-render) keeps it hidden.
    if (prevScrollPxRef.current === scrollPx) return
    prevScrollPxRef.current = scrollPx
    setScrollbarActive(true)
    if (scrollbarHideTimerRef.current) clearTimeout(scrollbarHideTimerRef.current)
    scrollbarHideTimerRef.current = setTimeout(() => {
      scrollbarHideTimerRef.current = null
      setScrollbarActive(false)
    }, 1000)
  }, [scrollPx])
  useEffect(
    () => () => {
      if (scrollbarHideTimerRef.current) {
        clearTimeout(scrollbarHideTimerRef.current)
        scrollbarHideTimerRef.current = null
      }
    },
    [],
  )

  // Null when there's nothing to scroll — the bar doesn't render at
  // all, so a fresh shell / alt-screen TUI never shows a phantom bar.
  const scrollbarThumb = useMemo(() => {
    if (!snapshot) return null
    return computeScrollbarThumb(
      scrollPx,
      snapshot.scrollback.length,
      snapshot.scrollback.length + snapshot.grid.length,
      snapshot.rows,
      cellHeightPx,
    )
  }, [snapshot, scrollPx, cellHeightPx])

  // Mousedown anywhere on the track owns the gesture: a hit inside
  // the thumb drags from the grabbed point; a hit on bare track
  // centers the thumb on the pointer (jump) and continues as a drag.
  // Listeners go on window so the drag survives leaving the pane.
  const handleScrollbarMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      e.stopPropagation()
      const track = scrollbarTrackRef.current
      const snap = snapshotRef.current
      if (!track || !snap) return
      const ch = cellMetrics.height || 20
      const scrollbackLen = snap.scrollback.length
      const thumb = computeScrollbarThumb(
        scrollPxRef.current,
        scrollbackLen,
        scrollbackLen + snap.grid.length,
        snap.rows,
        ch,
      )
      if (!thumb) return
      const rect = track.getBoundingClientRect()
      if (rect.height <= 0) return
      const yFrac = (e.clientY - rect.top) / rect.height
      const onThumb =
        yFrac >= thumb.topFrac && yFrac <= thumb.topFrac + thumb.heightFrac
      const grabFrac = onThumb ? yFrac - thumb.topFrac : thumb.heightFrac / 2
      const apply = (clientY: number) => {
        const topFrac = (clientY - rect.top) / rect.height - grabFrac
        setScrollPx(
          scrollPxFromThumbTopFrac(topFrac, thumb.heightFrac, scrollbackLen, ch),
        )
      }
      apply(e.clientY)
      setScrollbarDragging(true)
      const onMove = (ev: MouseEvent) => apply(ev.clientY)
      const onUp = () => {
        setScrollbarDragging(false)
        window.removeEventListener('mousemove', onMove)
        window.removeEventListener('mouseup', onUp)
      }
      window.addEventListener('mousemove', onMove)
      window.addEventListener('mouseup', onUp)
    },
    [cellMetrics.height],
  )

  // ── Styles ────────────────────────────────────────────────────
  const containerStyle: React.CSSProperties = useMemo(
    () => ({
      fontFamily: config.font.family,
      fontSize: `${fontSize}px`,
      lineHeight: `${Math.ceil(fontSize * config.font.lineHeightMultiplier)}px`,
      color: `rgb(${(config.colors.foreground >> 16) & 0xff},${(config.colors.foreground >> 8) & 0xff},${config.colors.foreground & 0xff})`,
      // Seam fill (see `seamBg`): a fullscreen TUI's own bg extends
      // into the cell-quantization remainder at the right/bottom
      // edges instead of exposing the theme background as a seam.
      backgroundColor: hexToCss(seamBg ?? config.colors.background),
      whiteSpace: 'pre',
      // Right and bottom edges are padding-free: their space goes to the
      // column/row math; visual breathing room there comes from the
      // centered remainder (same treatment the right edge got in 0.40.25).
      padding: '4px 0 0 4px',
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
      seamBg,
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

  // Content origin: where grid cell (0,0) sits inside the container,
  // in unscaled px — the 4px padding plus the centering/letterbox
  // offset from scaleLayout. EVERY overlay positioned in container
  // space (cursor overlay, IME shadow textarea) must add these;
  // pointer math inverts the same values inside toGridXY, and the
  // DOM strip / WebGL canvas get them via the scale wrapper's
  // translate. One origin, all consumers.
  const contentOriginX = 4 + scaleLayout.offsetX
  const contentOriginY = 4 + scaleLayout.offsetY

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
        left: `${contentOriginX + cellMetrics.width * snapshot.cursor.col}px`,
        // The cursor cell lives in the grid (bottom of the buffer);
        // scrolling up moves it DOWN the screen by exactly the
        // pixel scroll position.
        top: `${contentOriginY + cellMetrics.height * snapshot.cursor.row + scrollPx}px`,
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
    scrollPx,
    contentOriginX,
    contentOriginY,
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
    // span underneath already renders it. Gated on the exact bottom
    // (scrollPx === 0): any pixel of scroll shifts the grid rows, so
    // an anchored overlay would float off its cell.
    if (snapshot.cursor.visible && scrollPx === 0) {
      const cursorVisibleRow = snapshot.cursor.row
      if (cursorVisibleRow >= 0 && cursorVisibleRow < snapshot.rows) {
        const baseStyle: React.CSSProperties = {
          position: 'absolute',
          left: `${contentOriginX + cellMetrics.width * snapshot.cursor.col}px`,
          top: `${contentOriginY + cellMetrics.height * cursorVisibleRow}px`,
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
    if (!isFocused && !snapshot.cursor.visible && scrollPx === 0) {
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
          // Column position, not char count — wide runs span more
          // columns than chars (runCols.ts).
          cellCol += runColSpan(run)
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
            left: `${contentOriginX + cellMetrics.width * found.col}px`,
            top: `${contentOriginY + cellMetrics.height * found.row - 1}px`,
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
  }, [
    snapshot,
    cellMetrics,
    scrollPx,
    isFocused,
    defaultBgCss,
    defaultFgCss,
    contentOriginX,
    contentOriginY,
  ])

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
        Kessel: {phase.message}
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
    // Mouse-report mode shows the arrow cursor (alacritty's cursor-
    // icon convention: I-beam only when a local selection applies) —
    // plain clicks/drags belong to the app there. Holding the
    // Shift/Option override flips back to the I-beam, since that
    // gesture IS a local selection.
    cursor: hoveredLink
      ? 'pointer'
      : snapshot?.mouseReport && snapshot?.sgrMouse && !selectionOverrideHeld
        ? 'default'
        : 'text',
    // Canvas mode: no DOM text to select — suppress native selection
    // so stray drags over overlay text (HUD, cursor char) can't fight
    // the model selection. DOM mode keeps native selection.
    ...(useWebgl
      ? { userSelect: 'none' as const, WebkitUserSelect: 'none' as const }
      : {}),
    // Composer 1b: the pane now lives inside a flex-column wrapper so the
    // compose bar can dock beneath it. Override the `height: 100%` from
    // `containerStyle` with flex-grow + `minHeight: 0` so the terminal
    // shrinks to leave room for the bar (and the ResizeObserver reshapes
    // the PTY to the smaller height) instead of overflowing it.
    height: 'auto',
    minHeight: 0,
  }

  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        width: '100%',
        height: '100%',
        overflow: 'hidden',
      }}
      data-terminal-pane-wrapper=""
    >
    <div
      ref={containerRef}
      className="kessel-pane"
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
      onCopy={handleCopy}
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
      {/* Rows render into a strip translated by the sub-row scroll
          fraction (+ top-overscan compensation), inside a viewport
          pinned to the container's content box (`inset: 4px` matches
          the 4px padding) that clips the overscan rows. A scroll that
          stays within one row changes ONLY the strip's transform — a
          compositor-side translation; the memoized rows don't
          re-render. Row keys stay `abs-<n>` (absolute buffer row) and
          each div keeps `data-abs-row`, the anchors that native
          selection and the copy handler depend on. */}
      <div
        style={{
          position: 'absolute',
          top: 4,
          right: 4,
          bottom: 4,
          left: 4,
          overflow: 'hidden',
        }}
      >
        {/* Scale wrapper (passive scale-to-fit + resize hold): the
            whole strip scales uniformly from the top-left, offset to
            center/letterbox. Always present so DOM identity (and any
            native selection anchored in the rows) survives scale
            transitions; identity renders as translate(0,0) scale(1).
            The short transition is what makes claim-takeover and the
            resize hold read as a smooth reflow instead of a snap —
            armed only after a show has settled (see
            `scaleTransitionArmed`) so a portal move never animates. */}
        <div
          style={{
            transform: `translate(${scaleLayout.offsetX}px, ${scaleLayout.offsetY}px) scale(${scaleLayout.scale})`,
            transformOrigin: 'top left',
            transition: scaleTransitionArmed
              ? 'transform 120ms ease-out'
              : 'none',
          }}
        >
        {useWebgl ? (
          /* WebGL painter host. Replaces ONLY the row strip: it sits
             inside the same clipping viewport + scale wrapper, so
             passive scale-to-fit and the resize hold apply as CSS
             transforms over the canvas exactly as they do over the
             DOM rows. Scrolling is painted internally (fraction
             uniform), not via translateY. pointer-events stay on the
             container — the canvas is inert like the row divs. */
          <canvas
            ref={webglCanvasRef}
            data-terminal-webgl-canvas=""
            style={{ display: 'block', pointerEvents: 'none' }}
          />
        ) : (
        <div
          style={{
            transform: `translateY(${stripLayout.translateY}px)`,
            willChange: 'transform',
          }}
        >
          {stripRows.map((row, rowIdx) => {
            const absRow = stripAbsRows[rowIdx] ?? rowIdx
            return (
              <TerminalRow
                key={`abs-${absRow}`}
                row={row}
                absRow={absRow}
                defaultFg={defaultFgCss}
                defaultBg={defaultBgCss}
                cellWidth={cellMetrics.width}
                cellHeight={cellMetrics.height}
                dpr={dpr}
              />
            )
          })}
        </div>
        )}
        </div>
      </div>
      {/* Passive-view affordance: this pane is watching a grid sized
          by another viewer (scaled down to fit). Styling matches the
          DEV badge above. */}
      {scaleLayout.passive && snapshot && (
        <div
          data-terminal-passive-pill=""
          style={{
            position: 'absolute',
            bottom: 6,
            right: 8,
            padding: '2px 6px',
            background: 'rgba(0,0,0,0.8)',
            color: '#9a9a9a',
            fontSize: '10px',
            fontFamily: 'monospace',
            zIndex: 999,
            pointerEvents: 'none',
            borderRadius: '3px',
          }}
        >
          viewing at {snapshot.cols}×{snapshot.rows}
        </div>
      )}
      {scrollbarThumb && (
        <div
          ref={scrollbarTrackRef}
          data-terminal-scrollbar=""
          onMouseEnter={() => setScrollbarHover(true)}
          onMouseLeave={() => setScrollbarHover(false)}
          onMouseDown={handleScrollbarMouseDown}
          onClick={(e) => e.stopPropagation()}
          style={{
            position: 'absolute',
            top: 4,
            bottom: 4,
            right: 2,
            width: 8,
            zIndex: 20,
            opacity:
              scrollbarActive || scrollbarHover || scrollbarDragging ? 1 : 0,
            transition: 'opacity 250ms ease',
            cursor: 'default',
          }}
        >
          <div
            style={{
              position: 'absolute',
              left: 0,
              right: 0,
              top: `${scrollbarThumb.topFrac * 100}%`,
              height: `${scrollbarThumb.heightFrac * 100}%`,
              borderRadius: 4,
              backgroundColor: scrollbarDragging
                ? 'rgba(255,255,255,0.4)'
                : 'rgba(255,255,255,0.25)',
            }}
          />
        </div>
      )}
      {/* Cursor overlay positions in UNSCALED grid space (it's a
          sibling of the scale wrapper) — hide it while scaled; a
          scaled view is passive/transitional and the TUI paints its
          own cursor cell inside the (scaled) grid anyway. */}
      {cursorOverlay && scaleLayout.scale === 1 && (
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
          <strong style={{ color: '#fff' }}>Kessel</strong>
          {painterKind === 'webgl' && (
            <span style={{ color: painterFatal ? '#f66' : '#6f6' }}>
              {' '}
              · {painterFatal ? `dom(webgl:${painterFatal})` : 'webgl'}
            </span>
          )}
          {' '}· phase:{phase.kind}
          {' '}cells:{snapshot?.cols ?? '?'}x{snapshot?.rows ?? '?'}
          {' '}cursor:{snapshot?.cursor.col ?? 0},{snapshot?.cursor.row ?? 0}
          {' '}offPx:{Math.round(scrollPx)}
          {' '}scr:{snapshot?.scrollback.length ?? 0}
          {' '}v:{snapshot?.version ?? 0}
          <span ref={fpsSpanRef} style={{ color: '#6cf' }} />
          {!isReady && phase.kind !== 'idle' && ' · loading'}
        </div>
      )}
    </div>

      {/* Composer 1b — message bar docked beneath the live pane. Gated on
       *  phase 'ready' so it only renders with a RESOLVED daemon
       *  sessionId (`phase.sessionId`, the id the daemon minted at
       *  /cli/sessions/v2/spawn and that the grid-WS streams), NEVER the
       *  renderer's `terminalId` and never a null/stale id. The composer
       *  route resolves via `lookup_by_session_id`, so it must get the
       *  real daemon session id. */}
      {phase.kind === 'ready' && (
        <TerminalComposeBar sessionId={phase.sessionId} />
      )}
    </div>
  )
}
