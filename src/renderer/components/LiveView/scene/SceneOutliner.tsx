// The outliner: "Find an object", then the tree of collections and objects
// (read-only), nested by parent with 16 px per level. Each object row has
// its kind's icon, its name, what can't change live, who holds it, the
// people on it and the agent's dot, and its eye. Only the rows on screen are
// drawn (2,000 objects), whole rows only. Keys (the familiar layers list):
// Up and Down move (an object is picked), Left closes a collection or goes
// to its parent, Right opens one or goes to its first row.

import { useEffect, useId, useRef, useState, type CSSProperties } from 'react'
import { Button, SearchField } from '@/components/ui'
import { collTitle } from '@/lib/live-view/scene/fold'
import { objRow, type OutlinerModel, type OutRow, type RecentChange } from '@/lib/live-view/scene/panel'
import type { SimState } from '@/lib/live-view/scene/sim'
import { LiveAvatar } from '../shell/LiveAvatar'
import { ChevronGlyph } from '../shell/glyphs'
import { CollGlyph, DrivenGlyph, EyeGlyph, EyeOffGlyph, KindGlyph, LinkedGlyph, LockGlyph, QuatGlyph } from './scene-glyphs'

export const ROW_H = 26
const OVERSCAN = 6
const BADGE = { driven: DrivenGlyph, quat: QuatGlyph, linked: LinkedGlyph }

const domId = (base: string, addr: string): string => `${base}-${addr.replace(/[^a-z0-9]/gi, '')}`

export interface OutlinerProps {
  S: SimState
  model: OutlinerModel
  recent: Record<string, RecentChange>
  pick: string | null
  /** The row the keys are on. */
  cur: string | null
  /** How many rows the list shows. */
  rows: number
  /** Scroll this row into view (it just became your pick). */
  reveal: string | null
  live: boolean
  /** The search, as typed. */
  q: string
  onQuery: (q: string) => void
  onToggle: (coll: string, open: boolean) => void
  onPick: (addr: string) => void
  onCur: (addr: string | null) => void
}

