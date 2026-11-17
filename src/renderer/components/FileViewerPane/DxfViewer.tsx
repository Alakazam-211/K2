// DXF preview for FileViewerPane: ASCII DXF text → SVG. Ported from the
// K2 Companion (`src/zen/files/DxfView.tsx`); the render part is the
// same, the gestures are desktop ones:
//   - wheel / trackpad pinch zooms around the cursor,
//   - drag pans (any mouse button),
//   - double-click or the Fit button refits.
// On the drawing model's path the left button selects and moves instead
// (dxf/edit/): panning is then the middle or right button, or Space with
// the left.
// dxf-parser and the converter (`dxfModel.ts`) are a dynamic import, so
// neither is in this chunk or the start-up bundle. The text arrives from
// FileViewerPane's normal `fs/read-file` (the room's server, so remote
// rooms work like any other text preview). When `fs/read-file` refuses
// (over its 10 MB cap, or not UTF-8: old ANSI-codepage DXFs),
// `DxfHostBytesViewer` reads the bytes instead and decodes them here.

import { memo, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { useRoom } from '@/components/Room/RoomContext'
import { isHostSwitchedError } from '@/lib/daemon-cli'
import { loadHostBinary } from '@/lib/load-host-binary'
import { useDocVersion } from './useDocVersion'
import { ViewerNote } from './ViewerNote'
import type { Bounds, DxfDrawing, DxfNode } from './dxfModel'
import type { CadChangeSource } from './dxf/cad-change-source'
import type { ModelDrawing } from './dxf/model/load'
import { LayoutsNote, NotDrawnNote, useModelSource } from './dxf/model/ModelStats'
import type { DrawingModelSource } from './dxf/model/source'
import { setPxPerUnit } from './dxf/model/lines'
import { SHEET_STYLE } from './dxf/sheet'
import { pageTextMetrics } from './dxf/text-metrics'
import { useDxfLiveChanges } from './dxf/useDxfLiveChanges'
import type { EditSource } from './dxf/edit/edit-source'
import type { SheetCtx, SheetTool } from './dxf/edit/sheet-tool'
import { useDxfEdit } from './dxf/edit/useDxfEdit'
import { framed, viewBounds } from './dxf/view-address'

type Box = { x: number; y: number; w: number; h: number }

/** Largest DXF read as bytes (past the daemon's 10 MB text cap). */
export const DXF_MAX_BYTES = 200 * 1024 * 1024

/** Zoom limits, relative to the fitted view. */
const MIN_SCALE = 0.25
const MAX_SCALE = 400
/** Pointer travel (px) before a press counts as a drag. */
const DRAG_SLOP_PX = 3

/** The fitted viewBox in SVG space (Y already flipped), 4% margin. */
export function fitBox(b: Bounds): Box {
  const w = Math.max(b.maxX - b.minX, 1e-6)
  const h = Math.max(b.maxY - b.minY, 1e-6)
  const pad = Math.max(w, h) * 0.04
  return { x: b.minX - pad, y: -b.maxY - pad, w: w + 2 * pad, h: h + 2 * pad }
}

const fmtNum = (v: number): string =>
  String(Number(Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 10 ? v.toFixed(1) : v.toFixed(2)))

function skippedLabel(skipped: Record<string, number>): string {
  return Object.entries(skipped)
    .sort((a, b) => b[1] - a[1])
    .map(([t, c]) => (c > 1 ? `${t} ×${c}` : t))
    .join(', ')
}

// `vector-effect` does not inherit from <g>: set it per shape so lines stay
// hairlines at any zoom.
const HAIR = 'non-scaling-stroke'

function renderNode(node: DxfNode, key: number): ReactNode {
  switch (node.k) {
    case 'line':
      return <line key={key} x1={node.x1} y1={node.y1} x2={node.x2} y2={node.y2} stroke={node.c} vectorEffect={HAIR} />
    case 'circle':
      return <circle key={key} cx={node.cx} cy={node.cy} r={node.r} stroke={node.c} vectorEffect={HAIR} />
    case 'path':
      return node.fill ? (
        <path key={key} d={node.d} fill={node.c} stroke={node.c} vectorEffect={HAIR} />
      ) : (
        <path key={key} d={node.d} stroke={node.c} vectorEffect={HAIR} />
      )
    case 'text':
      // Un-flip so it reads; DXF rotation is counter-clockwise.
      return (
        <text
          key={key}
          transform={`translate(${node.x} ${node.y}) scale(1 -1)${node.rot ? ` rotate(${-node.rot})` : ''}${node.wf ? ` scale(${node.wf} 1)` : ''}`}
          fontSize={node.fs}
          textAnchor={node.anchor}
          fill={node.c}
          stroke="none"
        >
          {node.lines.length === 1 ? (
            // Aligned and fit TEXT run exactly from their first point to their second.
            <tspan x={0} y={node.y0} {...(node.len ? { textLength: node.len, lengthAdjust: 'spacingAndGlyphs' } : {})}>
              {node.lines[0]}
            </tspan>
          ) : (
            node.lines.map((l, i) => (
              <tspan key={i} x={0} y={node.y0 + i * node.lh}>
                {l || ' '}
              </tspan>
            ))
          )}
        </text>
      )
    case 'group':
      return (
        <g key={key} transform={`matrix(${node.m.join(' ')})`}>
          {node.children.map(renderNode)}
        </g>
      )
  }
}

/** The drawing itself; memoised so pan/zoom never re-renders it. */
const DxfShapes = memo(function DxfShapes({ nodes }: { nodes: DxfNode[] }) {
  return (
    <g transform="scale(1 -1)" fill="none" strokeWidth={1} style={{ fontFamily: 'var(--font-ui)' }} data-testid="dxf-shapes">
      {nodes.map(renderNode)}
    </g>
  )
})

export function DxfViewer({ text, filePath, view, changes, model, edits }: {
  text: string
  /** The file: with it, each new version's change is highlighted. */
  filePath?: string
  /** Open framed to a view address (`@h:2B1`, `@layer:A-WALL`, a set); Fit still shows it all. */
  view?: string | readonly string[]
  /** Where changes come from instead of the daemon (tests, shots); null turns the highlight off. */
  changes?: CadChangeSource | null
  /** Where the drawing's model comes from instead of the daemon (tests, shots); null reads the text only. */
  model?: DrawingModelSource | null
  /** Where edits go instead of the daemon (tests, shots); null turns editing off. */
  edits?: EditSource | null
}): React.JSX.Element {
  // The model path adds its shapes, what it couldn't draw, and its paper layouts.
  const [drawing, setDrawing] = useState<(DxfDrawing & Partial<Pick<ModelDrawing, 'shapes' | 'notDrawn' | 'paper' | 'header' | 'rows' | 'version'>>) | null>(null)
  // Read again (after an edit lands, or when the drawing changed under one).
  const [reads, setReads] = useState(0)
  const loadedFor = useRef<string | null>(null)
  const viewFor = useRef<string | null>(null)
  const toolRef = useRef<SheetTool | null>(null)
  const modelSource = useModelSource(model, filePath)
  const [error, setError] = useState<string | null>(null)
  const boxRef = useRef<HTMLDivElement>(null)
  const svgRef = useRef<SVGSVGElement>(null)
  const viewRef = useRef<Box | null>(null)
  const refitRef = useRef<() => void>(() => {})
  const frameRef = useRef<(b: Bounds) => void>(() => {})

  useEffect(() => {
    let cancelled = false
    // A new version of the same file keeps its drawing up until the new one is read.
    if (loadedFor.current !== (filePath ?? '')) setDrawing(null)
    setError(null)
    // Text is sized by the UI font's measured capitals (a DXF height is the cap height).
    // The drawing comes from the server's model when it has one, else from the text.
    Promise.all([import('./dxfModel'), pageTextMetrics(), modelSource ? import('./dxf/model/load') : null])
      .then(async ([m, metrics, load]) => {
        const fromModel = load && modelSource ? await load.loadModel(modelSource, metrics).catch(() => null) : null
        const d = fromModel ?? m.buildDrawing(text, metrics)
        if (cancelled) return
        loadedFor.current = filePath ?? ''
        setDrawing(d)
      })
      .catch((e: unknown) => {
        if (!cancelled) setError(e instanceof Error ? e.message : String(e))
      })
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [text, modelSource, reads])

  const fit = drawing?.bounds ? fitBox(drawing.bounds) : null
  const viewKey = typeof view === 'string' ? view : view?.join(' ')
  const padUnits = (): number => {
    const r = boxRef.current?.getBoundingClientRect()
    const v = viewRef.current
    return r && v && r.width && r.height ? 5 * Math.max(v.w / r.width, v.h / r.height) : 0
  }
  // An undo that finds the drawing can't be changed here opens it view only (editing is set up below).
  const viewOnlyRef = useRef<(words: string) => void>(() => {})
  const whoIsRef = useRef<(pid: string) => Promise<{ name: string | null; self: boolean }>>(async () => ({ name: null, self: false }))
  const live = useDxfLiveChanges({
    drawing,
    filePath,
    changes,
    frame: (b) => frameRef.current(b),
    padUnits,
    onVersion: () => setReads((n) => n + 1),
    onOff: (words) => viewOnlyRef.current(words),
    whoIs: (pid) => whoIsRef.current(pid),
  })
  const modelPart = drawing?.header && drawing.rows && drawing.version !== undefined ? drawing : null
  const editDrawing = useMemo(
    () => (modelPart ? { header: modelPart.header!, rows: modelPart.rows!, version: modelPart.version! } : null),
    [modelPart],
  )
  const editing = useDxfEdit({ drawing: editDrawing, filePath, edits, live, reload: () => setReads((n) => n + 1), padUnits })
  toolRef.current = editing.tool
  viewOnlyRef.current = editing.goViewOnly
  whoIsRef.current = editing.whoIs

  // Gestures write the viewBox straight to the DOM: no React render per frame.
  // A layout effect: the listeners are bound in the same commit that shows
  // the drawing, so the first wheel / drag on a just-shown drawing is never
  // dropped (a passive effect could still be pending; that flaked the wheel
  // test under load).
  useLayoutEffect(() => {
    const el = boxRef.current
    const svg = svgRef.current
    if (!el || !svg || !fit) return
    // Dashes are in drawing units: a drawing with linetype patterns is told its zoom (lines.ts).
    const dashed = svg.querySelector('[data-dashed]') !== null
    const apply = (v: Box): void => {
      viewRef.current = v
      svg.setAttribute('viewBox', `${v.x} ${v.y} ${v.w} ${v.h}`)
      if (dashed) setPxPerUnit(svg, el, v)
    }
    const ro = dashed && typeof ResizeObserver !== 'undefined' ? new ResizeObserver(() => viewRef.current && setPxPerUnit(svg, el, viewRef.current)) : null
    ro?.observe(el)
    const frame = (b: Bounds): void => apply(fitBox(framed(b, drawing!.bounds)))
    // The same file read again keeps where you were looking.
    const shown = `${filePath ?? ''}|${viewKey ?? ''}`
    const start = viewBounds(view, drawing!)
    if (viewFor.current === shown && viewRef.current) apply(viewRef.current)
    else if (start) frame(start)
    else apply(fit)
    viewFor.current = shown
    refitRef.current = () => apply(fit)
    frameRef.current = frame

    /** SVG units per screen pixel (viewBox is "meet"-fitted). */
    const unitsPerPx = (v: Box): { k: number; r: DOMRect } => {
      const r = el.getBoundingClientRect()
      if (!r.width || !r.height) return { k: 1, r }
      return { k: Math.max(v.w / r.width, v.h / r.height), r }
    }
    /** Screen point → SVG point under it. */
    const toSvg = (v: Box, cx: number, cy: number): { x: number; y: number } => {
      const { k, r } = unitsPerPx(v)
      const ox = v.x + v.w / 2 - (r.width * k) / 2
      const oy = v.y + v.h / 2 - (r.height * k) / 2
      return { x: ox + (cx - r.left) * k, y: oy + (cy - r.top) * k }
    }
    const zoomAt = (v: Box, factor: number, cx: number, cy: number): Box => {
      const scaleNow = fit.w / v.w
      const next = Math.min(MAX_SCALE, Math.max(MIN_SCALE, scaleNow * factor))
      const f = scaleNow / next
      const p = toSvg(v, cx, cy)
      return { x: p.x - (p.x - v.x) * f, y: p.y - (p.y - v.y) * f, w: v.w * f, h: v.h * f }
    }

    // Selecting and moving (the model path), in drawing coordinates.
    const ctx: SheetCtx = {
      toDrawing: (cx, cy) => {
        const p = toSvg(viewRef.current ?? fit, cx, cy)
        return [p.x, -p.y]
      },
      unitsPerPx: () => unitsPerPx(viewRef.current ?? fit).k,
    }
    // Space held: the left button pans too.
    let space = false
    const typing = (t: EventTarget | null) => t instanceof HTMLElement && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))
    const onKey = (e: KeyboardEvent): void => {
      if (e.code !== 'Space' || typing(e.target)) return
      space = e.type === 'keydown'
      if (e.target === el) e.preventDefault()
      el.style.cursor = space ? 'grab' : ''
    }
    const noMenu = (e: Event): void => {
      if (toolRef.current) e.preventDefault()
    }

    // Drag to pan (pointer capture keeps it going outside the pane).
    let drag: { id: number; x: number; y: number; moved: boolean; tool?: boolean } | null = null
    const onDown = (e: PointerEvent): void => {
      if (drag) return
      const tool = toolRef.current
      if (tool && e.button === 0 && !space) {
        el.focus({ preventScroll: true })
        if (tool.down(e, ctx)) drag = { id: e.pointerId, x: e.clientX, y: e.clientY, moved: false, tool: true }
      }
      drag ??= { id: e.pointerId, x: e.clientX, y: e.clientY, moved: false }
      try {
        el.setPointerCapture(e.pointerId)
      } catch {
        // Synthetic events (tests) have no active pointer to capture.
      }
    }
    const onMove = (e: PointerEvent): void => {
      const v = viewRef.current
      if (!drag || e.pointerId !== drag.id || !v) return
      if (drag.tool) return toolRef.current?.move(e, ctx)
      const dxPx = e.clientX - drag.x
      const dyPx = e.clientY - drag.y
      if (!drag.moved && Math.abs(dxPx) + Math.abs(dyPx) < DRAG_SLOP_PX) return
      drag.moved = true
      el.style.cursor = 'grabbing'
      const { k } = unitsPerPx(v)
      apply({ ...v, x: v.x - dxPx * k, y: v.y - dyPx * k })
      drag.x = e.clientX
      drag.y = e.clientY
    }
    const onUp = (e: PointerEvent): void => {
      if (!drag || e.pointerId !== drag.id) return
      if (drag.tool && e.type === 'pointerup') toolRef.current?.up(e, ctx)
      else if (drag.tool) toolRef.current?.cancel()
      drag = null
      el.style.cursor = ''
    }

    // Wheel (mouse) and trackpad pinch (Chromium/WebKitGTK send ctrl+wheel).
    let gesturing = false
    const onWheel = (e: WheelEvent): void => {
      e.preventDefault()
      if (gesturing) return
      const v = viewRef.current
      if (!v) return
      const px = e.deltaMode === 1 ? e.deltaY * 16 : e.deltaY
      const rate = e.ctrlKey ? 0.01 : 0.002
      apply(zoomAt(v, Math.exp(-px * rate), e.clientX, e.clientY))
    }
    // WKWebView trackpad pinch arrives as Safari gesture events.
    type GestureLike = Event & { scale?: number; clientX?: number; clientY?: number }
    let lastScale = 1
    const onGestureStart = (e: Event): void => {
      e.preventDefault()
      gesturing = true
      lastScale = 1
    }
    const onGestureChange = (e: Event): void => {
      e.preventDefault()
      const g = e as GestureLike
      const v = viewRef.current
      if (!v || typeof g.scale !== 'number' || g.scale <= 0) return
      const r = el.getBoundingClientRect()
      const cx = typeof g.clientX === 'number' ? g.clientX : r.left + r.width / 2
      const cy = typeof g.clientY === 'number' ? g.clientY : r.top + r.height / 2
      apply(zoomAt(v, g.scale / lastScale, cx, cy))
      lastScale = g.scale
    }
    const onGestureEnd = (e: Event): void => {
      e.preventDefault()
      gesturing = false
    }
    const onDbl = (): void => apply(fit)

    el.addEventListener('pointerdown', onDown)
    el.addEventListener('pointermove', onMove)
    el.addEventListener('pointerup', onUp)
    el.addEventListener('pointercancel', onUp)
    el.addEventListener('wheel', onWheel, { passive: false })
    el.addEventListener('gesturestart', onGestureStart)
    el.addEventListener('gesturechange', onGestureChange)
    el.addEventListener('gestureend', onGestureEnd)
    el.addEventListener('dblclick', onDbl)
    el.addEventListener('contextmenu', noMenu)
    window.addEventListener('keydown', onKey)
    window.addEventListener('keyup', onKey)
    return () => {
      el.removeEventListener('pointerdown', onDown)
      el.removeEventListener('pointermove', onMove)
      el.removeEventListener('pointerup', onUp)
      el.removeEventListener('pointercancel', onUp)
      el.removeEventListener('wheel', onWheel)
      el.removeEventListener('gesturestart', onGestureStart)
      el.removeEventListener('gesturechange', onGestureChange)
      el.removeEventListener('gestureend', onGestureEnd)
      el.removeEventListener('dblclick', onDbl)
      el.removeEventListener('contextmenu', noMenu)
      window.removeEventListener('keydown', onKey)
      window.removeEventListener('keyup', onKey)
      ro?.disconnect()
      refitRef.current = () => {}
      frameRef.current = () => {}
    }
    // `fit` is derived from `drawing`: re-bind only when the drawing or the view address changes.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [drawing, viewKey])

  if (error) {
    const binary = /binary dxf/i.test(error)
    return (
      <ViewerNote
        tone="error"
        title={binary ? 'Binary DXF is not supported' : 'Could not draw the DXF'}
        detail={binary ? 'Save it as ASCII DXF to preview it here.' : error}
        testId="dxf-error"
      />
    )
  }
  if (!drawing) return <ViewerNote title="Drawing…" />
  if (!fit || !drawing.bounds) {
    return (
      <ViewerNote
        title="Nothing to draw"
        detail={
          drawing.skippedTotal
            ? `${drawing.skippedTotal} entities not shown (${skippedLabel(drawing.skipped)}).`
            : null
        }
        testId="dxf-empty"
      />
    )
  }

  const b = drawing.bounds
  return (
    <div className="flex h-full w-full flex-col" data-testid="file-dxf" onKeyDown={editing.onKey}>
      <div className="flex shrink-0 items-center gap-2 px-3 py-1 text-[11px] text-[var(--color-text-muted)]">
        <span className="min-w-0 flex-1 truncate" data-testid="dxf-stats">
          {drawing.drawn.toLocaleString()} entities · {fmtNum(b.maxX - b.minX)} × {fmtNum(b.maxY - b.minY)} units
          {drawing.layers > 1 ? ` · ${drawing.layers} layers` : ''}
          {drawing.paper?.length ? <LayoutsNote paper={drawing.paper} /> : null}
        </span>
        {drawing.notDrawn?.count ? <NotDrawnNote nd={drawing.notDrawn} /> : null}
        <button
          type="button"
          className="px-1.5 py-0.5 text-[10px] font-medium border border-[var(--color-border)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
          onClick={() => refitRef.current()}
          title="Fit the drawing (double-click)"
          data-testid="dxf-fit"
        >
          Fit
        </button>
      </div>
      {editing.viewOnly}
      {live.note}
      <div className="flex min-h-0 flex-1">
        <div
          ref={boxRef}
          className={`relative min-h-0 min-w-0 flex-1 select-none overflow-hidden outline-none ${editing.tool ? 'cursor-default' : 'cursor-grab'}`}
          style={{ touchAction: 'none', ...SHEET_STYLE }}
          tabIndex={editing.tool ? -1 : undefined}
          data-testid="dxf-sheet"
        >
          <svg
            ref={svgRef}
            className="block h-full w-full"
            viewBox={`${fit.x} ${fit.y} ${fit.w} ${fit.h}`}
            preserveAspectRatio="xMidYMid meet"
            data-testid="dxf-svg"
          >
            {drawing.shapes ?? <DxfShapes nodes={drawing.nodes} />}
            {live.outlines}
            {editing.marks}
          </svg>
          {editing.over}
          {drawing.skippedTotal ? (
            <div
              className="pointer-events-none absolute bottom-2.5 left-3 right-3 text-[11px] text-[var(--dxf-note)]"
              data-testid="dxf-skipped"
            >
              {`${drawing.skippedTotal} ${drawing.skippedTotal === 1 ? 'entity' : 'entities'} not shown (${skippedLabel(drawing.skipped)})`}
            </div>
          ) : null}
        </div>
        {editing.panel}
      </div>
    </div>
  )
}

const BINARY_DXF_SENTINEL = 'AutoCAD Binary DXF'

/**
 * DXF bytes → text. UTF-8 first (R2007+ files), else Windows-1252 (the
 * usual `$DWGCODEPAGE` of older ANSI DXFs). Binary DXF keeps its sentinel
 * so `buildDrawing` refuses it with the usual message.
 */
export function decodeDxfBytes(bytes: Uint8Array): string {
  const head = new TextDecoder('latin1').decode(bytes.subarray(0, BINARY_DXF_SENTINEL.length))
  if (head === BINARY_DXF_SENTINEL) return head
  try {
    return new TextDecoder('utf-8', { fatal: true }).decode(bytes)
  } catch {
    return new TextDecoder('windows-1252').decode(bytes)
  }
}

/** DXF preview from raw bytes, for files `fs/read-file` would not return as text. */
export function DxfHostBytesViewer({ filePath }: { filePath: string }): React.JSX.Element {
  const scope = useRoom().scope
  const [text, setText] = useState<string | null>(null)
  const [error, setError] = useState<{ title: string; detail: string } | null>(null)
  const [progress, setProgress] = useState<number | null>(null)
  // The file the drawing on screen came from.
  const shownPathRef = useRef<string | null>(null)
  // A new version of this file from any writer: read it again (the text
  // path gets it from FileViewerPane's own read).
  const version = useDocVersion(scope, filePath)

  useEffect(() => {
    let cancelled = false
    const ac = new AbortController()
    const startedKey = scope.connectionKey
    // A new version keeps the old drawing up until the new one is read.
    if (shownPathRef.current !== filePath) {
      setText(null)
      setError(null)
      setProgress(null)
    }
    loadHostBinary(scope, filePath, {
      allowRangeAssembly: true,
      signal: ac.signal,
      onProgress: (ratio) => {
        if (!cancelled) setProgress(ratio)
      },
    })
      .then((bytes) => {
        if (cancelled || scope.connectionKey !== startedKey) return
        if (bytes.byteLength > DXF_MAX_BYTES) {
          const mb = (n: number): string => `${(n / (1024 * 1024)).toFixed(1)} MB`
          shownPathRef.current = null
          setError({
            title: 'Too large to preview',
            detail: `${mb(bytes.byteLength)} is over the ${mb(DXF_MAX_BYTES)} preview limit.`,
          })
          return
        }
        shownPathRef.current = filePath
        setError(null)
        setText(decodeDxfBytes(bytes))
      })
      .catch((e: unknown) => {
        if (cancelled || isHostSwitchedError(e)) return
        if (e instanceof Error && e.name === 'AbortError') return
        shownPathRef.current = null
        setError({ title: 'Could not open the DXF', detail: e instanceof Error ? e.message : String(e) })
      })
    return () => {
      cancelled = true
      ac.abort()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filePath, scope.connectionKey, version])

  if (error) return <ViewerNote tone="error" title={error.title} detail={error.detail} testId="dxf-error" />
  if (text === null) {
    return (
      <ViewerNote
        title="Loading drawing…"
        detail={progress !== null && progress < 1 ? `${Math.round(progress * 100)}%` : null}
      />
    )
  }
  return <DxfViewer text={text} filePath={filePath} />
}
