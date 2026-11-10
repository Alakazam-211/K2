// The Org Chart's canvas (prd-org-chart-v1 §6.5): buckets (orgs), agent
// cards, solid lines and the dotted layer, drawn from the chart and its
// layout. It owns only view state (camera, a drag in progress); every
// gesture comes out as one `OrgGesture` for the page to send.
//
//   - drag a card into a bucket: join that org (from another org: move);
//     out onto the canvas: leave it; inside its own bucket, or a loose card:
//     a person's placement;
//   - drag a bucket by its title: a person's placement;
//   - drag from a card's (or bucket's) dot to another's: bottom → top is
//     "below", side → side is "beside"; top → top and loops snap back with
//     the daemon's sentence and post nothing;
//   - a line: select it, then Remove (or Delete);
//   - a member's ⋯ menu: Make head, Remove from the org.
// Rights come from the chart: `canManage` (orgs, members, heads, lines:
// dots, membership drops, menus) and `canArrange` (where things sit: drags
// that only move). A Member login has neither and gets the view only.
//
// Lines are Board's: routes, ends and dash patterns from board-file-1's
// geom.js (`board-geom.ts`); pan, zoom and minimap are copies of Board's.

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Surface, OverflowMenu, type OverflowMenuItem } from '@/components/ui'
import { BoardGeom, boardDash, type Box, type Side } from './board-geom'
import { toWorld, wheel, type Camera } from './org-camera'
import { bucketAt, BUCKET_TITLE_H, type OrgLayout } from './org-layout'
import type { OrgAgent, OrgChart, OrgLineKind, OrgRef } from './org-api'
import { checkLine, wireKind } from './org-rules'
import { orgOfAgent } from './org-ops'

export type OrgGesture =
  | { type: 'join'; org: string; agent: string; from: string | null }
  /** `at`: where the card was let go, when it was dragged out. */
  | { type: 'leave'; org: string; agent: string; at?: { x: number; y: number } }
  | { type: 'place'; node: OrgRef; x: number; y: number }
  | { type: 'line'; kind: OrgLineKind; a: OrgRef; b: OrgRef }
  | { type: 'unline'; id: string }
  | { type: 'head'; org: string; agent: string }

const SIDES: Side[] = ['top', 'right', 'bottom', 'left']
const DRAG_SLOP = 3
const LINE_W = 2
const DOT_W = 2

/** The side of `box` whose anchor is nearest a world point (where a dragged
 *  line lands on the card it was let go over). */
export function nearestSide(box: Box, x: number, y: number): Side {
  let best: Side = 'top'
  let bestD = Infinity
  for (const s of SIDES) {
    const [ax, ay] = BoardGeom.anchorPoint(box, s)
    const d = Math.hypot(ax - x, ay - y)
    if (d < bestD) {
      bestD = d
      best = s
    }
  }
  return best
}

/** Where a dropped card goes, from where it was let go (its box in world units). */
export function resolveCardDrop(chart: OrgChart, layout: OrgLayout, agent: string, box: Box): OrgGesture {
  const cur = orgOfAgent(chart, agent)
  const target = bucketAt(layout, box.x + box.w / 2, box.y + box.h / 2)
  if (target && target !== cur) return { type: 'join', org: target, agent, from: cur }
  if (target && target === cur) {
    const b = layout.orgs.get(target)!
    return { type: 'place', node: { kind: 'agent', id: agent }, x: Math.round(box.x - b.x), y: Math.round(box.y - b.y) }
  }
  if (cur) return { type: 'leave', org: cur, agent, at: { x: Math.round(box.x), y: Math.round(box.y) } }
  return { type: 'place', node: { kind: 'agent', id: agent }, x: Math.round(box.x), y: Math.round(box.y) }
}

