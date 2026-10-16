// The Changes tab's diff viewer (C3): one file, or every changed file
// stacked ("view all") with sticky headers and lazy loading. Its own small
// toolbar: word wrap, collapse unchanged, previous/next change (F7 /
// Shift+F7, or Alt+Down / Alt+Up). Inline vs split and single vs all come
// from C2 (`mode` / `view`). Loaded lazily (see ChangesDiffViewer in
// index.ts) so CodeMirror's merge code is fetched only when a Changes tab
// shows a diff.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useLazySides } from './useLazySides'
import { useDiffPrefs, type DiffPrefsState } from './prefs'
import { DiffFileBody, StatusBadge } from './DiffFileBody'
import { rowTitle, viewFromSides, type DiffContent } from './diffQuery'
import { buildStops, currentChangeNumber, stepChange, type ChangeCursor, type ChangeStop, type FileStops } from './navigation'
import type { MergeDiffApi } from './MergeDiffEditor'
import { diffRowKey, type ChangesDiffFile, type ChangesDiffViewerProps } from './types'

// ── Navigation across files ─────────────────────────────────────────────

interface Nav {
  /** A file (by row key) says where its changes are. */
  report: (key: string, stops: FileStops) => void
  apiRef: (key: string) => React.MutableRefObject<MergeDiffApi | null>
  setCursor: (key: string, pos: number) => void
  go: (dir: 1 | -1) => void
  counter: { current: number; total: number; more: boolean }
}

/**
 * Next/previous change over `keys` (row keys in display order). State is
 * keyed by row, not index, so a file joining or leaving the list keeps
 * every other file's stops and editor handle.
 */
