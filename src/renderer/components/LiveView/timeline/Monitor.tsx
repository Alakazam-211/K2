// The program monitor (T4): the picture, 16:9 and letterboxed in a black
// well, drawn by a MonitorSource (the sample's footage only on the sample),
// and on it: the clip's label top-left, "Sample data" and the details (ⓘ)
// top-right with the people watching the same spot under them, one quiet
// note bottom-left (or the preparing count), and a card in the picture's
// place when there's no picture to show (each with its next step). What it
// shows comes from monitorView; this only draws it.

import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import { Spinner } from '@/components/ui'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { TimelineModel } from '@/lib/live-view/timeline/fold'
import type { DetailRow } from '@/lib/live-view/timeline/monitor-details'
import type { MonitorSource } from '@/lib/live-view/timeline/monitor-source'
import { cantLine, type MonitorCard, type MonitorView } from '@/lib/live-view/timeline/monitor-view'
import { LiveAvatar } from '../shell/LiveAvatar'
import type { Box } from './box'
import { AudioGlyph, CheckGlyph, FilmCardGlyph, ImageGlyph, InfoGlyph, NestGlyph, NoPreviewGlyph, OfflineGlyph } from './glyphs'
import { MonitorDetails } from './MonitorDetails'

/** The picture's place in a W × H well: 16:9, centred, letterboxed. */
export function frameRect(W: number, H: number): { x: number; y: number; w: number; h: number } {
  const w = Math.max(0, Math.min(W, (H * 16) / 9))
  const h = (w * 9) / 16
  return { x: Math.round((W - w) / 2), y: Math.round((H - h) / 2), w: Math.round(w), h: Math.round(h) }
}

/** Details open as a sheet over the tab when the window is narrow or the monitor short. */
export const SHEET_BELOW_H = 380

function useSize(): [React.RefObject<HTMLDivElement | null>, { w: number; h: number }] {
  const ref = useRef<HTMLDivElement>(null)
  const [s, setS] = useState({ w: 960, h: 540 })
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const read = (): void => setS((o) => (o.w === el.clientWidth && o.h === el.clientHeight ? o : { w: el.clientWidth || o.w, h: el.clientHeight || o.h }))
    read()
    const ro = new ResizeObserver(read)
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  return [ref, s]
}

function CardGlyph({ c }: { c: MonitorCard }): React.JSX.Element {
  const size = 26
  if (c.kind === 'no_preview') return <NoPreviewGlyph size={size} />
  if (c.kind === 'offline') return <OfflineGlyph size={size} />
  if (c.kind === 'nest') return <NestGlyph size={size} />
  if (c.kind === 'nomedia') return c.kindWord === 'Audio' ? <AudioGlyph size={size} /> : <ImageGlyph size={size} />
  return <FilmCardGlyph size={size} />
}

function SkipButton({ f, onSkip }: { f: number | null; onSkip: (f: number) => void }): React.JSX.Element | null {
  if (f == null) return null
  return (
    <div className="lv-mon-acts">
      <button type="button" className="lv-mon-btn" data-k2-focus="" data-k2-chamfer="control" data-testid="lv-mon-skip" onClick={() => onSkip(f)}>
        Skip to the next clip
      </button>
    </div>
  )
}

/** The view and the model travel boxed (box.ts): React's dev prop diff can't serialise their BigInt ticks. */
export interface MonitorProps {
  view: Box<MonitorView>
  model: Box<TimelineModel>
  source: MonitorSource
  people: readonly LivePerson[]
  details: DetailRow[]
  narrow: boolean
  /** Where details open as a sheet (the tab's upper row). */
  sheetHost: HTMLElement | null
  onSkip: (f: number) => void
  onRestart: () => void
  /** Show the clips that can't be previewed (preparing). */
  onShowCant: () => void
}