/** What a line dragged from (from, fromSide) to (to, toSide) makes, or why not. */
export function resolveWire(
  chart: OrgChart,
  from: OrgRef,
  fromSide: Side,
  to: OrgRef,
  toSide: Side,
): { ok: true; gesture: OrgGesture } | { ok: false; message: string } {
  const w = wireKind(fromSide, toSide)
  if (!w.ok) return { ok: false, message: w.refusal.message }
  const [a, b] = w.kind === 'below' && w.upper === 'to' ? [to, from] : [from, to]
  const no = checkLine(chart, w.kind, a, b)
  if (no) return { ok: false, message: no.message }
  return { ok: true, gesture: { type: 'line', kind: w.kind, a, b } }
}

type Drag =
  | { kind: 'pan'; sx: number; sy: number; cam: Camera }
  | { kind: 'card'; agent: string; sx: number; sy: number; box: Box; moved: boolean }
  | { kind: 'bucket'; org: string; sx: number; sy: number; box: Box; moved: boolean }
  | { kind: 'wire'; from: OrgRef; side: Side; start: [number, number]; at: [number, number] }

interface Props {
  chart: OrgChart
  layout: OrgLayout
  canManage: boolean
  canArrange: boolean
  dotted: boolean
  cam: Camera
  onCamera: (cam: Camera) => void
  onGesture: (g: OrgGesture) => void
  onRefuse: (message: string) => void
  onOpenOrg: (org: string) => void
  selectedLine: string | null
  onSelectLine: (id: string | null) => void
  /** The viewport's size changed (the minimap and Fit read it). */
  onViewport?: (size: { w: number; h: number }) => void
}

function initials(name: string): string {
  return (
    name
      .split(/[\s\-_]+/)
      .filter(Boolean)
      .slice(0, 2)
      .map((s) => s[0]!.toUpperCase())
      .join('') || '?'
  )
}

