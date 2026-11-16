// Over the still: boxes, the ghost and arrows of the live change in an SVG
// laid exactly on the letterboxed still (in the still's own pixels, from
// its own camera), and the labels in the stage's pixels: tags, one label
// per hold, your pick, notes, others' selections, and a chip on the edge
// for what is off the picture (click: frame it). Labels are measured after
// drawing and placed by lib/live-view/scene/labels.ts so none touches
// another or the stage's chrome. A new change slides from its old place
// (380 ms, easing out) before its tag and arrows show; under reduced
// motion it is drawn at its end at once, and the tag doesn't fade. Every
// colour is a presence token (`--lv-c`).

import { useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { cx } from '@/components/ui'
import { placeLabels, type LabelReq, type Placed, type Rect } from '@/lib/live-view/scene/labels'
import { liveChange, overlayDraw, slotVar, type BoundsOf, type ChipSpec, type TagSpec } from '@/lib/live-view/scene/overlay'
import type { Letterbox, Still } from '@/lib/live-view/scene/picture-source'
import type { SimState } from '@/lib/live-view/scene/sim'
import { EdgeArrow } from '../shell/glyphs'
import { LockGlyph } from './scene-glyphs'

/** The slide: the mock's 380 ms, easing out. */
export const SLIDE_MS = 380
/** What labels keep clear of: the picker, the older-picture mark, the compass, chips, the key hint, ⓘ, the quiet line, the card, menus, people at the edges. */
const CHROME = '.lv-vbtn, .lv-oldpic, .lv-cmpslot, .lv-chip, .lv-keys, .lv-infob, .lv-toast, .lv-card, .lv-pop, [data-testid^="lv-edge-"]'
/** Chips sit 18 px in from the stage's edge, like the corner slots. */
const EDGE_IN = 14
const PRIO: Record<TagSpec['kind'], number> = { live: 1, hold: 2, mine: 3, note: 4, sel: 5 }

const tint = (slot: Parameters<typeof slotVar>[0]): CSSProperties => ({ ['--lv-c' as string]: slotVar(slot) }) as CSSProperties

function TagBody({ t }: { t: TagSpec }): React.JSX.Element {
  if (t.kind === 'live')
    return (
      <>
        <i className="lv-dot" aria-hidden="true" />
        <b>
          {t.who}
          {t.actorKind === 'file' ? ':' : ''}
        </b>{' '}
        {t.text}
        {t.sub ? <span className="lv-sub">{t.sub}</span> : null}
      </>
    )
  if (t.kind === 'hold')
    return (
      <>
        <span className="lv-lk">
          <LockGlyph size={11} />
        </span>
        <b>{t.who}</b>
        <span className="lv-q"> · {t.text}</span>
      </>
    )
  if (t.kind === 'sel')
    return (
      <>
        {t.agent ? <i className="lv-dot" aria-hidden="true" /> : null}
        {t.who}
        {t.text ? ` ${t.text}` : ''}
      </>
    )
  return t.kind === 'mine' ? <b>{t.text}</b> : <>{t.text}</>
}

/** A label's size before it is laid out (jsdom has no layout): about 7 px a letter. */
const guess = (el: HTMLElement | undefined, text: string): { w: number; h: number } => ({ w: el?.offsetWidth || 22 + text.length * 7, h: el?.offsetHeight || 22 })

export interface SceneOverlayProps {
  S: SimState
  pick: string | null | undefined
  still: Still
  lb: Letterbox
  bounds: BoundsOf
  /** False under reduced motion: no slide, no fades. */
  motion: boolean
  stage: React.RefObject<HTMLDivElement | null>
  pane: { width: number; height: number }
  onFrame: (a: string) => void
}

export function SceneOverlay({ S, pick, still, lb, bounds, motion, stage, pane, onFrame }: SceneOverlayProps): React.JSX.Element | null {
  const lc = liveChange(S)
  const animKey = lc && lc.phase === 'live' && motion ? `${lc.f.rev}:${S.i}` : null
  const [anim, setAnim] = useState<{ key: string | null; t: number }>({ key: null, t: 1 })
  const t = !animKey ? 1 : anim.key === animKey ? anim.t : 0
  useEffect(() => {
    if (!animKey) return
    let raf = 0
    const t0 = performance.now()
    const tick = (): void => {
      const p = Math.min(1, (performance.now() - t0) / SLIDE_MS)
      setAnim({ key: animKey, t: p >= 1 ? 1 : 1 - (1 - p) ** 3 })
      if (p < 1) raf = requestAnimationFrame(tick)
    }
    raf = requestAnimationFrame(tick)
    return () => cancelAnimationFrame(raf)
  }, [animKey])

  const cam = still.camera
  const draw = useMemo(() => (cam ? overlayDraw(S, { pick, still: { camera: cam, width: still.width, height: still.height }, bounds, t }) : null), [S, pick, cam, still.width, still.height, bounds, t])

  // Still pixels to the stage's.
  const k = lb.width / still.width
  const toStage = (r: Rect): Rect => ({ x0: lb.left + r.x0 * k, y0: lb.top + r.y0 * k, x1: lb.left + r.x1 * k, y1: lb.top + r.y1 * k })
  const els = useRef(new Map<string, HTMLElement>())
  const [pos, setPos] = useState<Record<string, Placed>>({})
  const labels: { id: string; tag?: TagSpec; chip?: ChipSpec; text: string }[] = draw
    ? [...draw.chips.map((c) => ({ id: `chip:${c.a}`, chip: c, text: c.name })), ...draw.tags.map((g) => ({ id: g.id, tag: g, text: `${g.who} ${g.text} ${g.sub ?? ''}` }))]
    : []
  const sig = JSON.stringify(labels.map((l) => [l.id, l.text, l.tag?.anchor, l.chip?.at])) + `|${lb.left},${lb.top},${lb.width},${lb.height}|${pane.width},${pane.height}`
  useLayoutEffect(() => {
    const el = stage.current
    if (!el) return
    const sr = el.getBoundingClientRect()
    const obstacles = [...el.querySelectorAll<HTMLElement>(CHROME)]
      .map((e) => e.getBoundingClientRect())
      .filter((r) => r.width > 0 && r.height > 0)
      .map((r) => ({ x0: r.left - sr.left, y0: r.top - sr.top, x1: r.right - sr.left, y1: r.bottom - sr.top }))
    const stageFrame = { x0: EDGE_IN, y0: EDGE_IN, x1: pane.width - EDGE_IN, y1: pane.height - EDGE_IN }
    const reqs: LabelReq[] = labels.map((l) => {
      const { w, h } = guess(els.current.get(l.id), l.text)
      if (l.chip) {
        const p = toStage({ x0: l.chip.at.x, y0: l.chip.at.y, x1: l.chip.at.x, y1: l.chip.at.y })
        return { id: l.id, kind: 'edge', side: l.chip.side, anchor: p, w, h, prio: 0, frame: stageFrame }
      }
      const g = l.tag!
      return { id: l.id, kind: g.place, anchor: toStage(g.anchor), w, h, prio: PRIO[g.kind] }
    })
    const placed = placeLabels(reqs, { frame: { x0: lb.left, y0: lb.top, x1: lb.left + lb.width, y1: lb.top + lb.height } })
    const next = Object.fromEntries(placed.map((p) => [p.id, p]))
    setPos((cur) => (JSON.stringify(cur) === JSON.stringify(next) ? cur : next))
    // eslint-disable-next-line react-hooks/exhaustive-deps -- `sig` is everything the placement reads
  }, [sig, stage])

  if (!draw) return null
  const at = (id: string): CSSProperties => {
    const p = pos[id]
    return p ? { left: p.x, top: p.y } : { left: 0, top: 0, visibility: 'hidden' }
  }
  const ref = (id: string) => (e: HTMLElement | null) => {
    if (e) els.current.set(id, e)
    else els.current.delete(id)
  }
  return (
    <>
      <svg
        className="lv-ovl"
        data-testid="lv-scene-overlay"
        data-t={t >= 1 ? '1' : t <= 0 ? '0' : t.toFixed(2)}
        data-motion={motion ? '' : undefined}
        viewBox={`0 0 ${still.width} ${still.height}`}
        preserveAspectRatio="none"
        style={{ left: lb.left, top: lb.top, width: lb.width, height: lb.height }}
        aria-hidden="true"
      >
        {draw.ghosts.map((g) => (
          <path key={`g${g.a}`} className="lv-bx" data-style="ghost" data-a={g.a} data-hidden={g.hidden ? '' : undefined} d={g.d} style={tint(g.slot)} />
        ))}
        {draw.boxes.map((b) => (
          <path key={b.a} className="lv-bx" data-a={b.a} data-style={b.style} data-approx={b.approx ?? undefined} data-hidden={b.hidden ? '' : undefined} data-away={b.away ? '' : undefined} data-rides={b.rides ? '' : undefined} d={b.d} style={tint(b.slot)} />
        ))}
        {draw.arrows.map((a, i) => (
          <path key={`a${a.a}${i}`} className="lv-dir" data-kind={a.kind} data-fading={a.fading && motion ? '' : undefined} d={a.d} style={tint(a.slot)} />
        ))}
      </svg>
      <div className="lv-tags" data-testid="lv-scene-tags">
        {draw.chips.map((c) => {
          const id = `chip:${c.a}`
          const say = `${c.name} is off the picture. Frame it`
          return (
            <button
              key={id}
              ref={ref(id)}
              type="button"
              className="lv-offchip lv-plate"
              data-testid="lv-offchip"
              data-k2-focus=""
              data-k2-chamfer="chip"
              data-side={c.side}
              data-touching={pos[id]?.touching ? '' : undefined}
              aria-label={say}
              title={say}
              style={{ ...tint(c.slot), ...at(id) }}
              onClick={() => onFrame(c.a)}
            >
              <EdgeArrow side={c.side} size={12} />
              <i className="lv-dot" aria-hidden="true" />
              {c.name}
            </button>
          )
        })}
        {draw.tags.map((g) => (
          <div
            key={g.id}
            ref={ref(g.id)}
            className={cx('lv-tag', 'lv-plate')}
            data-testid="lv-tag"
            data-kind={g.kind}
            data-a={g.a}
            data-k2-chamfer="chip"
            data-away={g.away ? '' : undefined}
            data-fading={g.fading && motion ? '' : undefined}
            data-touching={pos[g.id]?.touching ? '' : undefined}
            title={g.title}
            style={{ ...tint(g.slot), ...at(g.id) }}
          >
            <TagBody t={g} />
          </div>
        ))}
      </div>
    </>
  )
}
