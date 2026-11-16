// The ruler over the lanes (56 px): the big counter in its corner (the
// monitor's timecode; at 800 px it lives in the transport); then, scrolled
// with the lanes, the top row (other people's playhead tags and marker
// flags in one 12px tag style), the timecode labels, the change brackets,
// ticks rising from the bottom, the readiness line and the monitor's head.
// The off-screen stacks sit at its ends: an avatar stack with an arrow,
// names in the tooltip. No words besides names and timecodes.

import type { CSSProperties } from 'react'
import { personInk } from '@/components/HomeChat/HomeChatGlyphs'
import type { LivePerson } from '@/lib/live-view/shared/people'
import type { TimelineModel } from '@/lib/live-view/timeline/fold'
import type { Bracket, Readiness } from '@/lib/live-view/timeline/lanes-view'
import { layoutRulerTop, rulerTicks, type RulerHead, type RulerMarker, type Span } from '@/lib/live-view/timeline/ruler'
import { EdgeStack } from '../shell/EdgeStack'
import type { Box } from './box'
import { markerColour } from './Clip'
import { FlagShape, HeadShape } from './glyphs'

const HEX = /^#[0-9a-f]{6}$/i

/** About how wide an off-screen stack is (its arrow and up to three avatars), so tags step aside from it. */
export const stackWidth = (n: number): number => (n ? 30 + Math.min(3, n) * 18 : 0)

export interface RulerProps {
  /** The model, boxed (box.ts). */
  model: Box<TimelineModel>
  ppf: number
  timeW: number
  left: number
  tw: number
  headW: number
  narrow: boolean
  /** The monitor's playhead (yours until you follow someone). */
  monitor: { f: number; colour: string; name: string } | null
  heads: RulerHead[]
  markers: RulerMarker[]
  brackets: Bracket[]
  readiness: Readiness[]
  window: [number, number]
  stacks: { left: LivePerson[]; right: LivePerson[] }
  /** The loop that runs (from 1 s before a change to 1 s after), shaded; a bracket's click loops its change. */
  loop?: { f0: number; f1: number } | null
  onBracket?: (b: Bracket) => void
  onStack: (side: 'left' | 'right') => void
  measureTag: (text: string) => number
}