function RowView({ r, i, p, base }: { r: OutRow; i: number; p: OutlinerProps; base: string }): React.JSX.Element {
  const top = { top: i * ROW_H }
  if (r.t === 'skel')
    return (
      <div className="lv-orow" style={top} aria-hidden="true">
        <div className="lv-onm lv-skel" style={{ paddingLeft: r.depth * 16 + 18, display: 'flex' }}>
          <i style={{ width: 14 }} />
          <i style={{ width: 70 + ((i * 37) % 90) }} />
        </div>
      </div>
    )
  if (r.t === 'more')
    return (
      <div className="lv-orow" data-more="" style={top} role="none">
        <div className="lv-onm" style={{ paddingLeft: r.depth * 16 + 18 }}>
          <span className="lv-orow-name">{r.n} more arriving…</span>
        </div>
      </div>
    )
  const T = p.model.T!
  const cur = p.cur === r.addr ? '' : undefined
  if (r.t === 'coll') {
    const c = T.colls[r.addr]
    const n = p.S.pages?.[r.addr] ? p.S.pages[r.addr].total : (T.objsIn[r.addr] ?? []).length
    const title = collTitle(p.S.D, r.addr)
    return (
      <div id={domId(base, r.addr)} role="treeitem" aria-level={r.depth + 1} aria-expanded={r.open} aria-label={`${title}, ${n} object${n === 1 ? '' : 's'}`} data-coll="" data-cur={cur} className="lv-orow" style={top} onClick={() => p.onCur(r.addr)}>
        <div className="lv-onm" style={{ paddingLeft: r.depth * 16 }}>
          <button
            type="button"
            tabIndex={-1}
            className="lv-tw no-drag"
            aria-label={`${r.open ? 'Close' : 'Open'} ${title}`}
            aria-expanded={r.open}
            onClick={(e) => {
              e.stopPropagation()
              p.onToggle(r.addr, !r.open)
            }}
          >
            <ChevronGlyph size={11} />
          </button>
          <span className="lv-ti">
            <CollGlyph kind={c.kind} size={14} />
          </span>
          <span className="lv-orow-name">{title}</span>
          <span className="lv-ocount">{n}</span>
        </div>
      </div>
    )
  }
  const info = objRow(p.S, T, r.addr, p.recent)
  const o = info.obj
  const mine = p.pick === r.addr
  const fresh = info.recent && info.recent.age === 0
  const B = info.badge ? BADGE[info.badge.kind] : null
  const style = { ...top, ...(info.recent ? { ['--lv-c' as string]: info.recent.colour } : {}) } as CSSProperties
  return (
    <div
      id={domId(base, r.addr)}
      role="treeitem"
      aria-level={r.depth + 1}
      aria-selected={mine}
      data-obj=""
      data-cur={cur}
      data-hidden={o.hidden ? '' : undefined}
      data-flash={fresh ? '' : undefined}
      className="lv-orow"
      style={style}
      title={fresh ? `${info.recent!.who} ${info.recent!.phrase}` : undefined}
      onClick={() => p.live && p.onPick(r.addr)}
    >
      <div className="lv-onm" style={{ paddingLeft: r.depth * 16 + 18 }}>
        <span className={`lv-ti lv-k-${o.kind}`}>
          <KindGlyph kind={o.kind} size={14} />
        </span>
        <span className="lv-orow-name">{o.name}</span>
        {B && info.badge ? (
          <span data-k2-chamfer="chip" className="lv-badge" title={info.badge.tip}>
            <B size={10} />
          </span>
        ) : null}
      </div>
      <div className="lv-whos">
        {info.hold ? (
          <span data-k2-chamfer="chip" data-hold="" className="lv-badge" style={{ ['--lv-c' as string]: info.hold.colour } as CSSProperties} title={`Held by ${info.hold.name}: ${info.hold.label}`}>
            <LockGlyph size={10} />
          </span>
        ) : null}
        {info.on.map((q) => (
          <span key={q.handle} className="inline-flex" title={`${q.name} is on it${q.away ? ' (away)' : ''}`}>
            <LiveAvatar person={{ name: q.name, kind: q.kind === 'agent' ? 'agent' : 'person', colour: q.colour, away: q.away }} size={14} />
          </span>
        ))}
        {info.agent ? <i className="lv-adot" data-acting={info.acting ? '' : undefined} style={{ ['--lv-c' as string]: info.agent.colour } as CSSProperties} title={`${info.agent.name} is on this`} /> : null}
      </div>
      <i className="lv-eye" title={o.hidden ? 'Hidden in the viewport' : 'Shown'}>
        {o.hidden ? <EyeOffGlyph size={12} /> : <EyeGlyph size={12} />}
      </i>
    </div>
  )
}

