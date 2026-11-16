// What sits on the picture besides the still: the card (opening, no still
// at all, no picture yet, an empty scene), the state chips along the
// stage's foot (updating, couldn't update with Retry, picture updated, the
// quiet notes), the key hint on its own plate, and the older-picture mark.
// The words come from lib/live-view/scene/picture-words.ts.

import { Button } from '@/components/ui'
import { KEY_HINT } from '@/lib/live-view/scene/views'
import type { CardAction, PictureCard as Card, PictureChip } from '@/lib/live-view/scene/picture-words'
import { InfoGlyph, OkGlyph, RetryGlyph, WarnGlyph } from '../shell/glyphs'
import { DoneGlyph, PictureGlyph, SceneViewGlyph } from './scene-glyphs'

export function PictureCard({ card, onAction }: { card: Card; onAction: (id: CardAction) => void }): React.JSX.Element {
  if (card.kind === 'opening') {
    return (
      <>
        <div className="lv-skel" aria-hidden="true" />
        <div className="lv-card" data-quiet="" data-k2-chamfer="surface" data-testid="lv-scene-card" role="status">
          <h3>{card.title}</h3>
          <ol className="lv-steps">
            {(card.steps ?? []).map((s) => (
              <li key={s.label} data-state={s.state}>
                {s.state === 'done' ? <DoneGlyph size={12} /> : s.state === 'now' ? <span className="lv-spin" aria-hidden="true" /> : <span className="lv-pend" aria-hidden="true" />}
                {s.label}
              </li>
            ))}
          </ol>
        </div>
      </>
    )
  }
  const bad = card.kind === 'nostill'
  return (
    <div className="lv-card" data-top={card.atTop ? '' : undefined} data-k2-chamfer="surface" data-testid="lv-scene-card" data-kind={card.kind}>
      <div className="lv-big" data-bad={bad ? '' : undefined} aria-hidden="true">
        {bad ? <WarnGlyph size={18} /> : card.kind === 'empty' ? <SceneViewGlyph size={18} /> : <PictureGlyph size={18} />}
      </div>
      <h3>{card.title}</h3>
      {card.body ? <p>{card.body}</p> : null}
      {card.actions.length ? (
        <div className="lv-btns">
          {card.actions.map((a) => (
            <Button key={a.id} variant={a.primary ? 'accent' : 'default'} size="md" data-k2-focus="" data-k2-chamfer="control" className="inline-flex items-center gap-1.5 rounded-[var(--radius-field)]" onClick={() => onAction(a.id)}>
              {a.id === 'retry' ? <RetryGlyph size={13} /> : null}
              {a.label}
            </Button>
          ))}
        </div>
      ) : null}
    </div>
  )
}

export function PictureChips({ chips, onRetry }: { chips: PictureChip[]; onRetry: () => void }): React.JSX.Element {
  return (
    <div className="lv-chips" data-testid="lv-scene-chips" aria-live="polite">
      {chips.map((c) => (
        <span key={c.id} className="lv-chip lv-plate" data-k2-chamfer="chip" data-tone={c.tone} title={c.title}>
          {c.tone === 'busy' ? <span className="lv-spin" aria-hidden="true" /> : c.tone === 'bad' ? <WarnGlyph size={13} /> : c.tone === 'ok' ? <OkGlyph size={13} /> : <InfoGlyph size={13} />}
          {c.text}
          {c.action ? (
            <button type="button" className="lv-lnk" data-k2-focus="" onClick={onRetry}>
              {c.action.label}
            </button>
          ) : null}
        </span>
      ))}
    </div>
  )
}

/** "Home Scene · 0 Camera · . Frame · Esc Back", on its own plate. */
export function KeyHint(): React.JSX.Element {
  return (
    <span className="lv-keys lv-plate" data-k2-chamfer="chip" data-testid="lv-scene-keys" title="Keys on the picture">
      {KEY_HINT.map((k) => (
        <span key={k.key} className="contents">
          <kbd>{k.key}</kbd>
          {k.label}
        </span>
      ))}
    </span>
  )
}

export function OlderMark({ text }: { text: string }): React.JSX.Element {
  return (
    <div className="lv-vold">
      <span className="lv-oldpic lv-plate" data-k2-chamfer="chip">
        <WarnGlyph size={13} />
        {text}
      </span>
    </div>
  )
}