function useChangeNav(keys: readonly string[], reveal: (file: number) => void): Nav {
  const stopsRef = useRef(new Map<string, FileStops>())
  const apisRef = useRef(new Map<string, React.MutableRefObject<MergeDiffApi | null>>())
  const [cursorAt, setCursorAt] = useState<{ key: string; pos: number } | null>(null)
  const [version, setVersion] = useState(0)
  const keysRef = useRef(keys)
  keysRef.current = keys
  const cursorRef = useRef(cursorAt)
  cursorRef.current = cursorAt

  const report = useCallback((key: string, stops: FileStops) => {
    stopsRef.current.set(key, stops)
    setVersion((v) => v + 1)
  }, [])

  const apiRef = useCallback((key: string) => {
    let r = apisRef.current.get(key)
    if (!r) {
      r = { current: null }
      apisRef.current.set(key, r)
    }
    return r
  }, [])

  const setCursor = useCallback((key: string, pos: number) => setCursorAt({ key, pos }), [])

  const snapshot = useCallback((): { stops: ChangeStop[]; cursor: ChangeCursor | null } => {
    const ks = keysRef.current
    const stops = buildStops(ks.map((k) => stopsRef.current.get(k) ?? { state: 'pending' }))
    const c = cursorRef.current
    const file = c ? ks.indexOf(c.key) : -1
    return { stops, cursor: c && file >= 0 ? { file, pos: c.pos } : null }
  }, [])

  const go = useCallback(
    (dir: 1 | -1) => {
      const { stops, cursor } = snapshot()
      const i = stepChange(stops, cursor, dir)
      if (i < 0) return
      const stop = stops[i]
      const key = keysRef.current[stop.file]
      const api = apisRef.current.get(key)?.current
      if (stop.pending || !api) {
        // Not rendered yet: bring it on screen (that loads it); the next
        // press lands on its first change.
        reveal(stop.file)
        setCursorAt({ key, pos: -1 })
        return
      }
      api.goTo(stop.pos)
      setCursorAt({ key, pos: stop.pos })
    },
    [snapshot, reveal],
  )

  const keysJoined = keys.join('\n')
  const counter = useMemo(() => {
    const { stops, cursor } = snapshot()
    const real = stops.filter((s) => !s.pending)
    return {
      current: currentChangeNumber(real, cursor),
      total: real.length,
      more: real.length !== stops.length,
    }
    // `version` bumps when a file reports; `keysJoined` when the list moves.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [snapshot, cursorAt, version, keysJoined])

  return { report, apiRef, setCursor, go, counter }
}

/** What a file contributes to navigation once its content is known. */
function stopsForContent(content: DiffContent | null, error: string | null): FileStops | null {
  if (error) return { state: 'none' }
  if (!content) return null
  return content.kind === 'text' ? null : { state: 'none' } // text reports via onChunks
}

// ── Toolbar ─────────────────────────────────────────────────────────────

function ToolButton(props: {
  label: string
  title: string
  active?: boolean
  disabled?: boolean
  onClick: () => void
  testId?: string
}): React.JSX.Element {
  return (
    <button
      type="button"
      className={`px-1.5 py-0.5 text-[11px] leading-4 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed border border-transparent hover:border-[var(--color-border)] ${
        props.active ? 'text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)]' : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
      }`}
      title={props.title}
      aria-pressed={props.active}
      disabled={props.disabled}
      onClick={props.onClick}
      data-testid={props.testId}
    >
      {props.label}
    </button>
  )
}

function Toolbar(props: {
  prefs: DiffPrefsState
  nav: Nav
  left: React.ReactNode
}): React.JSX.Element {
  const { prefs, nav } = props
  const { current, total, more } = nav.counter
  return (
    <div className="flex items-center gap-2 px-3 py-1 border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] min-h-[30px]">
      <div className="flex items-center gap-1.5 min-w-0 flex-1">{props.left}</div>
      <div className="flex items-center gap-0.5 shrink-0">
        <ToolButton label="Wrap" title="Word wrap" active={prefs.wrap} onClick={() => prefs.set({ wrap: !prefs.wrap })} testId="diff-wrap" />
        <ToolButton
          label="Collapse"
          title="Hide unchanged lines"
          active={prefs.collapse}
          onClick={() => prefs.set({ collapse: !prefs.collapse })}
          testId="diff-collapse"
        />
        <span className="w-px h-3.5 mx-1 bg-[var(--color-border)]" />
        <ToolButton label="↑" title="Previous change (Shift+F7)" disabled={total === 0 && !more} onClick={() => nav.go(-1)} testId="diff-prev" />
        <ToolButton label="↓" title="Next change (F7)" disabled={total === 0 && !more} onClick={() => nav.go(1)} testId="diff-next" />
        <span className="text-[10px] tabular-nums text-[var(--color-text-muted)] min-w-[44px] text-right" data-testid="diff-counter">
          {total === 0 && !more ? 'No changes' : `${current} / ${total}${more ? '+' : ''}`}
        </span>
      </div>
    </div>
  )
}

function Counts({ row }: { row: ChangesDiffFile }): React.JSX.Element | null {
  if (row.additions == null && row.deletions == null) return null
  return (
    <span className="text-[10px] tabular-nums shrink-0">
      <span className="text-[var(--color-status-ok-soft)]">+{row.additions ?? 0}</span>{' '}
      <span className="text-[var(--color-status-error-soft)]">−{row.deletions ?? 0}</span>
    </span>
  )
}

function FileTitle({ row, onOpenFile }: { row: ChangesDiffFile; onOpenFile?: (p: string) => void }): React.JSX.Element {
  const canOpen = !!onOpenFile && row.status !== 'D' && !row.dir
  return (
    <>
      <StatusBadge status={row.status} />
      <span className="text-xs text-[var(--color-text-primary)] truncate" title={rowTitle(row)} data-testid="diff-file-title">
        {rowTitle(row)}
      </span>
      <Counts row={row} />
      {canOpen && (
        <button
          type="button"
          className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer shrink-0"
          onClick={(e) => {
            e.stopPropagation()
            onOpenFile(row.path)
          }}
          title="Open in editor"
        >
          Open
        </button>
      )}
    </>
  )
}

// ── Keys ────────────────────────────────────────────────────────────────

/** F7 / Shift+F7 (Orca, VS Code) and Alt+Down / Alt+Up: next / previous
 *  change. Returns the direction, or 0 when the key isn't ours. */
export function navKeyDirection(e: Pick<KeyboardEvent, 'key' | 'shiftKey' | 'altKey' | 'metaKey' | 'ctrlKey'>): 1 | -1 | 0 {
  if (e.metaKey || e.ctrlKey) return 0
  if (e.key === 'F7' && !e.altKey) return e.shiftKey ? -1 : 1
  if (e.altKey && !e.shiftKey && e.key === 'ArrowDown') return 1
  if (e.altKey && !e.shiftKey && e.key === 'ArrowUp') return -1
  return 0
}

// ── Single file ─────────────────────────────────────────────────────────

const noReveal = (): void => {}

function SingleDiff(props: ChangesDiffViewerProps & { prefs: DiffPrefsState }): React.JSX.Element {
  const { file, sides, base, onOpenFile, prefs } = props
  const state = useMemo(
    () => (file ? viewFromSides(file, sides, base) : { loading: false, refreshing: false, content: null, error: null }),
    [file, sides, base],
  )
  const fileKey = file ? diffRowKey(file) : ''
  const keys = useMemo(() => (fileKey ? [fileKey] : []), [fileKey])
  const nav = useChangeNav(keys, noReveal)
  const navStops = stopsForContent(state.content, state.error)
  useEffect(() => {
    if (navStops && fileKey) nav.report(fileKey, navStops)
  }, [navStops?.state, fileKey, nav.report]) // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div
      className="h-full flex flex-col overflow-hidden"
      onKeyDownCapture={(e) => {
        const dir = navKeyDirection(e)
        if (!dir) return
        e.preventDefault()
        e.stopPropagation()
        nav.go(dir)
      }}
      data-testid="changes-diff-single"
    >
      <Toolbar
        prefs={prefs}
        nav={nav}
        left={
          file ? (
            <>
              <FileTitle row={file} onOpenFile={onOpenFile} />
              {state.refreshing && <span className="text-[10px] text-[var(--color-text-muted)]">updating…</span>}
            </>
          ) : null
        }
      />
      <div className="flex-1 min-h-0 overflow-hidden">
        {file ? (
          <DiffFileBody
            row={file}
            state={state}
            layout={props.mode}
            wrap={prefs.wrap}
            collapse={prefs.collapse}
            fill
            onOpenFile={onOpenFile}
            onChunks={(starts) => nav.report(fileKey, { state: 'chunks', starts })}
            onCursor={(pos) => nav.setCursor(fileKey, pos)}
            apiRef={nav.apiRef(fileKey)}
          />
        ) : (
          <div className="h-full flex items-center justify-center text-xs text-[var(--color-text-muted)]" data-testid="diff-empty">
            Select a file to see its changes
          </div>
        )}
      </div>
    </div>
  )
}

// ── View all ────────────────────────────────────────────────────────────

const LAZY_MARGIN = '800px 0px'

function FileSection(props: {
  row: ChangesDiffFile
  scrollRoot: React.RefObject<HTMLDivElement | null>
  forced: boolean
  common: ChangesDiffViewerProps
  prefs: DiffPrefsState
  nav: Nav
  sectionRef: (el: HTMLElement | null) => void
  selected: boolean
}): React.JSX.Element {
  const { row, common, prefs, nav, forced } = props
  const rowKey = diffRowKey(row)
  const ref = useRef<HTMLElement | null>(null)
  const [seen, setSeen] = useState(false)
  const [hidden, setHidden] = useState(false)

  useEffect(() => {
    const el = ref.current
    if (!el || seen) return
    if (typeof IntersectionObserver === 'undefined') {
      setSeen(true)
      return
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) setSeen(true)
      },
      { root: props.scrollRoot.current, rootMargin: LAZY_MARGIN },
    )
    io.observe(el)
    return () => io.disconnect()
  }, [seen, props.scrollRoot])

  const enabled = seen || forced
  const state = useLazySides({ row, base: common.base, rev: common.rev, enabled, loadSides: common.loadSides })
  const navStops = hidden ? ({ state: 'none' } as FileStops) : stopsForContent(state.content, state.error)
  useEffect(() => {
    if (navStops) nav.report(rowKey, navStops)
  }, [navStops?.state, rowKey, nav.report]) // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <section
      ref={(el) => {
        ref.current = el
        props.sectionRef(el)
      }}
      className="border-b border-[var(--color-border)]"
      data-testid="diff-section"
      data-key={diffRowKey(row)}
    >
      <header
        className={`sticky top-0 z-10 flex items-center gap-1.5 px-3 py-1 border-b border-[var(--color-border)] cursor-pointer ${
          props.selected ? 'bg-[var(--color-bg-elevated)]' : 'bg-[var(--color-bg-surface)]'
        }`}
        onClick={() => common.onSelectFile?.(row)}
      >
        <button
          type="button"
          className="w-4 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer shrink-0"
          onClick={(e) => {
            e.stopPropagation()
            setHidden((h) => !h)
          }}
          title={hidden ? 'Show diff' : 'Hide diff'}
          aria-expanded={!hidden}
        >
          {hidden ? '▸' : '▾'}
        </button>
        <FileTitle row={row} onOpenFile={common.onOpenFile} />
      </header>
      {!hidden &&
        (enabled ? (
          <DiffFileBody
            row={row}
            state={state}
            layout={common.mode}
            wrap={prefs.wrap}
            collapse={prefs.collapse}
            fill={false}
            onOpenFile={common.onOpenFile}
            onChunks={(starts) => nav.report(rowKey, { state: 'chunks', starts })}
            onCursor={(pos) => nav.setCursor(rowKey, pos)}
            apiRef={nav.apiRef(rowKey)}
          />
        ) : (
          <div className="min-h-[120px]" data-testid="diff-placeholder" />
        ))}
    </section>
  )
}

