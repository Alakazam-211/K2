import { useEffect, useRef, useCallback, useState } from 'react'
import { listen } from '@tauri-apps/api/event'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import {
  terminalCreate,
  terminalExists,
  terminalGetGrid,
  terminalResize,
  terminalScroll,
  terminalSetFocus,
  terminalWrite,
} from '@/lib/terminal-daemon'
import { useTerminalSettingsStore } from '@/stores/terminal-settings'
import { useSettingsStore } from '@/stores/settings'
import { keyEventToSequence, naturalTextEditingSequence } from '@/lib/key-mapping'
import { isFileDragActive, markDropConsumed, isImagePath, quotePathForImageDrop, bracketPaste } from '@/lib/file-drag'
import { detectWorkingSignal } from '@/lib/agent-signals'
import { detectLinks, type DetectedLink } from './terminalLinkDetector'
import { useTabsStore } from '@/stores/tabs'
import { useActiveAgentsStore } from '@/stores/active-agents'
import { useToastStore } from '@/stores/toast'
import { useConnectHostStore } from '@/stores/connect-host'
import { executeRemoteDrop } from '@/lib/handle-remote-drop'

// ── Types matching Rust GridUpdate / CompactLine / StyleSpan ──────────

interface StyleSpan {
  s: number  // start column (inclusive)
  e: number  // end column (inclusive)
  fg?: number  // foreground 0xRRGGBB
  bg?: number  // background 0xRRGGBB
  fl?: number  // flags bitfield
}

interface CompactLine {
  row: number
  text: string
  spans?: StyleSpan[]
}

interface GridUpdate {
  cols: number
  rows: number
  cursor_col: number
  cursor_row: number
  cursor_visible: boolean
  cursor_shape: string
  lines: CompactLine[]
  full: boolean
  mode: number
  display_offset: number
  selection?: [number, number, number, number]
}

// Attribute flags (must match Rust ATTR_* constants)
const ATTR_BOLD = 1
const ATTR_ITALIC = 2
const ATTR_UNDERLINE = 4
const ATTR_STRIKETHROUGH = 8
const ATTR_INVERSE = 16
const ATTR_DIM = 32
const ATTR_HIDDEN = 64

const DEFAULT_FG = 0xe0e0e0
const DEFAULT_BG = 0x0a0a0a

// ── Props ────────────────────────────────────────────────────────────

interface AlacrittyTerminalViewProps {
  terminalId: string
  tabId?: string
  paneGroupId?: string
  cwd: string
  command?: string
  args?: string[]
  onExit?: (exitCode: number) => void
  /** performance.now() at Cmd+T / Cmd+Shift+T press. Used to log
   *  end-to-end spawn→visible latency for parity comparison with
   *  Kessel. See tabs.ts TerminalItemData.spawnedAt. */
  spawnedAt?: number
}