export function Monitor(p: MonitorProps): React.JSX.Element {
  const v = p.view.v
  const { source } = p
  const [stage, size] = useSize()
  const r = frameRect(size.w, size.h)
  const canvas = useRef<HTMLCanvasElement>(null)
  const [det, setDet] = useState(false)
  const [cantOpen, setCantOpen] = useState(false)
  const info = useRef<HTMLButtonElement>(null)

  // Paint the picture whenever what to show or the frame's size changes.
  useEffect(() => {
    const c = canvas.current
    if (!c || !r.w || !r.h) return
    const dpr = Math.min(2, typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1)
    const W = Math.round(r.w * dpr)
    const H = Math.round(r.h * dpr)
    if (c.width !== W || c.height !== H) {
      c.width = W
      c.height = H
    }
    const g = c.getContext('2d')
    if (!g) return // no canvas here (tests): the overlays still say everything
    g.setTransform(dpr, 0, 0, dpr, 0, 0)
    source.paint(g, r.w, r.h, v.paint)
  }, [source, v.paint.how, v.paint.mi?.hash, v.paint.srcF, v.paint.inF, r.w, r.h]) // eslint-disable-line react-hooks/exhaustive-deps

  const closeDet = (): void => {
    setDet(false)
    info.current?.focus()
  }
  const sheet = p.narrow || size.h < SHEET_BELOW_H
  const detailsNode = det ? <MonitorDetails rows={p.details} sheet={sheet} maxH={sheet ? (p.sheetHost?.clientHeight ?? size.h) - 16 : size.h - 52} onClose={closeDet} /> : null
  const live = v.state !== 'none'
  const person = (id: string): LivePerson | undefined => p.people.find((x) => x.id === id)
  const showCant = cantOpen || !!v.pv.prep?.hl

  return (
    <div
      ref={stage}
      className="lv-mon flex-1 min-h-0"
      data-testid="lv-monitor"
      data-state={v.state}
      data-exact={v.exact ? '' : undefined}
      tabIndex={0}
      data-k2-focus=""
      aria-label="The monitor. Space plays or pauses; the arrow keys step a frame."
    >
      <div className="lv-mon-frame" data-testid="lv-monitor-frame" data-paint={v.paint.how} data-blank={v.paint.how === 'none' ? '' : undefined} style={{ left: r.x, top: r.y, width: r.w, height: r.h }}>
        <canvas ref={canvas} aria-hidden="true" />
        {!live ? <p className="lv-mon-line m-0">{v.head ? 'The edit’s picture plays here.' : ''}</p> : null}
        {live && v.label ? (
          <span className="lv-mon-chip lv-mon-label" data-k2-chamfer="chip" data-testid="lv-mon-label" title={`${v.label.lane} · ${v.label.name}`}>
            <span className="lv-mon-lane">{v.label.lane}</span>
            <span className="lv-mon-clip">{v.label.name}</span>
          </span>
        ) : null}
        {live && source.sample ? (
          <span className="lv-mon-chip lv-mon-sample" data-k2-chamfer="chip" data-testid="lv-mon-sample" title="Sample data: no real file and no real footage. Each source is drawn in the app.">
            Sample data
          </span>
        ) : null}
        {live ? (
          <button
            ref={info}
            type="button"
            className="lv-mon-chip lv-mon-info"
            data-k2-chamfer="chip"
            data-k2-focus=""
            data-testid="lv-mon-info"
            aria-expanded={det}
            aria-label="Details"
            title="Details: the file, the player and the numbers behind this view"
            onClick={() => (det ? closeDet() : setDet(true))}
          >
            <InfoGlyph size={14} />
          </button>
        ) : null}
        {live && v.spots.length ? (
          <div className="lv-mon-spots" data-testid="lv-mon-spots">
            {v.spots.map((s) => {
              const who = person(s.id)
              return (
                <span key={s.id} className="lv-mon-chip lv-mon-spot" data-k2-chamfer="chip" data-testid={`lv-mon-spot-${s.id}`} style={{ '--lv-c': s.colour } as CSSProperties} title={`${s.name} is watching here`}>
                  <LiveAvatar person={who ?? { name: s.name, kind: 'person', colour: s.colour, away: false, self: false }} size={16} />
                  {s.name}
                </span>
              )
            })}
          </div>
        ) : null}
        {v.card ? (
          <div className="lv-mon-card" role="status" data-k2-chamfer="surface" data-testid="lv-mon-card" data-kind={v.card.kind} data-tone={v.card.kind === 'offline' ? 'bad' : undefined}>
            <CardGlyph c={v.card} />
            <b>{v.card.title}</b>
            {v.card.line ? <span className="lv-mon-sub">{v.card.line}</span> : null}
            {v.card.kindWord ? <span className="lv-mon-kindw">{v.card.kindWord}</span> : null}
            <SkipButton f={v.card.next} onSkip={p.onSkip} />
          </div>
        ) : null}
        {v.gap ? (
          <div className="lv-mon-gap" role="status" data-testid="lv-mon-gap">
            {v.gap.line}
            <SkipButton f={v.gap.next} onSkip={p.onSkip} />
          </div>
        ) : null}
        {v.spinner ? (
          <span className="lv-mon-first" aria-hidden="true">
            <Spinner />
          </span>
        ) : null}
        {v.note ? (
          <div className="lv-mon-chip lv-mon-note" data-k2-chamfer="chip" role="status" data-testid="lv-mon-note" data-late={v.note.late ? '' : undefined} title={v.note.text}>
            {v.note.spin ? <Spinner className="!h-3 !w-3" /> : null}
            <span className="lv-mon-tx">{v.note.text}</span>
            {v.note.restart ? (
              <button type="button" className="lv-mon-btn" data-k2-focus="" data-k2-chamfer="control" data-testid="lv-mon-restart" onClick={p.onRestart}>
                Play from the start
              </button>
            ) : null}
          </div>
        ) : null}
        {v.prep ? (
          <div className="lv-mon-chip lv-mon-prep" role="status" data-k2-chamfer="chip" data-testid="lv-mon-prep" data-done={v.prep.done ? '' : undefined}>
            {v.prep.hold ? (
              <span className="lv-mon-l">
                <Spinner className="!h-3 !w-3" />
                {v.prep.hold}
              </span>
            ) : null}
            <span className="lv-mon-l">
              {v.prep.done ? <CheckGlyph size={13} /> : v.prep.hold ? null : <Spinner className="!h-3 !w-3" />}
              {v.prep.text}
            </span>
            {v.prep.cant.length && !v.prep.done ? (
              <span className="lv-mon-l">
                <button
                  type="button"
                  className="lv-mon-btn"
                  data-k2-focus=""
                  data-k2-chamfer="control"
                  aria-pressed={showCant}
                  data-testid="lv-mon-cant"
                  title="Show them on the lanes"
                  onClick={() => {
                    setCantOpen((o) => !o)
                    if (!showCant) p.onShowCant()
                  }}
                >
                  {cantLine(v.prep.cant.length)}
                </button>
              </span>
            ) : null}
            {showCant && v.prep.cant.length && !v.prep.done ? (
              <>
                <span className="lv-mon-l lv-mon-why">The computer that holds them can’t read their files, so they show as cards. Go to:</span>
                <span className="lv-mon-l">
                  {v.prep.cant.map((c) => (
                    <button key={c.key} type="button" className="lv-mon-btn" data-k2-focus="" data-k2-chamfer="control" onClick={() => p.onSkip(c.f)}>
                      {c.name}
                    </button>
                  ))}
                </span>
              </>
            ) : null}
          </div>
        ) : null}
      </div>
      {detailsNode && !sheet ? detailsNode : null}
      {detailsNode && sheet && p.sheetHost ? createPortal(detailsNode, p.sheetHost) : null}
    </div>
  )
}