export function Ruler(p: RulerProps): React.JSX.Element {
  const { ppf, timeW, left, tw, headW, monitor } = p
  const M = p.model.v
  const [f0, f1] = p.window
  const ticks = rulerTicks(M, ppf, f0, f1)
  const reserved: Span[] = []
  if (p.stacks.left.length) reserved.push({ x0: left, x1: left + 4 + stackWidth(p.stacks.left.length) })
  if (p.stacks.right.length) reserved.push({ x0: left + tw - 4 - stackWidth(p.stacks.right.length), x1: left + tw })
  const top = layoutRulerTop(p.heads, p.markers, ppf, p.measureTag, reserved, timeW)
  let lv: number[] = []
  const brk = p.brackets
    .filter((b) => b.f1 >= f0 && b.f0 <= f1)
    .map((b) => {
      const x0 = b.f0 * ppf
      const x1 = Math.max(x0 + 6, b.f1 * ppf)
      let l = 0
      while (lv[l] != null && lv[l] > x0 - 2 && l < 2) l++
      lv = lv.slice()
      lv[l] = x1
      return { b, x0, x1, l }
    })
  return (
    <div className="lv-tl-ruler" data-testid="lv-ruler">
      <div className="lv-tl-corner" style={{ width: headW }}>
        {!p.narrow ? (
          <span className="lv-tl-counter" data-testid="lv-counter" title="The monitor’s timecode">
            {monitor && !M.T.provisional ? M.tcF(monitor.f) : '--:--:--:--'}
          </span>
        ) : null}
      </div>
      <div className="lv-tl-rclip">
        <div className="lv-tl-rbody" style={{ width: timeW, transform: `translateX(${-left}px)` }} data-testid="lv-ruler-body">
          {p.loop ? (
            // The loop's span, its 1 s lead-in and tail shaded deeper, as an in/out range.
            <i className="lv-tl-loop" data-testid="lv-ruler-loop" aria-hidden="true" style={{ left: p.loop.f0 * ppf, width: Math.max(2, (p.loop.f1 - p.loop.f0) * ppf), '--lv-lead': `${(M.T.rate ? M.T.rate.fps : 24) * ppf}px` } as CSSProperties} />
          ) : null}
          {ticks.map((t) => (
            <i key={'t' + t.f} className="lv-tl-tick" data-major={t.major ? '' : undefined} style={{ left: t.x }} />
          ))}
          {ticks
            .filter((t) => t.label)
            .map((t) => (
              <span key={'l' + t.f} className="lv-tl-tl" data-testid="lv-ruler-label" style={{ left: t.x }}>
                {t.label}
              </span>
            ))}
          {brk.map(({ b, x0, x1, l }, i) => (
            <i
              key={'b' + i}
              className="lv-tl-brk"
              data-testid="lv-bracket"
              data-fresh={b.fresh ? '' : undefined}
              title={`${b.line}. Click to loop it.`}
              style={{ left: x0, width: x1 - x0, top: 35 + l * 4, '--lv-c': b.colour } as CSSProperties}
              onClick={p.onBracket ? () => p.onBracket?.(b) : undefined}
            />
          ))}
          {p.readiness.map((r, i) => (
            <i key={'r' + i} className="lv-tl-rdy" data-kind={r.kind} style={{ left: r.f0 * ppf, width: Math.max(1, (r.f1 - r.f0) * ppf) }} />
          ))}
          {top.flags.map((g) => {
            const m = g.markers[0]
            const names = g.markers.map((x) => x.name)
            const tip = names.length > 1 ? names.slice(0, 8).join(', ') + (names.length > 8 ? ` and ${names.length - 8} more` : '') : [m.name, M.tcF(m.f), m.note].filter(Boolean).join(' · ')
            return (
              <span key={m.key}>
                <i className="lv-tl-flag" data-testid="lv-ruler-flag" title={tip} style={{ left: g.x, '--lv-mc': markerColour(m.color) } as CSSProperties}>
                  <FlagShape />
                </i>
                {g.label ? (
                  <span className="lv-tl-tag lab" data-marker="" data-k2-chamfer="chip" data-testid="lv-ruler-tag" title={`Marker: ${tip}`} style={{ left: g.label.x0, width: g.label.w }}>
                    {g.label.text}
                  </span>
                ) : null}
              </span>
            )
          })}
          {top.tags.map((t) => (
            <span
              key={t.head.id}
              className="lv-tl-tag lab"
              data-person=""
              data-away={t.head.away ? '' : undefined}
              data-k2-chamfer="chip"
              data-testid="lv-ruler-tag"
              title={`${t.head.name}’s playhead · ${M.tcF(t.head.f)}`}
              style={{ left: t.x0, width: t.w, '--lv-c': t.head.colour, '--lv-ink': HEX.test(t.head.colour) ? personInk(t.head.colour) : 'var(--color-on-accent)' } as CSSProperties}
            >
              {t.head.name}
            </span>
          ))}
          {monitor ? (
            <i className="lv-tl-mon" title={`The monitor: ${monitor.name} · ${M.tcF(monitor.f)}`} style={{ left: monitor.f * ppf, '--lv-c': monitor.colour } as CSSProperties}>
              <HeadShape />
            </i>
          ) : null}
        </div>
        {p.stacks.left.length ? <EdgeStack side="left" people={p.stacks.left} onJump={() => p.onStack('left')} className="lv-tl-stack left-1" /> : null}
        {p.stacks.right.length ? <EdgeStack side="right" people={p.stacks.right} onJump={() => p.onStack('right')} className="lv-tl-stack right-1" /> : null}
      </div>
    </div>
  )
}
