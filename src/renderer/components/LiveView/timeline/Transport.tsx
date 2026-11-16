// The transport under the monitor (filmcraft's layout, watching only): a
// scrub bar of the whole edit (each change as a span, the timeline's
// markers, everyone's playhead, the loop, and the monitor's own head), then
// one row: whose playhead it is on the left; the buttons centred, so Play
// sits on the picture's centre line (the row's side columns are equal); and
// on the right Back to X, loop, zoom and fit, View, the key hint and the
// mode word, which fold away from the hint inwards when the row is tight.
// At 800 px the big counter hides, so the timecode sits over whose playhead.

import { useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { Button } from '@/components/ui'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { TimelineModel } from '@/lib/live-view/timeline/fold'
import type { WatchKey } from '@/lib/live-view/timeline/keys'
import type { MonitorView } from '@/lib/live-view/timeline/monitor-view'
import { scrubAt, scrubFrame, type ScrubMarks } from '@/lib/live-view/timeline/transport'
import { LiveAvatar } from '../shell/LiveAvatar'
import type { Box } from './box'
import { markerColour } from './Clip'
import { FrameBackGlyph, FrameFwdGlyph, KeysGlyph, LoopGlyph, NextEditGlyph, PauseGlyph, PlayGlyph, PrevEditGlyph } from './glyphs'

const pc = (x: number): string => `${(100 * x).toFixed(3)}%`

export function ScrubBar({ model, marks, loop, onScrub }: { model: Box<TimelineModel>; marks: ScrubMarks; loop: { f0: number; f1: number } | null; onScrub: (f: number) => void }): React.JSX.Element {
  const M = model.v
  const at = (f: number): string => pc(scrubAt(M, f))
  const w = (a: number, b: number): string => pc(Math.max(0, scrubAt(M, b) - scrubAt(M, a)))
  return (
    <div
      className="lv-tl-scrub"
      data-testid="lv-scrub"
      role="slider"
      tabIndex={-1}
      aria-label="The whole edit: click to move your playhead"
      aria-valuemin={0}
      aria-valuemax={Math.max(0, Math.round(M.endF) - 1)}
      aria-valuenow={Math.round(marks.head)}
      aria-valuetext={M.tcF(marks.head)}
      title="The whole edit: click to move your playhead"
      onMouseDown={(e) => {
        const r = e.currentTarget.getBoundingClientRect()
        onScrub(scrubFrame(M, e.clientX - r.left, r.width))
      }}
    >
      <i className="lv-sb" />
      {loop ? <i className="lv-sl" data-testid="lv-scrub-loop" style={{ left: at(loop.f0), width: w(loop.f0, loop.f1) }} /> : null}
      {marks.changes.map((c, i) => (
        <i key={'c' + i} className="lv-sc" data-testid="lv-scrub-change" title={c.title} style={{ left: at(c.f0), width: w(c.f0, c.f1), '--lv-c': c.colour } as CSSProperties} />
      ))}
      {marks.markers.map((m) => (
        <i key={m.key} className="lv-sm" data-testid="lv-scrub-marker" title={m.title} style={{ left: at(m.f), '--lv-mc': markerColour(m.color) } as CSSProperties} />
      ))}
      {marks.heads.map((h) => (
        <i key={h.id} className="lv-sp" data-testid={`lv-scrub-head-${h.id}`} title={h.title} style={{ left: at(h.f), '--lv-c': h.colour } as CSSProperties} />
      ))}
      <i className="lv-sh" data-testid="lv-scrub-now" style={{ left: at(marks.head) }} />
    </div>
  )
}

/** The model and the view travel boxed (box.ts): React's dev prop diff can't serialise their BigInt ticks. */
export interface TransportProps {
  model: Box<TimelineModel>
  view: Box<MonitorView>
  marks: ScrubMarks
  who: string
  person: LivePerson | null
  narrow: boolean
  agentName: string
  hasEdits: boolean
  canLoop: boolean
  loop: { f0: number; f1: number } | null
  back: React.ReactNode
  /** Zoom and fit, and the View menu (the lanes' settings). */
  viewMenu: React.ReactNode
  zoom: React.ReactNode
  keysHint: string
  onKey: (k: WatchKey) => void
  onPlay: () => void
  onLoop: () => void
  onScrub: (f: number) => void
  /** Following ended because they stopped or left: the line, Back to my place and Follow again (none once they've left). */
  finished?: { text: string; back: string; again: string | null; onBack: () => void; onAgain: () => void } | null
}

/** How far the right group folds when it doesn't fit: 1 hides the key hint, 2 zoom (the keys still zoom), 3 loop, 4 the mode word (its words stay in details). */
function useFold(deps: unknown[]): [React.RefObject<HTMLDivElement | null>, number] {
  const ref = useRef<HTMLDivElement>(null)
  const [fold, setFold] = useState(0)
  const [w, setW] = useState(0)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(() => setW(el.clientWidth))
    ro.observe(el)
    return () => ro.disconnect()
  }, [])
  useLayoutEffect(() => setFold(0), [w, ...deps]) // eslint-disable-line react-hooks/exhaustive-deps
  useLayoutEffect(() => {
    const el = ref.current
    if (el && fold < 4 && el.scrollWidth > el.clientWidth + 1) setFold(fold + 1)
  })
  return [ref, fold]
}