function shellEscape(path: string): string {
  // Escape special shell characters with backslashes (like iTerm2/Terminal.app)
  return path.replace(/[ '"\\()&|;<>$`!#*?[\]{}~]/g, '\\$&')
}

// Image/PDF paths skip backslash-escape so Claude Code's `[Image #N]`
// detection (which fs.exists()s the literal string) can resolve them.
function formatPathForTerminal(path: string): string {
  return isImagePath(path) ? quotePathForImageDrop(path) : shellEscape(path)
}

/**
 * Build the terminal payload for a dropped/pasted set of paths. If any
 * path is an image, the full payload is wrapped in bracketed paste so
 * Claude Code's paste-event image detector fires and renders `[Image #N]`.
 * Plain terminals with bracketed paste enabled (zsh, bash, vim) see a
 * normal paste and insert the text as-is.
 */
function buildDropPayload(paths: string[]): string {
  const formatted = paths.map(formatPathForTerminal).join(' ')
  const trailing = formatted + ' '
  return paths.some(isImagePath) ? bracketPaste(trailing) : trailing
}

function colorToCSS(c: number): string {
  return `rgb(${(c >> 16) & 0xff},${(c >> 8) & 0xff},${c & 0xff})`
}

// ── Line Renderer ────────────────────────────────────────────────────

function renderLineSpans(line: CompactLine): React.JSX.Element[] {
  const { text, spans } = line
  if (!text) return [<span key="empty">{'\u00A0'}</span>]
  if (!spans || spans.length === 0) return [<span key="default">{text || '\u00A0'}</span>]

  const elements: React.JSX.Element[] = []
  let lastEnd = 0

  // Convert character indices to string positions
  // CompactLine text has trailing spaces trimmed, spans reference column indices
  const chars = [...text]  // handle multi-byte chars correctly

  for (let i = 0; i < spans.length; i++) {
    const span = spans[i]

    // Add unstyled text before this span
    if (span.s > lastEnd) {
      const unstyled = chars.slice(lastEnd, span.s).join('')
      if (unstyled) elements.push(<span key={`g${i}`}>{unstyled}</span>)
    }

    // Build style for this span
    const style: React.CSSProperties = {}
    const flags = span.fl ?? 0
    let fg = span.fg ?? DEFAULT_FG
    let bg = span.bg ?? DEFAULT_BG

    if (flags & ATTR_INVERSE) {
      const tmp = fg; fg = bg; bg = tmp
    }

    if (fg !== DEFAULT_FG) style.color = colorToCSS(fg)
    if (bg !== DEFAULT_BG) style.backgroundColor = colorToCSS(bg)
    if (flags & ATTR_BOLD) style.fontWeight = 'bold'
    if (flags & ATTR_ITALIC) style.fontStyle = 'italic'
    if (flags & ATTR_DIM) style.opacity = 0.7
    if (flags & ATTR_HIDDEN) style.color = 'transparent'
    if (flags & ATTR_UNDERLINE) style.textDecoration = 'underline'
    if (flags & ATTR_STRIKETHROUGH) {
      style.textDecoration = style.textDecoration
        ? `${style.textDecoration} line-through`
        : 'line-through'
    }

    const styledText = chars.slice(span.s, span.e + 1).join('')
    elements.push(
      <span key={`s${i}`} style={Object.keys(style).length > 0 ? style : undefined}>
        {styledText}
      </span>
    )

    lastEnd = span.e + 1
  }

  // Add remaining unstyled text after last span
  if (lastEnd < chars.length) {
    elements.push(<span key="tail">{chars.slice(lastEnd).join('')}</span>)
  }

  return elements
}

// ── Component ────────────────────────────────────────────────────────

export function AlacrittyTerminalView({
  terminalId,
  tabId,
  paneGroupId,
  cwd,
  command,
  args,
  onExit,
  spawnedAt,
}: AlacrittyTerminalViewProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const viewportRef = useRef<HTMLDivElement>(null)
  const ptyIdRef = useRef<string | null>(null)
  const cellMetricsRef = useRef<{ width: number; height: number }>({ width: 0, height: 0 })
  const [cellMetrics, setCellMetrics] = useState<{ width: number; height: number }>({ width: 0, height: 0 })
  const lastColsRef = useRef(0)
  const lastRowsRef = useRef(0)
  const termModeRef = useRef(0)
  const rafPendingRef = useRef(false)
  const pendingFrameRef = useRef<GridUpdate | null>(null)

  const fontSize = useTerminalSettingsStore((s) => s.fontSize)
  const linkClickMode = useTerminalSettingsStore((s) => s.linkClickMode)
  const settings = useSettingsStore((s) => s.settings)
  const naturalTextEditing = settings?.terminal?.naturalTextEditing !== false

  const [created, setCreated] = useState(false)
  const [cursorBlinkVisible, setCursorBlinkVisible] = useState(true)

  // ── Link detection state ──────────────────────────────────────────
  const cmdHeldRef = useRef(false)
  const [hoveredLink, setHoveredLink] = useState<{ row: number; link: DetectedLink } | null>(null)
  const mouseDownLinkRef = useRef<DetectedLink | null>(null)
  const lastDetectPosRef = useRef<{ x: number; y: number }>({ x: 0, y: 0 })
  const lastDetectTimeRef = useRef(0)

  // Grid state
  const linesRef = useRef<Map<number, CompactLine>>(new Map())
  const [gridState, setGridState] = useState<{
    rows: number
    cols: number
    cursorCol: number
    cursorRow: number
    cursorVisible: boolean
    cursorShape: string
    displayOffset: number
    version: number  // increment to trigger re-render
  }>({ rows: 24, cols: 80, cursorCol: 0, cursorRow: 0, cursorVisible: true, cursorShape: 'bar', displayOffset: 0, version: 0 })

  // Debug
  const frameCountRef = useRef(0)
  const wheelEventCountRef = useRef(0)

  // Working-state detection — last time we saw an LLM "working" hint in
  // the bottom rows of the grid. The idle-check interval (see effect
  // below) flips the pane status to idle once the hint has been absent
  // long enough to rule out a single-frame gap.
  const lastSeenWorkingAtRef = useRef(0)
  const [debugInfo, setDebugInfo] = useState({ frames: 0, offset: 0, wheel: 0 })

  // ── Measure cell metrics from DOM ───────────────────────────────────

  const measureCell = useCallback(() => {
    const span = document.createElement('span')
    span.style.cssText = `font-family: 'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace; font-size: ${fontSize}px; position: absolute; visibility: hidden; white-space: pre;`
    span.textContent = 'W'
    document.body.appendChild(span)
    const rect = span.getBoundingClientRect()
    document.body.removeChild(span)
    const width = rect.width
    const height = Math.ceil(fontSize * 1.2)  // match Rust font_renderer cell_height = px_size * 1.2
    cellMetricsRef.current = { width, height }
    setCellMetrics({ width, height })
    return { width, height }
  }, [fontSize])

  // ── Apply grid update ───────────────────────────────────────────────

  const applyGridUpdate = useCallback((update: GridUpdate) => {
    const map = linesRef.current

    if (update.full) {
      map.clear()
    }

    for (const line of update.lines) {
      map.set(line.row, line)
    }

    frameCountRef.current++
    termModeRef.current = update.mode

    // Viewport working-state detection — gated on displayOffset === 0 so
    // a scrolled-up user doesn't accidentally pin the pane in 'working'
    // state by keeping the spinner-row off-screen.
    if (update.display_offset === 0) {
      if (detectWorkingSignal(map, update.rows)) {
        lastSeenWorkingAtRef.current = Date.now()
        useActiveAgentsStore.getState().recordTitleActivity(terminalId, true)
      }
    }

    setGridState({
      rows: update.rows,
      cols: update.cols,
      cursorCol: update.cursor_col,
      cursorRow: update.cursor_row,
      cursorVisible: update.cursor_visible,
      cursorShape: update.cursor_shape,
      displayOffset: update.display_offset,
      version: frameCountRef.current,
    })

    setDebugInfo({
      frames: frameCountRef.current,
      offset: update.display_offset,
      wheel: wheelEventCountRef.current,
    })
  }, [terminalId])

  // ── rAF-batched rendering ──────────────────────────────────────────
  // Each GridUpdate is a full visible-grid snapshot, so only the latest
  // frame matters — intermediate ones are safely overwritten.

  // ── Working-state idle watcher ─────────────────────────────────────
  // Working → idle transitions happen when no hint has been seen for
  // 1s. We check on a 500ms timer so the transition is at most ~1.5s
  // after the real transition, but never flickers on single-frame gaps.
  useEffect(() => {
    const IDLE_GRACE_MS = 1000
    const interval = setInterval(() => {
      const last = lastSeenWorkingAtRef.current
      if (last === 0) return // never seen working — don't touch state
      if (Date.now() - last > IDLE_GRACE_MS) {
        useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
        lastSeenWorkingAtRef.current = 0
      }
    }, 500)
    return () => clearInterval(interval)
  }, [terminalId])

  const scheduleRender = useCallback((payload: GridUpdate) => {
    pendingFrameRef.current = payload
    if (rafPendingRef.current) return
    rafPendingRef.current = true
    requestAnimationFrame(() => {
      rafPendingRef.current = false
      const frame = pendingFrameRef.current
      pendingFrameRef.current = null
      if (frame) applyGridUpdate(frame)
    })
  }, [applyGridUpdate])

  // ── Calculate grid dimensions ──────────────────────────────────────

  const calculateDimensions = useCallback(() => {
    const container = containerRef.current
    if (!container) return { cols: 80, rows: 24 }
    const { width, height } = cellMetricsRef.current
    if (width === 0 || height === 0) return { cols: 80, rows: 24 }
    // Container may be hidden (display:none on an inactive retained-view
    // tab) — clientWidth/Height are 0 in that case. Fall back to 80x24
    // so the PTY is born at a usable size; the ResizeObserver will
    // reshape it to real dimensions when the tab becomes visible.
    if (container.clientWidth === 0 || container.clientHeight === 0) {
      return { cols: 80, rows: 24 }
    }
    const cols = Math.max(1, Math.floor(container.clientWidth / width))
    const rows = Math.max(1, Math.floor(container.clientHeight / height))
    return { cols, rows }
  }, [])

  // ── Font size changes ──────────────────────────────────────────────

  useEffect(() => {
    measureCell()
    if (!created || !ptyIdRef.current) return
    const { cols, rows } = calculateDimensions()
    if (cols !== lastColsRef.current || rows !== lastRowsRef.current) {
      lastColsRef.current = cols
      lastRowsRef.current = rows
      terminalResize(ptyIdRef.current, cols, rows).catch((e) => console.warn('[terminal]', e))
    }
  }, [fontSize, created, calculateDimensions, measureCell])

  // ── Create terminal + listen for events ────────────────────────────

  useEffect(() => {
    let unlistenGrid: (() => void) | undefined
    let unlistenExit: (() => void) | undefined
    let unlistenTitle: (() => void) | undefined
    let unlistenDrop: (() => void) | undefined
    let mounted = true

    const setup = async (): Promise<void> => {
      // Measure cell metrics first
      measureCell()

      const exists = await terminalExists(terminalId)

      if (!exists) {
        // Measure container and create terminal with correct initial dimensions
        const { cols, rows } = calculateDimensions()
        lastColsRef.current = cols
        lastRowsRef.current = rows
        // eslint-disable-next-line no-console
        console.info(
          `%c[Alacritty] creating terminal ${terminalId}`,
          'color:#ff0;font-weight:bold',
          { cwd, command, args, cols, rows },
        )
        await terminalCreate({
          id: terminalId,
          cwd,
          command: command ?? null,
          args: args ?? null,
          cols,
          rows,
        })
      }

      // Reattach: get current grid state
      if (exists) {
        try {
          const grid = await terminalGetGrid<GridUpdate>(terminalId)
          if (mounted) applyGridUpdate(grid)
        } catch { /* fallback */ }
        // Reset so the resize poll fires on the next tick
        lastColsRef.current = 0
        lastRowsRef.current = 0
      }

      if (!mounted) return
      ptyIdRef.current = terminalId
      setCreated(true)

      // E2E timing — matches the Kessel side's `spawnedAt → ready` log
      // so side-by-side comparison is direct.
      if (spawnedAt !== undefined) {
        const e2eMs = Math.round(performance.now() - spawnedAt)
        // eslint-disable-next-line no-console
        console.info(
          `%c[Alacritty] ready ${terminalId} e2e=${e2eMs}ms`,
          'color:#ff0',
        )
        requestAnimationFrame(() => {
          setTimeout(() => {
            const paintMs = Math.round(performance.now() - spawnedAt)
            // eslint-disable-next-line no-console
            console.info(
              `%c[Alacritty] ${terminalId} first-paint≈${paintMs}ms (Cmd+T → cursor visible)`,
              'color:#ff0;font-weight:bold',
            )
          }, 0)
        })
      }

      terminalSetFocus(terminalId, true).catch((e) => console.warn('[terminal]', e))

      // Parity instrumentation with Kessel's SessionStreamView.
      // first-frame = first grid update received.
      // tui-alt-screen = when the TermMode alt-screen bit flips
      //   (ALT_SCREEN = 1 << 7 in alacritty_terminal's TermMode).
      // tui-ready = earliest of: alt-screen enter, bracketed-paste
      //   on (1 << 4), or app-cursor on (1 << 1). Works uniformly
      //   for Claude (no alt-screen, emits bracketed-paste) and
      //   vim/htop (alt-screen). Matches the Kessel-side predicate.
      //
      // TermMode bit references:
      //   APP_CURSOR       = 1 << 1
      //   APP_KEYPAD       = 1 << 2
      //   BRACKETED_PASTE  = 1 << 4
      //   ALT_SCREEN       = 1 << 7
      const MODE_APP_CURSOR = 1 << 1
      const MODE_BRACKETED_PASTE = 1 << 4
      const ALT_SCREEN_BIT = 1 << 7
      let firstFrameAt: number | null = null
      let altScreenAt: number | null = null
      let tuiReadyAt: number | null = null
      let prevMode = 0

      // Listen for grid updates (DOM text rendering)
      unlistenGrid = await listen<GridUpdate>(`terminal:grid:${terminalId}`, (event) => {
        const payload = event.payload
        if (firstFrameAt === null && spawnedAt !== undefined) {
          firstFrameAt = performance.now()
          const ms = Math.round(firstFrameAt - spawnedAt)
          // eslint-disable-next-line no-console
          console.info(
            `%c[Alacritty] ${terminalId} first-frame=${ms}ms (Cmd+T → shell/tui emit)`,
            'color:#ff0',
          )
        }
        // Detect alt-screen enter.
        if (
          altScreenAt === null &&
          spawnedAt !== undefined &&
          (payload.mode & ALT_SCREEN_BIT) !== 0
        ) {
          altScreenAt = performance.now()
          const ms = Math.round(altScreenAt - spawnedAt)
          // eslint-disable-next-line no-console
          console.info(
            `%c[Alacritty] ${terminalId} tui-alt-screen=${ms}ms`,
            'color:#ff0',
          )
        }
        // tui-ready fires on the TRANSITION of any "TUI is active"
        // bit from 0 → 1. ALT_SCREEN, BRACKETED_PASTE, APP_CURSOR
        // each indicate the shell/TUI has set up its session and
        // is ready for user interaction. First one to flip wins.
        if (tuiReadyAt === null && spawnedAt !== undefined) {
          const watchedBits =
            ALT_SCREEN_BIT | MODE_BRACKETED_PASTE | MODE_APP_CURSOR
          const newlySet = payload.mode & watchedBits & ~prevMode
          if (newlySet !== 0) {
            tuiReadyAt = performance.now()
            const ms = Math.round(tuiReadyAt - spawnedAt)
            const label =
              (newlySet & ALT_SCREEN_BIT) !== 0
                ? 'alt_screen'
                : (newlySet & MODE_BRACKETED_PASTE) !== 0
                  ? 'bracketed_paste'
                  : 'app_cursor'
            // eslint-disable-next-line no-console
            console.info(
              `%c[Alacritty] ${terminalId} tui-ready=${ms}ms (${label} ON → TUI interactive)`,
              'color:#ff0;font-weight:bold',
            )
          }
        }
        prevMode = payload.mode
        scheduleRender(payload)
        useActiveAgentsStore.getState().recordOutput(terminalId)
      })

      unlistenExit = await listen<{ exitCode: number }>(`terminal:exit:${terminalId}`, (event) => {
        onExit?.(event.payload.exitCode)
      })

      // Listen for terminal title changes (e.g. Claude chat names).
      // Working-state detection now lives in the viewport scan inside
      // applyGridUpdate; the title is only a fast-idle hint — when
      // Claude / similar tools finish (or the user hits Esc) they
      // restore the ✳-family idle prefix on the title, and we can clear
      // the spinner immediately instead of waiting for the grid
      // idle-grace window.
      unlistenTitle = await listen<string>(`terminal:title:${terminalId}`, (event) => {
        const raw = event.payload ?? ''
        const isIdleMarker = /^[*✱✲✳✴✵✶✷✸✹⚹⁎∗※]/.test(raw)
        if (isIdleMarker) {
          lastSeenWorkingAtRef.current = 0
          useActiveAgentsStore.getState().recordTitleActivity(terminalId, false)
        }

        const newTitle = raw.replace(/^[\u2800-\u28FF*✱✲✳✴✵✶✷✸✹⚹⁎∗※·•●◦‣⏺]\s*/g, '').trim()
        if (newTitle && tabId) {
          useTabsStore.getState().setTabTitle(tabId, newTitle)
        }
      })

      unlistenDrop = await listen<{ paths: string[]; position: { x: number; y: number } }>(
        'tauri://drag-drop',
        (event) => {
          const { paths, position } = event.payload
          if (!paths || paths.length === 0 || !ptyIdRef.current) return

          // tauri://drag-drop is window-level: every terminal in the window
          // receives it. Hit-test the drop position against this terminal's
          // container so split-column layouts don't paste into every pane.
          if (!position) return
          const el = document.elementFromPoint(position.x, position.y)
          if (!el) return
          // File tree handles its own drops
          if ((el as HTMLElement).closest?.('[data-path]')) return
          // Only accept if the drop landed inside *this* terminal's container
          if (!containerRef.current?.contains(el)) return

          // REMOTE host (K2 Connect): the dropped `paths` are LOCAL paths
          // with no bytes on the daemon. Upload them to the workspace's
          // `.k2so/downloads/` then inject the returned REMOTE paths so the
          // agent can resolve a file that actually exists on the host. The
          // payload is built with this view's existing buildDropPayload
          // (same shell-quote + bracketed-paste logic) over remote paths.
          if (useConnectHostStore.getState().activeHost !== 'local') {
            const pty = ptyIdRef.current
            void executeRemoteDrop(
              paths,
              { kind: 'terminal' },
              { workspacePath: cwd },
              buildDropPayload,
            ).then((payload) => {
              if (payload && pty) terminalWrite(pty, payload)
            })
            return
          }

          // LOCAL host — unchanged: paste the local paths directly (images
          // get bracketed-paste wrapping so Claude Code's image detector
          // fires).
          terminalWrite(ptyIdRef.current, buildDropPayload(paths))
        }
      )
    }

    setup().catch((err) => console.error('[AlacrittyTerminalView] Setup failed:', err))

    return () => {
      mounted = false
      unlistenGrid?.()
      unlistenExit?.()
      unlistenTitle?.()
      unlistenDrop?.()
    }
  }, [terminalId, cwd, command]) // eslint-disable-line react-hooks/exhaustive-deps

  // ── Resize ─────────────────────────────────────────────────────────

  useEffect(() => {
    const container = containerRef.current
    if (!container || !created) return

    const doResize = () => {
      const { cols, rows } = calculateDimensions()
      if (cols <= 0 || rows <= 0) return
      const colDiff = Math.abs(cols - lastColsRef.current)
      const rowDiff = Math.abs(rows - lastRowsRef.current)
      if (colDiff <= 2 && rowDiff <= 2 && lastColsRef.current > 0) return
      lastColsRef.current = cols
      lastRowsRef.current = rows
      if (ptyIdRef.current) {
        terminalResize(ptyIdRef.current, cols, rows).catch((e) => console.warn('[terminal]', e))
      }
    }

    // ResizeObserver for immediate response to container changes
    let resizeTimer: ReturnType<typeof setTimeout>
    const observer = new ResizeObserver(() => {
      clearTimeout(resizeTimer)
      resizeTimer = setTimeout(doResize, 80)
    })
    observer.observe(container)

    // Polling fallback (500ms) — catches cases ResizeObserver misses,
    // like background PTYs connecting to a new container
    const pollInterval = setInterval(doResize, 500)

    return () => { clearTimeout(resizeTimer); observer.disconnect(); clearInterval(pollInterval) }
  }, [created, calculateDimensions])

  // ── Focus tracking ─────────────────────────────────────────────────

  const handleFocus = useCallback(() => {
    if (ptyIdRef.current) terminalSetFocus(ptyIdRef.current, true).catch((e) => console.warn('[terminal]', e))
  }, [])
  const handleBlur = useCallback(() => {
    if (ptyIdRef.current) terminalSetFocus(ptyIdRef.current, false).catch((e) => console.warn('[terminal]', e))
  }, [])

  // ── Keyboard ───────────────────────────────────────────────────────

  const handleKeyDown = useCallback((e: React.KeyboardEvent) => {
    if (!ptyIdRef.current) return
    const ne = e.nativeEvent

    if (naturalTextEditing) {
      const seq = naturalTextEditingSequence(ne)
      if (seq) {
        e.preventDefault(); e.stopPropagation()
        terminalWrite(ptyIdRef.current, seq)
        return
      }
    }

    const data = keyEventToSequence(ne, termModeRef.current)
    if (data) {
      e.preventDefault(); e.stopPropagation()
      terminalWrite(ptyIdRef.current, data)
    }
  }, [naturalTextEditing])

  // ── Paste ──────────────────────────────────────────────────────────

  const handlePaste = useCallback((e: React.ClipboardEvent) => {
    if (!ptyIdRef.current) return
    const text = e.clipboardData.getData('text')
    e.preventDefault()

    // Finder's CMD+C copies file references via NSFilenamesPboardType, which
    // WKWebView doesn't expose to the web clipboard API. Query the native
    // pasteboard — if file paths are present, paste them shell-escaped to
    // match drag-drop behavior.
    daemonCliGet<string[]>('fs/clipboard-paths').then((paths) => {
      if (!ptyIdRef.current) return
      if (paths && paths.length > 0) {
        terminalWrite(ptyIdRef.current, buildDropPayload(paths))
        return
      }
      if (!text) return
      const MAX_PASTE_BYTES = 10 * 1024 * 1024
      if (text.length > MAX_PASTE_BYTES) {
        useToastStore.getState().addToast(
          `Paste too large (${(text.length / 1024 / 1024).toFixed(1)}MB, max 10MB)`,
          'error'
        )
        return
      }
      terminalWrite(ptyIdRef.current, text)
    }).catch(() => {
      if (!text || !ptyIdRef.current) return
      terminalWrite(ptyIdRef.current, text)
    })
  }, [])

  // ── Scroll — throttled + accumulated ─────────────────────────────────
  // macOS trackpad sends dozens of pixel-delta wheel events per swipe.
  // We accumulate the pixel deltas and send a line-based scroll IPC
  // at most every 50ms to avoid flooding.

  const scrollAccumRef = useRef(0)
  const scrollTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  const handleWheel = useCallback((e: React.WheelEvent) => {
    if (!ptyIdRef.current) return
    wheelEventCountRef.current++

    // Accumulate pixel deltas
    scrollAccumRef.current += e.deltaY

    // If no pending flush, schedule one
    if (!scrollTimerRef.current) {
      scrollTimerRef.current = setTimeout(() => {
        scrollTimerRef.current = null
        const accum = scrollAccumRef.current
        scrollAccumRef.current = 0
        if (accum === 0 || !ptyIdRef.current) return

        // Convert accumulated pixels to lines (1 line ≈ cell height, min 20px)
        const lineHeight = cellMetricsRef.current.height || 20
        const lines = Math.round(accum / lineHeight)
        const delta = -lines
        if (delta !== 0) {
          terminalScroll(ptyIdRef.current, delta).catch((e) => console.warn('[terminal]', e))
        }
      }, 50)
    }
  }, [])

  // ── Drag state tracking (for cursor style) ─────────────────────────

  const [isDragging, setIsDragging] = useState(false)
  useEffect(() => {
    const checkDrag = (): void => { setIsDragging(isFileDragActive()) }
    // Poll drag state on mousemove (lightweight — only sets state on change)
    const handler = (): void => { checkDrag() }
    document.addEventListener('mousemove', handler)
    document.addEventListener('mouseup', handler)
    return () => {
      document.removeEventListener('mousemove', handler)
      document.removeEventListener('mouseup', handler)
    }
  }, [])

  // ── Drag and drop ───────────────────────────────────────────────────

  const handleDragOver = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    e.stopPropagation()
  }, [])

  const handleDrop = useCallback((e: React.DragEvent) => {
    e.preventDefault()
    e.stopPropagation()
    if (!ptyIdRef.current) return

    // Handle files dropped from Finder
    const files = e.dataTransfer.files
    if (files.length > 0) {
      const paths: string[] = []
      for (let i = 0; i < files.length; i++) {
        // Electron/Tauri expose the full path via .path property
        const filePath = (files[i] as any).path
        if (filePath) paths.push(filePath)
      }
      if (paths.length > 0) {
        // REMOTE host: these are local file paths — upload + inject the
        // remote paths (same router as the tauri://drag-drop path). This
        // DOM handler is a rare fallback under Tauri (drag-drop is normally
        // intercepted natively), but keep it host-aware for parity.
        if (useConnectHostStore.getState().activeHost !== 'local') {
          const pty = ptyIdRef.current
          void executeRemoteDrop(
            paths,
            { kind: 'terminal' },
            { workspacePath: cwd },
            buildDropPayload,
          ).then((payload) => {
            if (payload && pty) terminalWrite(pty, payload)
          })
        } else {
          terminalWrite(ptyIdRef.current, buildDropPayload(paths))
        }
      }
    }

    // Handle text drops
    const text = e.dataTransfer.getData('text/plain')
    if (text && files.length === 0) {
      terminalWrite(ptyIdRef.current, text)
    }
  }, [cwd])

  // ── Link detection: Cmd key tracking ────────────────────────────────

  useEffect(() => {
    const handleKeyDown = (e: KeyboardEvent): void => {
      if (e.key === 'Meta') cmdHeldRef.current = true
    }
    const handleKeyUp = (e: KeyboardEvent): void => {
      if (e.key === 'Meta') {
        cmdHeldRef.current = false
        if (linkClickMode === 'cmd-click') setHoveredLink(null)
      }
    }
    const handleBlur = (): void => {
      cmdHeldRef.current = false
      setHoveredLink(null)
    }
    document.addEventListener('keydown', handleKeyDown)
    document.addEventListener('keyup', handleKeyUp)
    window.addEventListener('blur', handleBlur)
    return () => {
      document.removeEventListener('keydown', handleKeyDown)
      document.removeEventListener('keyup', handleKeyUp)
      window.removeEventListener('blur', handleBlur)
    }
  }, [linkClickMode])

  // ── Link detection: mouse hover ────────────────────────────────────

  const handleMouseMove = useCallback((e: React.MouseEvent) => {
    // In cmd-click mode, only detect when Cmd is held
    if (linkClickMode === 'cmd-click' && !cmdHeldRef.current) {
      if (hoveredLink) setHoveredLink(null)
      return
    }

    // Throttle: skip if mouse moved < 4px and < 80ms since last detection
    const now = Date.now()
    const dx = e.clientX - lastDetectPosRef.current.x
    const dy = e.clientY - lastDetectPosRef.current.y
    const dist = dx * dx + dy * dy
    if (dist < 16 && now - lastDetectTimeRef.current < 80) return
    lastDetectPosRef.current = { x: e.clientX, y: e.clientY }
    lastDetectTimeRef.current = now

    const viewport = viewportRef.current
    if (!viewport) return
    const rect = viewport.getBoundingClientRect()
    const { width: cw, height: ch } = cellMetricsRef.current
    if (cw === 0 || ch === 0) return

    const row = Math.floor((e.clientY - rect.top) / ch)
    const col = Math.floor((e.clientX - rect.left) / cw)
    const line = linesRef.current.get(row)
    if (!line || !line.text) {
      if (hoveredLink) setHoveredLink(null)
      return
    }

    const links = detectLinks(line.text, cwd)
    const hit = links.find((l) => col >= l.start && col < l.end)
    if (hit) {
      if (!hoveredLink || hoveredLink.row !== row || hoveredLink.link.start !== hit.start) {
        setHoveredLink({ row, link: hit })
      }
    } else if (hoveredLink) {
      setHoveredLink(null)
    }
  }, [linkClickMode, hoveredLink, cwd])

  const handleMouseLeaveViewport = useCallback(() => {
    if (hoveredLink) setHoveredLink(null)
  }, [hoveredLink])

  // ── Link detection: mouse-down/up validation ────────────────────────

  const handleLinkMouseDown = useCallback((e: React.MouseEvent) => {
    // Store the link at mouse-down so we can validate on click
    mouseDownLinkRef.current = hoveredLink?.link ?? null
  }, [hoveredLink])

  // ── Link detection: click handler ──────────────────────────────────

  const handleLinkClick = useCallback((e: React.MouseEvent) => {
    // In cmd-click mode, require Cmd. In click mode, just need hoveredLink.
    if (linkClickMode === 'cmd-click' && !e.metaKey) return
    if (!hoveredLink) return

    // Validate: mouse-down must have been on the same link (prevents drag-to-link false clicks)
    const downLink = mouseDownLinkRef.current
    mouseDownLinkRef.current = null
    if (!downLink || downLink.start !== hoveredLink.link.start || downLink.target !== hoveredLink.link.target) return

    const viewport = viewportRef.current
    if (!viewport) return
    const rect = viewport.getBoundingClientRect()
    const { width: cw, height: ch } = cellMetricsRef.current
    if (cw === 0 || ch === 0) return

    const row = Math.floor((e.clientY - rect.top) / ch)
    const col = Math.floor((e.clientX - rect.left) / cw)
    const line = linesRef.current.get(row)
    if (!line || !line.text) return

    const links = detectLinks(line.text, cwd)
    const clicked = links.find((l) => col >= l.start && col < l.end)
    if (!clicked) return

    e.preventDefault()
    e.stopPropagation()

    if (clicked.type === 'url') {
      daemonCliPost('fs/open-external', { target: clicked.target }).catch((e: unknown) => console.warn('[terminal-link]', e))
    } else if (clicked.type === 'file' && clicked.filePath) {
      const tabsStore = useTabsStore.getState()
      const openInSplit = useTerminalSettingsStore.getState().openLinksInSplitPane

      // If split pane setting is on, try to open in the sibling pane
      if (openInSplit && tabId && paneGroupId) {
        const tab = tabsStore.tabs.find((t) => t.id === tabId)
        if (tab && tab.paneGroups.size > 1) {
          // Find a pane group that isn't the terminal's pane
          const siblingId = [...tab.paneGroups.keys()].find((id) => id !== paneGroupId)
          if (siblingId) {
            tabsStore.openFileInPaneGroup(tabId, siblingId, clicked.filePath)
            return
          }
        }
      }

      tabsStore.openFileInNewTab(clicked.filePath)
    }
  }, [linkClickMode, hoveredLink, cwd, tabId, paneGroupId])

  // ── Cursor blink ───────────────────────────────────────────────────

  useEffect(() => {
    const interval = setInterval(() => setCursorBlinkVisible((v) => !v), 530)
    return () => clearInterval(interval)
  }, [])

  // ── Auto-focus ─────────────────────────────────────────────────────

  useEffect(() => { containerRef.current?.focus() }, [created])

  // Re-focus terminal when window regains focus (e.g., switching back from Messages app).
  // Only re-focuses if this terminal's container was the last focused element before blur.
  useEffect(() => {
    const container = containerRef.current
    if (!container) return

    let wasFocused = false

    const handleWindowBlur = () => {
      wasFocused = document.activeElement === container || container.contains(document.activeElement)
    }

    const handleWindowFocus = () => {
      if (wasFocused && ptyIdRef.current) {
        // Small delay to let the window settle before grabbing focus
        requestAnimationFrame(() => {
          container.focus()
        })
      }
    }

    window.addEventListener('blur', handleWindowBlur)
    window.addEventListener('focus', handleWindowFocus)
    return () => {
      window.removeEventListener('blur', handleWindowBlur)
      window.removeEventListener('focus', handleWindowFocus)
    }
  }, [created])

  // ── Render ─────────────────────────────────────────────────────────

  const { width: cellW, height: cellH } = cellMetrics
  // Two cursor modes:
  // 1. App shows cursor (vis:Y, e.g. zsh) — we render our overlay block cursor
  // 2. App hides cursor (vis:N, e.g. Claude Code) — app draws its own via INVERSE cells
  const showCursor = gridState.cursorVisible && gridState.displayOffset === 0

  // Build row elements from the lines map
  const rowElements: React.JSX.Element[] = []
  for (let r = 0; r < gridState.rows; r++) {
    const line = linesRef.current.get(r)
    rowElements.push(
      <div
        key={r}
        style={{
          height: cellH || undefined,
          whiteSpace: 'pre',
          overflow: 'hidden',
          lineHeight: cellH ? `${cellH}px` : undefined,
        }}
      >
        {line ? renderLineSpans(line) : '\u00A0'}
      </div>
    )
  }

  return (
    <div
      ref={containerRef}
      className="h-full w-full bg-[#0a0a0a] focus:outline-none overflow-hidden"
      tabIndex={0}
      onKeyDown={handleKeyDown}
      onPaste={handlePaste}
      onWheel={handleWheel}
      onFocus={handleFocus}
      onBlur={handleBlur}
      onDragOver={handleDragOver}
      onDrop={handleDrop}
      data-terminal-id={terminalId}
      data-terminal-container=""
      data-terminal-visible="true"
      style={{ cursor: isDragging ? 'default' : hoveredLink ? 'pointer' : 'text', position: 'relative' }}
    >
      <div
        ref={viewportRef}
        onMouseMove={handleMouseMove}
        onMouseDown={handleLinkMouseDown}
        onMouseLeave={handleMouseLeaveViewport}
        onClick={handleLinkClick}
        style={{
          fontFamily: "'MesloLGM Nerd Font', 'MesloLGM Nerd Font Mono', Menlo, Monaco, 'Courier New', monospace",
          fontSize: `${fontSize}px`,
          color: colorToCSS(DEFAULT_FG),
          background: colorToCSS(DEFAULT_BG),
          fontVariantLigatures: 'none',
          width: '100%',
          height: '100%',
          overflow: 'hidden',
        }}
      >
        {rowElements}
      </div>

      {/* Link underline overlay */}
      {hoveredLink && cellW > 0 && cellH > 0 && (
        <div
          style={{
            position: 'absolute',
            left: hoveredLink.link.start * cellW,
            top: hoveredLink.row * cellH + cellH - 1,
            width: (hoveredLink.link.end - hoveredLink.link.start) * cellW,
            height: 1,
            background: hoveredLink.link.type === 'url' ? '#6cb6ff' : '#8bdb81',
            pointerEvents: 'none',
            zIndex: 10,
            opacity: 0.8,
          }}
        />
      )}

      {/* Cursor overlay — block style, matches iTerm2/Alacritty default */}
      {showCursor && cellW > 0 && cellH > 0 && (
        <div
          style={{
            position: 'absolute',
            left: gridState.cursorCol * cellW,
            top: gridState.cursorRow * cellH,
            width: gridState.cursorShape === 'bar' ? 2.5 : cellW,
            height: gridState.cursorShape === 'underline' ? 3 : cellH,
            marginTop: gridState.cursorShape === 'underline' ? cellH - 3 : 0,
            background: gridState.cursorVisible
              ? 'rgba(240, 240, 240, 0.9)'   // App says cursor visible — bright white
              : 'rgba(240, 240, 240, 0.75)',  // App hid cursor — slightly dimmer but clearly visible
            pointerEvents: 'none',
            zIndex: 10,
          }}
        />
      )}

      {/* Debug overlay — only in dev mode. Renderer-label first so
       *  it's obvious at a glance which pane is Alacritty vs Kessel
       *  (the two are otherwise visually near-identical). */}
      {import.meta.env.DEV && (
        <div style={{
          position: 'absolute', top: 2, right: 2, padding: '2px 6px',
          background: 'rgba(0,0,0,0.8)', color: '#0f0', fontSize: '10px',
          fontFamily: 'monospace', zIndex: 999, pointerEvents: 'none',
          borderRadius: '3px',
        }}>
          <strong style={{ color: '#ff0' }}>Alacritty (Legacy)</strong> · frames:{debugInfo.frames} offset:{debugInfo.offset} wheel:{debugInfo.wheel} cells:{gridState.cols}x{gridState.rows} cursor:{gridState.cursorCol},{gridState.cursorRow} vis:{gridState.cursorVisible?'Y':'N'} cell:{cellW.toFixed(1)}x{cellH}
        </div>
      )}
    </div>
  )
}
