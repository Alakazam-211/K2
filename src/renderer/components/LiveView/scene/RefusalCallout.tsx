// A refusal on the picture (live-scene-v2 mock, "Refused on the render"): a
// pin on the refused object's box, a line, and the callout: a lock (or a
// warning when no one holds it), the shell's words with who in bold, and
// the shell's "Ask …" when there is someone here to ask. In the holder's
// presence colour, else the warning colour; the code is only in the
// tooltip. SceneOverlay places it (first, so tags keep clear of it).

import type { CSSProperties } from 'react'
import { Button } from '@/components/ui'
import type { CalloutSpec } from '@/lib/live-view/scene/callout'
import type { Placed, Rect } from '@/lib/live-view/scene/labels'
import { slotVar } from '@/lib/live-view/scene/overlay'
import { WarnGlyph } from '../shell/glyphs'
import type { HeroRefusal } from '../shell/shell-config'
import { LockGlyph } from './scene-glyphs'

/** The callout's words: who in bold, then the rest of the shell's sentence. */
function Words({ r }: { r: HeroRefusal }): React.JSX.Element {
  const t = r.view.title
  const who = r.notice.actor
  return t.startsWith(who) ? (
    <>
      <b>{who}</b>
      {t.slice(who.length)}
    </>
  ) : (
    <>{t}</>
  )
}

export function RefusalCallout({
  spec,
  refusal,
  anchor,
  pos,
  pane,
  boxRef,
}: {
  spec: CalloutSpec
  refusal: HeroRefusal
  /** The object's box, in the stage's pixels. */
  anchor: Rect
  pos: Placed | undefined
  pane: { width: number; height: number }
  boxRef: (e: HTMLElement | null) => void
}): React.JSX.Element {
  const tint = { ['--lv-c' as string]: spec.slot ? slotVar(spec.slot) : 'var(--color-status-warn)' } as CSSProperties
  const ask = refusal.view.actions.find((a) => a.id === 'ask')
  const cx = (anchor.x0 + anchor.x1) / 2
  const below = pos ? pos.y >= anchor.y1 : true
  const pin = { x: cx, y: below ? anchor.y1 : anchor.y0 }
  const end = pos ? { x: Math.max(pos.x + 14, Math.min(pos.x + pos.w - 14, cx)), y: below ? pos.y : pos.y + pos.h } : null
  return (
    <>
      {pos && end ? (
        <>
          <svg className="lv-cline" data-testid="lv-callout-line" viewBox={`0 0 ${pane.width} ${pane.height}`} style={tint} aria-hidden="true">
            <path d={`M${pin.x.toFixed(1)} ${pin.y.toFixed(1)}L${end.x.toFixed(1)} ${end.y.toFixed(1)}`} />
          </svg>
          <i className="lv-cpin" data-testid="lv-callout-pin" style={{ ...tint, left: pin.x, top: pin.y }} aria-hidden="true" />
        </>
      ) : (
        <i className="lv-cpin" data-testid="lv-callout-pin" style={{ ...tint, left: pin.x, top: pin.y, visibility: 'hidden' }} aria-hidden="true" />
      )}
      <div
        ref={boxRef}
        role="status"
        className="lv-callout"
        data-testid="lv-callout"
        data-a={spec.a}
        data-warn={spec.held ? undefined : ''}
        data-k2-chamfer="surface"
        title={spec.tip}
        style={{ ...tint, ...(pos ? { left: pos.x, top: pos.y } : { left: 0, top: 0, visibility: 'hidden' }) }}
      >
        <span className="lv-lk" aria-hidden="true">
          {spec.held ? <LockGlyph size={14} /> : <WarnGlyph size={14} />}
        </span>
        <span className="lv-ct">
          <Words r={refusal} />
        </span>
        {ask ? (
          <Button size="sm" data-k2-focus="" className="lv-cask text-[var(--color-text-primary)]" onClick={() => refusal.onAction('ask')}>
            {ask.label}
          </Button>
        ) : null}
      </div>
    </>
  )
}
