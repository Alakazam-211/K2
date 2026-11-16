// The card in the monitor's place when there's no edit to show (T6): the
// file opening part by part, no clips yet, or a file that can't be watched
// live. The words come from stage-card.ts; the actions go to the tab
// (Ask focuses the say row; Open saved versions goes to the shell's
// notice handler).

import { Spinner } from '@/components/ui'
import type { CardAction, StageCard } from '@/lib/live-view/timeline/stage-card'
import { CheckGlyph, FilmCardGlyph } from './glyphs'
import { WarnGlyph } from '../shell/glyphs'

export function StageCardView({ card, onAction }: { card: StageCard; onAction: (id: CardAction['id']) => void }): React.JSX.Element {
  if (card.kind === 'opening')
    return (
      <div className="lv-mon-card lv-mon-open" role="status" data-k2-chamfer="surface" data-testid="lv-mon-stage" data-kind="opening">
        <b>{card.title}</b>
        <span className="lv-mon-sub">{card.line}</span>
        <span className="lv-mon-bar" aria-hidden="true">
          <i style={{ width: `${card.progress}%` }} />
        </span>
        <ul className="lv-mon-rows">
          {card.rows.map((r) => (
            <li key={r.text} data-state={r.state}>
              <span className="lv-mon-st" aria-hidden="true">
                {r.state === 'done' ? <CheckGlyph size={12} /> : r.state === 'now' ? <Spinner className="!h-3 !w-3" /> : null}
              </span>
              <span>{r.text}</span>
              {r.n ? <span className="lv-mon-n lv-num">{r.n.toLocaleString('en')}</span> : null}
            </li>
          ))}
        </ul>
      </div>
    )
  return (
    <div className="lv-mon-card" role={card.kind === 'declined' ? 'alert' : 'status'} data-k2-chamfer="surface" data-testid="lv-mon-stage" data-kind={card.kind} data-tone={card.kind === 'declined' ? 'warn' : undefined}>
      {card.kind === 'declined' ? <WarnGlyph size={26} /> : <FilmCardGlyph size={26} />}
      <b>{card.title}</b>
      <span className="lv-mon-sub">{card.line}</span>
      <div className="lv-mon-acts">
        {card.actions.map((a) => (
          <button key={a.id} type="button" className="lv-mon-btn" data-primary={a.primary ? '' : undefined} data-k2-focus="" data-k2-chamfer="control" onClick={() => onAction(a.id)}>
            {a.label}
          </button>
        ))}
      </div>
    </div>
  )
}
