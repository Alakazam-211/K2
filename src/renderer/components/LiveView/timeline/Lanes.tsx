// The lanes and the ruler (T3): the ruler on top (scrolled in step with the
// lanes), then the lanes in a scroller with soft ends (ScrollEdgeFrame):
// headers held at the left, video stacked over audio with a split, clips
// drawn only a screen either side of what shows, everyone's playhead as a
// 1 px line and the monitor's as the brighter one, timeline markers as
// dashed lines. "More lanes below" when the lanes don't fit; the overview
// under a long edit. The lanes take what they need up to `maxFrame`, and
// the rest goes to the monitor.

import { useCallback, useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { ScrollEdgeFrame } from '@/components/ui'
import { personById, type LivePerson } from '@/lib/live-view/shared/people'
import { lanesOf, visibleItems } from '@/lib/live-view/timeline/fold'
import {
  bracketsOf,
  clipMarks,
  defaultPpf,
  fitPpf,
  drawWindow,
  ghostsOf,
  HEAD_W,
  HEAD_W_NARROW,
  isBig,
  laneHeight,
  lanesHeight,
  offscreenOf,
  playheadsOf,
  readinessOf,
  splitBefore,
  timeWidth,
  type Bracket,
  type ViewPrefs,
} from '@/lib/live-view/timeline/lanes-view'
import { DEFAULT_PV, mediaInfo, picModeFor, WAVE_MIN, type MediaFacts } from '@/lib/live-view/timeline/picture'
import { RULER_H } from '@/lib/live-view/timeline/ruler'
import { nowOf, type TimelineView } from '@/lib/live-view/timeline/snapshot'
import type { Edge } from '../shell/EdgeStack'
import { Clip, markerColour } from './Clip'
import { usePlayScroll, type PlayScroll } from './usePlayScroll'
import { DownGlyph, TrackMarkShape, TransitionShape } from './glyphs'
import { measurer, useProbeFonts } from './measure'
import { Overview } from './Overview'
import { Ruler } from './Ruler'
import { box as boxed } from './box'
import { TrackHeader, trackFacts } from './TrackHeader'

export interface LanesProps {
  view: TimelineView
  facts: MediaFacts
  people: readonly LivePerson[]
  narrow: boolean
  prefs: ViewPrefs
  /** Pixels a frame; null: the edit's default for this width; 'fit': the whole edit. */
  ppf: number | 'fit' | null
  /** Bring this frame to the middle (a step that zooms in somewhere). */
  centre?: { f: number; key: string } | null
  /** Bumped to fit the whole edit (scrolls to its start). */
  fitTick: number
  /** The most the lanes' scroller may take (px); the rest is the monitor's. */
  maxFrame: number
  /** The lanes' time width, for zoom and fit. */
  onMeasure: (tw: number) => void
  onKey: (e: React.KeyboardEvent) => void
  onJump: (side: Edge) => void
  /** The monitor's playhead (the monitor's own: yours, or whom you follow); from the state's own playhead when unset. */
  monitor?: { f: number; colour: string; name: string } | null
  /** The lanes follow the monitor while it plays (page or centred) and keep a followed edit point in view (usePlayScroll). */
  follow?: PlayScroll
  /** Your own wheel, drag or keys on the lanes (following stops). */
  onUserScroll?: () => void
  /** The loop that runs, shaded on the ruler; a change's bracket loops it. */
  loop?: { f0: number; f1: number } | null
  onBracket?: (b: Bracket) => void
}

const PROBES = ['lv-tl-name', 'lv-tl-tagfont'] as const

export function Lanes(p: LanesProps): React.JSX.Element {
  const { view, facts, narrow, prefs, onMeasure } = p
  const S = view.state
  const M = view.model
  const HW = narrow ? HEAD_W_NARROW : HEAD_W
  const frame = useRef<HTMLDivElement>(null)
  const [box, setBox] = useState({ w: 1100, h: 400, sb: 0 })
  const [pos, setPos] = useState({ left: 0, top: 0 })
  const { ref: probes, fonts } = useProbeFonts(PROBES)
  const measureName = measurer(fonts[0], 6.6)
  const measureTag = measurer(fonts[1], 7.2)

  const scroller = useCallback((): HTMLElement | null => frame.current?.querySelector<HTMLElement>('[data-scroll-edges-list]') ?? null, [])
  useLayoutEffect(() => {
    const s = scroller()
    if (!s) return
    let raf = 0
    const read = (): void => {
      raf = 0
      setPos((o) => (o.left === s.scrollLeft && o.top === s.scrollTop ? o : { left: s.scrollLeft, top: s.scrollTop }))
    }
    const size = (): void => setBox((o) => (o.w === s.clientWidth && o.h === s.clientHeight && o.sb === s.offsetHeight - s.clientHeight ? o : { w: s.clientWidth || o.w, h: s.clientHeight || o.h, sb: Math.max(0, s.offsetHeight - s.clientHeight) }))
    const onScroll = (): void => {
      if (!raf) raf = requestAnimationFrame(read)
    }
    size()
    read()
    s.addEventListener('scroll', onScroll, { passive: true })
    const ro = typeof ResizeObserver === 'undefined' ? null : new ResizeObserver(size)
    ro?.observe(s)
    return () => {
      s.removeEventListener('scroll', onScroll)
      ro?.disconnect()
      if (raf) cancelAnimationFrame(raf)
    }
  }, [scroller])

  const tw = Math.max(200, box.w - HW)
  useEffect(() => onMeasure(tw), [tw, onMeasure])
  const ppf = p.ppf === 'fit' ? fitPpf(M, tw) : (p.ppf ?? defaultPpf(M, tw))
  const timeW = timeWidth(M, ppf, tw)
  const now = nowOf(S)
  const heads = playheadsOf(S, M, now)
  const live = S.tier === 'live' && !S.loading
  const mine = heads.find((h) => h.self)
  const monitor = !live ? null : p.monitor !== undefined ? p.monitor : { f: mine?.f ?? 0, colour: 'var(--color-accent)', name: 'your playhead' }

  // Keep the middle in place on a zoom; fit goes to the start; a new sample (or the monitor leaving the screen) brings its playhead to a third in.
  const last = useRef<{ ppf: number; scenario?: string; step?: number; fit: number; centre?: string } | null>(null)
  useLayoutEffect(() => {
    const s = scroller()
    if (!s) return
    const was = last.current
    const at = (x: number): void => {
      s.scrollLeft = Math.max(0, x)
    }
    const monX = (monitor?.f ?? 0) * ppf
    if (!was || was.scenario !== view.scenario) at(monX - tw * 0.35)
    else if (was.fit !== p.fitTick) at(0)
    else if (p.centre && was.centre !== p.centre.key) at(p.centre.f * ppf - tw / 2)
    else if (was.ppf !== ppf) at(((s.scrollLeft + tw / 2) / was.ppf) * ppf - tw / 2)
    else if (was.step !== view.step && (monX < s.scrollLeft + 40 || monX > s.scrollLeft + tw - 40)) at(monX - tw * 0.35)
    last.current = { ppf, scenario: view.scenario, step: view.step, fit: p.fitTick, centre: p.centre?.key }
  })

  usePlayScroll(scroller, monitor ? monitor.f * ppf : null, tw, p.follow, p.onUserScroll)

  const go = (f: number): void => {
    const s = scroller()
    if (s) s.scrollLeft = Math.max(0, f * ppf - tw * 0.35)
  }

  const left = Math.min(pos.left, Math.max(0, timeW - tw))
  const win = drawWindow(left, tw, ppf)
  const [f0, f1] = win
  const v0 = left / ppf
  const v1 = (left + tw) / ppf
  const off = offscreenOf(
    heads.filter(() => live),
    v0,
    v1,
  )
  const asPeople = (hs: typeof heads): LivePerson[] => hs.flatMap((h) => personById(p.people, h.id) ?? [])
  const marks = clipMarks(S, M)
  const ghosts = ghostsOf(S, M)
  const pv = S.pv ?? DEFAULT_PV
  const L = lanesOf(M)
  const split = splitBefore(M)
  const H = lanesHeight(M, prefs.lanes)
  const frameH = Math.max(0, Math.min(H + box.sb, p.maxFrame))
  const more = H - box.h - pos.top > 6
  const big = isBig(M)
  const tmarkers = M.markers.filter((m) => m.owner === 'timeline' && m.f != null)

  return (
    <section
      className="lv-tl-lanes flex-none"
      aria-label="Lanes. Equals and minus zoom; backslash fits the edit."
      tabIndex={0}
      data-k2-focus=""
      data-testid="lv-lanes"
      data-size={prefs.lanes}
      data-pic={prefs.pic}
      style={{ height: RULER_H + frameH + (big ? 14 : 0) }}
      onKeyDown={p.onKey}
    >
      <div ref={probes} aria-hidden="true">
        <span className="lv-tl-probe lv-tl-name">Interview</span>
        <span className="lv-tl-probe lv-tl-tagfont">Interview</span>
      </div>
      <Ruler
        model={boxed(M)}
        ppf={ppf}
        timeW={timeW}
        left={left}
        tw={tw}
        headW={HW}
        narrow={narrow}
        monitor={monitor}
        heads={live ? heads.filter((h) => !h.self && h.f >= f0 && h.f <= f1) : []}
        markers={tmarkers.filter((m) => (m.f as number) >= f0 - 200 && (m.f as number) <= f1).map((m) => ({ key: m.key, name: m.name, f: m.f as number, color: m.color, note: m.note }))}
        brackets={live ? bracketsOf(S) : []}
        loop={p.loop ?? null}
        onBracket={p.onBracket}
        readiness={live ? readinessOf(M, f0, f1, pv, facts) : []}
        window={win}
        stacks={{ left: asPeople(off.left), right: asPeople(off.right) }}
        onStack={(side) => {
          const h = side === 'left' ? off.left[0] : off.right[0]
          if (h) go(h.f)
          p.onJump(side)
        }}
        measureTag={measureTag}
      />
      <div ref={frame} className="relative" style={{ height: frameH }}>
        <ScrollEdgeFrame className="h-full" scrollClassName="lv-tl-scroller" testId="lv-lanes-scroll" tint="var(--lv-tl-lane)">
          <div className="lv-tl-canvas" style={{ width: HW + timeW, height: H }}>
            {L.map((t) => {
              const h = laneHeight(prefs.lanes, t.kind)
              const mode = t.kind === 'video' ? picModeFor(h, prefs.pic) : 'off'
              const wave = t.kind === 'audio' && h >= WAVE_MIN && prefs.pic !== 'off'
              const isSplit = t.key === split
              return (
                <div key={t.key} className="lv-tl-lane" data-kind={t.kind} data-size={prefs.lanes} data-split={isSplit ? '' : undefined} data-testid={`lv-lane-${t.label}`} style={{ height: h + 1 + (isSplit ? 3 : 0) }}>
                  <TrackHeader t={trackFacts(t)} width={HW} narrow={narrow} size={prefs.lanes} height={h} />
                  <div className="lv-tl-body" style={{ width: timeW, height: h }}>
                    {ghosts
                      .filter((g) => g.track === t.key && g.f1 >= f0 && g.f0 <= f1)
                      .map((g, i) => (
                        <i key={'g' + i} className="lv-tl-ghost" data-kind={g.kind} data-old={g.fresh ? undefined : ''} style={{ left: g.f0 * ppf, width: Math.max(2, (g.f1 - g.f0) * ppf), '--lv-c': g.colour } as CSSProperties} />
                      ))}
                    {visibleItems(t, f0, f1).map((it) => {
                      const x = it.f0 * ppf
                      const w = Math.max(2, (it.f1 - it.f0) * ppf)
                      if (it.kind === 'transition')
                        return (
                          <i key={it.key} className="lv-tl-trx" title={`${it.name || 'A transition'} · ${M.dur(it.f1 - it.f0)} (not in the cuts-only preview)`} style={{ left: x, width: w }}>
                            <TransitionShape />
                          </i>
                        )
                      return (
                        <Clip
                          key={it.key}
                          model={boxed(M)}
                          it={it}
                          trackKind={t.kind}
                          x={x}
                          w={w}
                          h={h}
                          a={Math.max(x, left) - x}
                          b={Math.min(x + w, left + tw) - x}
                          mark={marks[it.key]}
                          mode={mode}
                          wave={wave}
                          ppf={ppf}
                          mi={mediaInfo(M, it.media, facts)}
                          facts={facts}
                          pending={(pv.pending || []).includes(it.key)}
                          measure={measureName}
                        />
                      )
                    })}
                    {t.markers
                      .filter((m) => m.f != null && m.f >= f0 && m.f <= f1)
                      .map((m) => (
                        <i key={m.key} className="lv-tl-tmk" title={`${m.name} (on ${t.label})`} style={{ left: (m.f as number) * ppf, '--lv-mc': markerColour(m.color) } as CSSProperties}>
                          <TrackMarkShape />
                        </i>
                      ))}
                  </div>
                </div>
              )
            })}
            <div className="lv-tl-lines" style={{ left: HW, width: timeW, height: H }} aria-hidden="true">
              {tmarkers
                .filter((m) => (m.f as number) >= f0 && (m.f as number) <= f1)
                .map((m) => (
                  <i key={m.key} className="lv-tl-mkline" style={{ left: (m.f as number) * ppf, '--lv-mc': markerColour(m.color) } as CSSProperties} />
                ))}
              {live
                ? heads
                    .filter((h) => !h.self && h.f >= f0 && h.f <= f1)
                    .map((h) => <i key={h.id} className="lv-tl-ph" data-away={h.away ? '' : undefined} style={{ left: h.f * ppf, '--lv-c': h.colour } as CSSProperties} />)
                : null}
              {monitor ? <i className="lv-tl-ph" data-mon="" data-testid="lv-monitor-line" style={{ left: monitor.f * ppf, '--lv-c': monitor.colour } as CSSProperties} /> : null}
            </div>
          </div>
        </ScrollEdgeFrame>
        {more ? (
          <button
            type="button"
            className="lv-tl-more"
            data-k2-focus=""
            data-k2-chamfer="chip"
            data-testid="lv-more-lanes"
            title="Scroll to the lanes below"
            onClick={() => scroller()?.scrollTo({ top: H, behavior: 'smooth' })}
          >
            <DownGlyph size={13} />
            More lanes below
          </button>
        ) : null}
      </div>
      {big ? <Overview model={boxed(M)} v0={v0} v1={v1} heads={live ? heads : []} onGo={go} /> : null}
    </section>
  )
}