function AllDiffs(props: ChangesDiffViewerProps & { prefs: DiffPrefsState }): React.JSX.Element {
  const { files, file, prefs } = props
  const scrollRoot = useRef<HTMLDivElement | null>(null)
  // Sections and forced loads are keyed by row key (indexes move as files
  // join or leave the list).
  const sections = useRef(new Map<string, HTMLElement>())
  const [forced, setForced] = useState<ReadonlySet<string>>(new Set())
  const keys = useMemo(() => files.map(diffRowKey), [files])
  const keysRef = useRef(keys)
  keysRef.current = keys

  const reveal = useCallback((i: number) => {
    const key = keysRef.current[i]
    if (!key) return
    setForced((s) => (s.has(key) ? s : new Set(s).add(key)))
    sections.current.get(key)?.scrollIntoView({ block: 'start' })
  }, [])
  const nav = useChangeNav(keys, reveal)

  // Follow the list's selection: scroll that file's section into view.
  const selectedKey = file ? diffRowKey(file) : null
  const selectedIndex = selectedKey ? keys.indexOf(selectedKey) : -1
  useEffect(() => {
    if (selectedIndex < 0 || !selectedKey) return
    reveal(selectedIndex)
    nav.setCursor(selectedKey, -1)
  }, [selectedKey, selectedIndex >= 0, reveal]) // eslint-disable-line react-hooks/exhaustive-deps

  const totals = useMemo(() => {
    let add = 0
    let del = 0
    for (const f of files) {
      add += f.additions ?? 0
      del += f.deletions ?? 0
    }
    return { add, del }
  }, [files])

  return (
    <div
      className="h-full flex flex-col overflow-hidden"
      onKeyDownCapture={(e) => {
        const dir = navKeyDirection(e)
        if (!dir) return
        e.preventDefault()
        e.stopPropagation()
        nav.go(dir)
      }}
      data-testid="changes-diff-all"
    >
      <Toolbar
        prefs={prefs}
        nav={nav}
        left={
          <span className="text-xs text-[var(--color-text-secondary)] truncate">
            {files.length} {files.length === 1 ? 'file' : 'files'}{' '}
            <span className="text-[10px] tabular-nums">
              <span className="text-[var(--color-status-ok-soft)]">+{totals.add}</span>{' '}
              <span className="text-[var(--color-status-error-soft)]">−{totals.del}</span>
            </span>
          </span>
        }
      />
      <div ref={scrollRoot} className="flex-1 min-h-0 overflow-auto" tabIndex={-1}>
        {files.length === 0 ? (
          <div className="h-full flex items-center justify-center text-xs text-[var(--color-text-muted)]" data-testid="diff-empty">
            No changes
          </div>
        ) : (
          files.map((row, i) => {
            const key = keys[i]
            return (
              <FileSection
                key={key}
                row={row}
                scrollRoot={scrollRoot}
                forced={forced.has(key)}
                common={props}
                prefs={prefs}
                nav={nav}
                selected={i === selectedIndex}
                sectionRef={(el) => {
                  if (el) sections.current.set(key, el)
                  else sections.current.delete(key)
                }}
              />
            )
          })
        )}
      </div>
    </div>
  )
}

// ── Entry ───────────────────────────────────────────────────────────────

export function ChangesDiffView(props: ChangesDiffViewerProps): React.JSX.Element {
  const prefs = useDiffPrefs()
  return props.view === 'all' ? <AllDiffs {...props} prefs={prefs} /> : <SingleDiff {...props} prefs={prefs} />
}