export function OrgChartCanvas(props: Props): React.JSX.Element {
  const { chart, layout, canManage, canArrange, dotted, cam, onCamera, onGesture, onRefuse, onOpenOrg, selectedLine, onSelectLine, onViewport } = props
  const viewportRef = useRef<HTMLDivElement>(null)
  const [drag, setDrag] = useState<Drag | null>(null)
  const dragRef = useRef<Drag | null>(null)
  const camRef = useRef(cam)
  camRef.current = cam
  const agentsById = useMemo(() => new Map(chart.agents.map((a) => [a.id, a])), [chart.agents])
  /** A card can be picked up when the drop can do something: change its org, or move it. */
  const canDrag = canManage || canArrange

  const setDragBoth = (d: Drag | null): void => {
    dragRef.current = d
    setDrag(d)
  }

  /** A pointer event's spot in world units. */
  const world = useCallback((clientX: number, clientY: number): [number, number] => {
    const r = viewportRef.current?.getBoundingClientRect()
    return toWorld(camRef.current, clientX - (r?.left ?? 0), clientY - (r?.top ?? 0))
  }, [])

  // Board's wheel: scroll pans, pinch / ⌃ / ⌘ zooms. A native listener, so
  // preventDefault holds (React's wheel listener is passive).
  useEffect(() => {
    const el = viewportRef.current
    if (!el) return
    const onWheel = (e: WheelEvent): void => {
      e.preventDefault()
      const r = el.getBoundingClientRect()
      onCamera(wheel(camRef.current, e, e.clientX - r.left, e.clientY - r.top))
    }
    el.addEventListener('wheel', onWheel, { passive: false })
    return () => el.removeEventListener('wheel', onWheel)
  }, [onCamera])

  useEffect(() => {
    const el = viewportRef.current
    if (!el || !onViewport) return
    const report = (): void => {
      const r = el.getBoundingClientRect()
      onViewport({ w: r.width, h: r.height })
    }
    report()
    if (typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(report)
    ro.observe(el)
    return () => ro.disconnect()
  }, [onViewport])

  /** The card or bucket under a world point (cards first: they sit on top). */
  const nodeAt = useCallback(
    (x: number, y: number): { ref: OrgRef; box: Box } | null => {
      for (const a of layout.agents.values()) {
        if (x >= a.x && x <= a.x + a.w && y >= a.y && y <= a.y + a.h) return { ref: { kind: 'agent', id: a.id }, box: a }
      }
      const org = bucketAt(layout, x, y)
      return org ? { ref: { kind: 'org', id: org }, box: layout.orgs.get(org)! } : null
    },
    [layout],
  )

  // One window-level move/up pair while a drag is on (as Board does).
  useEffect(() => {
    if (!drag) return
    const move = (e: PointerEvent): void => {
      const d = dragRef.current
      if (!d) return
      if (d.kind === 'pan') {
        onCamera({ ...d.cam, x: d.cam.x + e.clientX - d.sx, y: d.cam.y + e.clientY - d.sy })
        return
      }
      if (d.kind === 'wire') {
        setDragBoth({ ...d, at: world(e.clientX, e.clientY) })
        return
      }
      const dx = (e.clientX - d.sx) / camRef.current.s
      const dy = (e.clientY - d.sy) / camRef.current.s
      const moved = d.moved || Math.hypot(e.clientX - d.sx, e.clientY - d.sy) > DRAG_SLOP
      setDragBoth({ ...d, moved, box: { ...d.box, x: d.box.x + dx, y: d.box.y + dy }, sx: e.clientX, sy: e.clientY })
    }
    const up = (e: PointerEvent): void => {
      const d = dragRef.current
      setDragBoth(null)
      if (!d) return
      if (d.kind === 'card') {
        if (!d.moved) return
        const g = resolveCardDrop(chart, layout, d.agent, d.box)
        // Moving a card is arranging; changing its org is managing.
        if (g.type === 'place' ? canArrange : canManage) onGesture(g)
        return
      }
      if (d.kind === 'bucket') {
        if (d.moved) onGesture({ type: 'place', node: { kind: 'org', id: d.org }, x: Math.round(d.box.x), y: Math.round(d.box.y) })
        else onOpenOrg(d.org)
        return
      }
      if (d.kind === 'wire') {
        const [x, y] = world(e.clientX, e.clientY)
        const hit = nodeAt(x, y)
        if (!hit) return
        const r = resolveWire(chart, d.from, d.side, hit.ref, nearestSide(hit.box, x, y))
        if (r.ok) onGesture(r.gesture)
        else onRefuse(r.message)
      }
    }
    const cancel = (): void => setDragBoth(null)
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
    window.addEventListener('pointercancel', cancel)
    return () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
      window.removeEventListener('pointercancel', cancel)
    }
  }, [drag !== null, chart, layout, canManage, canArrange, onCamera, onGesture, onRefuse, onOpenOrg, nodeAt, world]) // eslint-disable-line react-hooks/exhaustive-deps

  // Delete / Backspace removes the selected line.
  useEffect(() => {
    if (!selectedLine || !canManage) return
    const key = (e: KeyboardEvent): void => {
      const t = e.target as HTMLElement | null
      if (t && (t.tagName === 'INPUT' || t.tagName === 'TEXTAREA' || t.isContentEditable)) return
      if (e.key === 'Delete' || e.key === 'Backspace') {
        e.preventDefault()
        onGesture({ type: 'unline', id: selectedLine })
        onSelectLine(null)
      }
    }
    window.addEventListener('keydown', key)
    return () => window.removeEventListener('keydown', key)
  }, [selectedLine, canManage, onGesture, onSelectLine])

  // While a card or bucket is dragged, it (and a bucket's members) draw where the pointer has it.
  const boxOf = (ref: OrgRef): Box | null => {
    const d = drag
    if (ref.kind === 'agent') {
      if (d?.kind === 'card' && d.agent === ref.id) return d.box
      const a = layout.agents.get(ref.id)
      if (!a) return null
      if (d?.kind === 'bucket' && a.orgId === d.org) {
        const b = layout.orgs.get(d.org)!
        return { ...a, x: a.x + d.box.x - b.x, y: a.y + d.box.y - b.y }
      }
      return a
    }
    if (d?.kind === 'bucket' && d.org === ref.id) return d.box
    return layout.orgs.get(ref.id) ?? null
  }

  const startWire = (e: React.PointerEvent, from: OrgRef, side: Side): void => {
    if (!canManage || e.button !== 0) return
    e.stopPropagation()
    e.preventDefault()
    const box = boxOf(from)
    if (!box) return
    const start = BoardGeom.anchorPoint(box, side)
    setDragBoth({ kind: 'wire', from, side, start, at: start })
  }

  const dots = (ref: OrgRef, label: string): React.JSX.Element | null =>
    canManage ? (
      <>
        {SIDES.map((s) => (
          <span
            key={s}
            role="button"
            tabIndex={-1}
            aria-label={`Draw a line from ${label}'s ${s}`}
            data-org-dot={s}
            onPointerDown={(e) => startWire(e, ref, s)}
            className={
              'absolute z-20 h-3 w-3 -translate-x-1/2 -translate-y-1/2 cursor-crosshair rounded-[var(--radius-pill)] border border-[var(--color-accent)] bg-[var(--color-bg)] opacity-0 transition-opacity group-hover:opacity-100 ' +
              (drag?.kind === 'wire' ? 'opacity-100' : '')
            }
            style={{
              left: s === 'left' ? 0 : s === 'right' ? '100%' : '50%',
              top: s === 'top' ? 0 : s === 'bottom' ? '100%' : '50%',
            }}
          />
        ))}
      </>
    ) : null

  // ── lines ─────────────────────────────────────────────────────────────
  const solid = chart.lines.flatMap((l) => {
    const A = boxOf(l.a)
    const B = boxOf(l.b)
    if (!A || !B) return []
    let from: Side
    let to: Side
    let pa = A
    let pb = B
    if (l.kind === 'below') {
      from = 'bottom'
      to = 'top'
    } else {
      // Beside: from the left card's right edge to the right card's left edge.
      if (A.x + A.w / 2 > B.x + B.w / 2) {
        pa = B
        pb = A
      }
      from = 'right'
      to = 'left'
    }
    const r = BoardGeom.route({ from: { id: 'a', anchor: from }, to: { id: 'b', anchor: to }, route: 'elbow' }, (id) =>
      id === 'a' ? pa : id === 'b' ? pb : null,
    )
    return [{ id: l.id, kind: l.kind, d: r.d, mid: BoardGeom.midpoint(r.pts) }]
  })
  const dottedLines = dotted
    ? chart.connections.flatMap((c, i) => {
        const A = boxOf({ kind: 'agent', id: c.a })
        const B = boxOf({ kind: 'agent', id: c.b })
        if (!A || !B) return []
        const r = BoardGeom.route({ from: { id: 'a', anchor: 'auto' }, to: { id: 'b', anchor: 'auto' }, route: 'straight' }, (id) =>
          id === 'a' ? A : id === 'b' ? B : null,
        )
        return [{ key: `${c.a}|${c.b}|${i}`, d: r.d }]
      })
    : []
  const selected = solid.find((l) => l.id === selectedLine) ?? null

  // ── cards ─────────────────────────────────────────────────────────────
  const card = (a: OrgAgent): React.JSX.Element | null => {
    const box = boxOf({ kind: 'agent', id: a.id })
    if (!box) return null
    const orgId = layout.agents.get(a.id)?.orgId ?? null
    const org = orgId ? chart.orgs.find((o) => o.id === orgId) ?? null : null
    const head = !!org && org.headId === a.id
    const remote = a.server !== null
    const dragging = drag?.kind === 'card' && drag.agent === a.id
    const menu: OverflowMenuItem[] = org
      ? [
          ...(head ? [] : [{ id: 'head', label: `Make head of ${org.name}`, onSelect: () => onGesture({ type: 'head', org: org.id, agent: a.id }) }]),
          { id: 'leave', label: `Remove from ${org.name}`, onSelect: () => onGesture({ type: 'leave', org: org.id, agent: a.id }) },
        ]
      : []
    return (
      <Surface
        key={a.id}
        chamfer
        material="tile"
        role2="elevated"
        elevation={dragging ? 3 : 1}
        data-org-agent={a.id}
        data-head={head ? '' : undefined}
        data-remote={remote ? '' : undefined}
        aria-label={`${a.name}${head ? `, head of ${org!.name}` : ''}${remote ? `, on ${a.server}` : ''}`}
        className={
          'group absolute flex items-center gap-2 px-2.5 select-none ' +
          (canDrag && !remote ? 'cursor-grab ' : '') +
          (dragging ? 'z-30 cursor-grabbing ' : 'z-10 ') +
          (head ? 'border-[var(--color-accent)] ' : '')
        }
        style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
        onPointerDown={(e) => {
          if (!canDrag || remote || e.button !== 0) return
          e.stopPropagation()
          setDragBoth({ kind: 'card', agent: a.id, sx: e.clientX, sy: e.clientY, box: { ...box }, moved: false })
        }}
      >
        <span
          aria-hidden
          className="flex h-9 w-9 flex-shrink-0 items-center justify-center overflow-hidden rounded-[var(--radius-badge)] type-name text-[var(--color-on-accent)]"
          style={{ background: a.color ?? 'var(--color-neutral)' }}
        >
          {a.iconUrl ? <img src={a.iconUrl} alt="" draggable={false} className="h-full w-full object-cover" /> : initials(a.name)}
        </span>
        <span className="min-w-0 flex-1">
          <span className="flex items-center gap-1.5">
            <span className="truncate type-name text-[var(--color-text-primary)]">{a.name}</span>
            {head ? (
              <span data-org-head-badge="" className="type-badge flex-shrink-0 px-1 text-[var(--color-accent)]" title={`Head of ${org!.name}`}>
                Head
              </span>
            ) : null}
          </span>
          {a.role ? <span className="block truncate type-hint text-[var(--color-text-muted)]">{a.role}</span> : null}
          {remote ? <span className="block truncate type-hint text-[var(--color-text-muted)]">on {a.server}</span> : null}
        </span>
        {canManage && menu.length ? (
          <span onPointerDown={(e) => e.stopPropagation()} className="flex-shrink-0">
            <OverflowMenu items={menu} label={`${a.name}: more`} triggerProps={{ 'data-org-agent-menu': a.id }} />
          </span>
        ) : null}
        {remote ? null : dots({ kind: 'agent', id: a.id }, a.name)}
      </Surface>
    )
  }

  const wire = drag?.kind === 'wire' ? drag : null

  return (
    <div
      ref={viewportRef}
      data-testid="org-chart-canvas"
      className={'relative h-full w-full overflow-hidden bg-[var(--color-bg-canvas)] ' + (drag?.kind === 'pan' ? 'cursor-grabbing' : 'cursor-default')}
      style={{
        backgroundImage: 'radial-gradient(var(--color-border) 1px, transparent 1px)',
        backgroundSize: `${24 * cam.s}px ${24 * cam.s}px`,
        backgroundPosition: `${cam.x}px ${cam.y}px`,
      }}
      onPointerDown={(e) => {
        if (e.button !== 0 && e.button !== 1) return
        onSelectLine(null)
        setDragBoth({ kind: 'pan', sx: e.clientX, sy: e.clientY, cam })
      }}
    >
      <div
        data-testid="org-chart-world"
        className="absolute left-0 top-0 origin-top-left"
        style={{ transform: `translate(${cam.x}px, ${cam.y}px) scale(${cam.s})` }}
      >
        {layout.labels.map((l) => (
          <div key={l.text} className="absolute type-section uppercase tracking-wider text-[var(--color-text-muted)]" style={{ left: l.x, top: l.y }}>
            {l.text}
          </div>
        ))}

        {chart.orgs.map((o) => {
          const box = boxOf({ kind: 'org', id: o.id })
          if (!box) return null
          const dragging = drag?.kind === 'bucket' && drag.org === o.id
          const head = o.headId ? agentsById.get(o.headId)?.name ?? null : null
          return (
            <Surface
              key={o.id}
              chamfer
              role2="surface"
              data-org-bucket={o.id}
              className={'group absolute ' + (dragging ? 'z-[5]' : 'z-0')}
              style={{ left: box.x, top: box.y, width: box.w, height: box.h }}
            >
              <div
                role="button"
                tabIndex={0}
                data-org-bucket-title={o.id}
                aria-label={`${o.name}: open`}
                className={
                  'flex items-center gap-2 border-b border-[var(--color-border)] px-3 ' + (canArrange ? 'cursor-grab' : 'cursor-pointer')
                }
                style={{ height: BUCKET_TITLE_H }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault()
                    onOpenOrg(o.id)
                  }
                }}
                onPointerDown={(e) => {
                  if (e.button !== 0) return
                  e.stopPropagation()
                  if (!canArrange) {
                    onOpenOrg(o.id)
                    return
                  }
                  setDragBoth({ kind: 'bucket', org: o.id, sx: e.clientX, sy: e.clientY, box: { ...box }, moved: false })
                }}
              >
                <span className="truncate type-name text-[var(--color-text-primary)]">{o.name}</span>
                <span className="type-hint text-[var(--color-text-muted)]">{o.memberIds.length}</span>
                <span className="ml-auto truncate type-hint text-[var(--color-text-muted)]">{head ? `Head: ${head}` : 'No head yet'}</span>
              </div>
              {o.memberIds.length === 0 ? (
                <div className="absolute inset-x-0 bottom-0 flex items-center justify-center type-hint text-[var(--color-text-muted)]" style={{ top: BUCKET_TITLE_H }}>
                  {canManage ? 'Drop agents here' : 'No agents yet'}
                </div>
              ) : null}
              {dots({ kind: 'org', id: o.id }, o.name)}
            </Surface>
          )
        })}

        <svg data-testid="org-chart-lines" className="pointer-events-none absolute left-0 top-0 z-[8] overflow-visible" width={1} height={1}>
          {dottedLines.map((l) => (
            <path
              key={l.key}
              data-org-dotted=""
              d={l.d}
              fill="none"
              strokeWidth={DOT_W}
              strokeLinecap="round"
              strokeDasharray={boardDash('dotted', DOT_W)}
              style={{ stroke: 'var(--color-text-muted)' }}
            />
          ))}
          {solid.map((l) => (
            <g key={l.id} data-org-line={l.id} data-kind={l.kind}>
              <path
                d={l.d}
                fill="none"
                strokeWidth={14}
                className="cursor-pointer"
                style={{ stroke: 'transparent', pointerEvents: 'stroke' }}
                onPointerDown={(e) => {
                  e.stopPropagation()
                  onSelectLine(l.id)
                }}
              />
              <path
                d={l.d}
                fill="none"
                strokeWidth={l.id === selectedLine ? LINE_W + 1 : LINE_W}
                strokeLinejoin="round"
                style={{ stroke: l.id === selectedLine ? 'var(--color-accent)' : 'var(--color-text-secondary)' }}
              />
            </g>
          ))}
          {wire ? (
            <path
              data-org-wire=""
              d={`M${wire.start[0]} ${wire.start[1]}L${wire.at[0]} ${wire.at[1]}`}
              fill="none"
              strokeWidth={LINE_W}
              strokeDasharray={boardDash('dashed', LINE_W)}
              style={{ stroke: 'var(--color-accent)' }}
            />
          ) : null}
        </svg>

        {chart.agents.map(card)}

        {selected && canManage ? (
          <button
            type="button"
            data-org-line-remove={selected.id}
            className="absolute z-40 -translate-x-1/2 -translate-y-1/2 border border-[var(--color-border-strong)] bg-[var(--color-bg-elevated)] px-2 py-0.5 type-hint text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
            style={{ left: selected.mid[0], top: selected.mid[1] }}
            onPointerDown={(e) => e.stopPropagation()}
            onClick={() => {
              onGesture({ type: 'unline', id: selected.id })
              onSelectLine(null)
            }}
          >
            Remove line
          </button>
        ) : null}
      </div>
    </div>
  )
}