export function SceneOutliner(p: OutlinerProps): React.JSX.Element {
  const base = `lv-orow${useId().replace(/[^a-z0-9]/gi, '')}`
  const list = useRef<HTMLDivElement>(null)
  const [top, setTop] = useState(0)
  const rows = p.model.rows
  const h = Math.max(2, p.rows) * ROW_H
  // Only the rows on screen are drawn: follow the scroll (at most once a frame).
  useEffect(() => {
    const el = list.current
    if (!el) return
    let raf = 0
    const on = (): void => {
      if (raf) return
      raf = requestAnimationFrame(() => {
        raf = 0
        setTop(el.scrollTop)
      })
    }
    el.addEventListener('scroll', on, { passive: true })
    return () => {
      el.removeEventListener('scroll', on)
      cancelAnimationFrame(raf)
    }
  }, [])
  // A new step starts at the list's top (it scrolls only to reveal your own pick).
  const revealed = useRef<string | null>(null)
  useEffect(() => {
    revealed.current = null
    if (list.current) list.current.scrollTop = 0
    setTop(0)
  }, [p.S])
  // Reveal your pick once, when it changes: scroll it on screen, starting at its collection when that fits.
  useEffect(() => {
    const el = list.current
    if (!el || !p.reveal || p.reveal === revealed.current) return
    const k = rows.findIndex((r) => r.addr === p.reveal)
    if (k < 0) return
    revealed.current = p.reveal
    if (k * ROW_H < el.scrollTop || (k + 1) * ROW_H > el.scrollTop + h) {
      const vis = Math.floor(h / ROW_H)
      let start = Math.max(0, k - vis + 2)
      for (let j = k - 1; j >= 0 && k - j <= vis - 2; j--) if (rows[j].t === 'coll' && rows[j].depth < (rows[k].depth || 1)) start = j
      el.scrollTop = start * ROW_H
      setTop(el.scrollTop)
    }
  }, [p.reveal, rows, h])

  const nav = rows.filter((r) => r.t === 'obj' || r.t === 'coll')
  const go = (r: OutRow | undefined): void => {
    if (!r) return
    p.onCur(r.addr)
    if (r.t === 'obj') p.onPick(r.addr)
    const el = list.current
    const k = rows.indexOf(r)
    if (!el) return
    if (k * ROW_H < el.scrollTop) el.scrollTop = k * ROW_H
    else if ((k + 1) * ROW_H > el.scrollTop + h) el.scrollTop = (k + 1) * ROW_H - h
    setTop(el.scrollTop)
  }
  const onKey = (e: React.KeyboardEvent): void => {
    if (!['ArrowDown', 'ArrowUp', 'ArrowLeft', 'ArrowRight'].includes(e.key) || !p.live) return
    e.preventDefault()
    e.stopPropagation()
    const i = nav.findIndex((r) => r.addr === (p.cur ?? p.pick))
    const r = nav[i]
    if (e.key === 'ArrowDown' || e.key === 'ArrowUp') return go(i < 0 ? nav[e.key === 'ArrowDown' ? 0 : nav.length - 1] : nav[Math.max(0, Math.min(nav.length - 1, i + (e.key === 'ArrowDown' ? 1 : -1)))])
    if (!r) return
    if (e.key === 'ArrowLeft') {
      if (r.t === 'coll' && r.open) p.onToggle(r.addr, false)
      else for (let j = i - 1; j >= 0; j--) if (nav[j].t === 'coll' && nav[j].depth < r.depth) return go(nav[j])
    } else if (r.t === 'coll' && !r.open) p.onToggle(r.addr, true)
    else if (r.t === 'coll') go(nav[i + 1])
  }
  const onFocus = (): void => {
    if (p.cur && nav.some((r) => r.addr === p.cur)) return
    const r = nav.find((x) => x.addr === p.pick) ?? nav[0]
    if (r) p.onCur(r.addr)
  }

  const a = Math.max(0, Math.floor(top / ROW_H) - OVERSCAN)
  const b = Math.min(rows.length, Math.ceil((top + h) / ROW_H) + OVERSCAN)
  const { none, query } = p.model
  return (
    <>
      <div className="lv-otool">
        <SearchField className="w-full" placeholder="Find an object" aria-label="Find an object by name" value={p.q}onChange={(e) => p.onQuery(e.target.value)} onClear={() => p.onQuery('')} />
      </div>
      {none ? (
        <div className="lv-nohit" role="status">
          <span>
            <b>No object is named “{query}”.</b> Names are matched anywhere in the scene.
          </span>
          <Button size="sm" onClick={() => p.onQuery('')}>
            Clear the search
          </Button>
        </div>
      ) : null}
      <div ref={list} hidden={none} className="lv-olist-box" style={{ height: Math.min(h, Math.max(rows.length, 2) * ROW_H) }}>
        {/* A plain scroller: whole rows stay legible (the panel's own frame fades its ends). */}
        <div>
          <div
            role="tree"
            tabIndex={0}
            aria-label="Collections and objects (read-only)"
            aria-activedescendant={p.cur && nav.some((r) => r.addr === p.cur) ? domId(base, p.cur) : undefined}
            data-rows={rows.length}
            className="lv-olist no-drag"
            style={{ height: rows.length * ROW_H }}
            onKeyDown={onKey}
            onFocus={onFocus}
          >
            {rows.slice(a, b).map((r, k) => (
              <RowView key={r.addr} r={r} i={a + k} p={p} base={base} />
            ))}
          </div>
        </div>
      </div>
    </>
  )
}