export function Transport(p: TransportProps): React.JSX.Element {
  const M = p.model.v
  const v = p.view.v
  const playing = v.playing
  const [right, fold] = useFold([p.back ? 1 : 0, p.finished ? (p.finished.again ? 2 : 1) : 0, p.narrow])
  const sm = 'lv-tl-tb !border-transparent !bg-transparent'
  return (
    <div className="lv-tl-transport" data-testid="lv-transport">
      <ScrubBar model={p.model} marks={p.marks} loop={p.loop} onScrub={p.onScrub} />
      <div className="lv-tl-trow" data-testid="lv-transport-row">
        <div className="lv-tl-tleft">
          {p.loop ? (
            <span className="inline-flex text-[var(--color-text-secondary)]" aria-hidden="true">
              <LoopGlyph size={16} />
            </span>
          ) : p.person ? (
            <LiveAvatar person={p.person} size={18} />
          ) : null}
          <div className="lv-tl-tpl">
            {p.narrow ? (
              <span className="lv-tl-tcs" data-testid="lv-transport-tc">
                {M.tcF(v.head.f)}
              </span>
            ) : null}
            {p.finished ? (
              <span className="lv-tl-tpw" role="status" data-testid="lv-finished" title={p.finished.text}>
                {p.finished.text}
              </span>
            ) : (
              <span className="lv-tl-tpw" role="status" data-testid="lv-transport-who" title={p.who}>
                {p.who}
              </span>
            )}
          </div>
        </div>
        <div className="lv-tl-tbtns" role="group" aria-label="Playback" data-testid="lv-transport-buttons">
          {p.narrow ? null : (
            <Button variant="quiet" className={sm} data-size="sm" aria-label={`${p.agentName}’s previous edit`} title={`${p.agentName}’s previous edit (↑)`} disabled={!p.hasEdits} data-testid="lv-prev-edit" onClick={() => p.onKey('edit-')}>
              <PrevEditGlyph size={13} />
            </Button>
          )}
          <Button variant="default" className="lv-tl-tb !text-[var(--color-text-primary)]" aria-label="Back a frame" title="Back a frame (←)" data-testid="lv-frame-back" onClick={() => p.onKey('frame-')}>
            <FrameBackGlyph size={14} />
          </Button>
          <Button variant="accent" className="lv-tl-tb" data-play="" aria-label={playing ? 'Pause' : 'Play'} title={v.playTip} disabled={!v.canPlay && !playing} data-testid="lv-play" onClick={p.onPlay}>
            {playing ? <PauseGlyph size={15} /> : <PlayGlyph size={15} />}
          </Button>
          <Button variant="default" className="lv-tl-tb !text-[var(--color-text-primary)]" aria-label="Forward a frame" title="Forward a frame (→)" data-testid="lv-frame-fwd" onClick={() => p.onKey('frame+')}>
            <FrameFwdGlyph size={14} />
          </Button>
          {p.narrow ? null : (
            <Button variant="quiet" className={sm} data-size="sm" aria-label={`${p.agentName}’s next edit`} title={`${p.agentName}’s next edit (↓)`} disabled={!p.hasEdits} data-testid="lv-next-edit" onClick={() => p.onKey('edit+')}>
              <NextEditGlyph size={13} />
            </Button>
          )}
        </div>
        <div ref={right} className="lv-tl-tright" data-fold={fold || undefined} data-testid="lv-transport-right">
          {p.back}
          {p.finished ? (
            <>
              <Button variant="default" size="sm" className="!text-[var(--color-text-primary)]" data-testid="lv-back-home" onClick={p.finished.onBack}>
                {p.finished.back}
              </Button>
              {p.finished.again ? (
                <Button variant="accent" size="sm" data-testid="lv-follow-again" onClick={p.finished.onAgain}>
                  {p.finished.again}
                </Button>
              ) : null}
            </>
          ) : null}
          <Button
            variant="quiet"
            className={`${sm}${p.loop ? ' !bg-[var(--color-control-hover-quiet)] !text-[var(--color-text-primary)]' : ''}`}
            data-size="sm"
            aria-pressed={!!p.loop}
            aria-label={p.loop ? 'Stop the loop' : 'Loop the latest change'}
            title={p.loop ? 'Stop the loop (Esc)' : p.canLoop ? `Loop ${p.agentName}’s latest change` : `${p.agentName} hasn’t changed anything to loop yet`}
            disabled={!p.canLoop && !p.loop}
            data-testid="lv-loop"
            onClick={p.onLoop}
          >
            <LoopGlyph size={13} />
          </Button>
          {p.zoom}
          {p.viewMenu}
          <span className="lv-tl-khint" tabIndex={0} data-k2-focus="" role="note" title={p.keysHint} aria-label={p.keysHint} data-testid="lv-keys-hint">
            <KeysGlyph size={15} />
          </span>
          <span className="lv-tl-mode" tabIndex={0} data-k2-focus="" data-k2-chamfer="chip" title={v.mode.title} data-testid="lv-mode">
            {v.mode.word}
          </span>
        </div>
      </div>
    </div>
  )
}
