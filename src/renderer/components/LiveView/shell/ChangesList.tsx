// Changes: one row per change, newest first. Each says who did what in one
// sentence with one verb ("Atlas trimmed the start of Interview B by 1s"),
// when apart from where ("2 min ago" · Go there), and Undo where the view
// offers it (the editors). Outside saves and refusals are rows too, quieter.

import { cx } from '@/components/ui'
import { agoText, personById, type LivePerson } from '@/lib/live-view/shared/people'
import type { LiveChange } from '@/lib/live-view/shared/source'
import { PanelSection } from './PanelSection'

const OUTSIDE = { name: 'Saved outside K2', colour: 'var(--color-text-muted)' }

export function ChangesList({
  changes,
  people,
  now,
  title = 'Changes',
  onGoThere,
  onUndo,
  onRedo,
  onAsk,
  undoOff = null,
  undoTip,
  empty = 'No changes yet',
}: {
  /** Oldest first, as the source keeps them. */
  changes: readonly LiveChange[]
  people: readonly LivePerson[]
  now: number
  title?: string
  onGoThere?: (c: LiveChange) => void
  onUndo?: (c: LiveChange) => void
  /** The editors: Redo on a row you undid. */
  onRedo?: (c: LiveChange) => void
  /** A row's second action ("Ask Sam"). */
  onAsk?: (c: LiveChange) => void
  /** Why Undo and Redo are off now (still focusable, the reason as the tooltip). */
  undoOff?: string | null
  /** Undo's tooltip ("⌘Z undoes only your own edits; this undoes this change"). */
  undoTip?: string
  /** The line when there are no rows. */
  empty?: string
}): React.JSX.Element {
  const rows = [...changes].sort((a, b) => b.at_ms - a.at_ms)
  return (
    <PanelSection id="changes" title={title} count={rows.length} countTestId="lv-changes-count">
      {rows.length === 0 ? <p className="type-hint m-0 px-3.5 py-1 text-[var(--color-text-secondary)]">{empty}</p> : null}
      <ul className="m-0 list-none p-0">
        {rows.map((c) => {
          const p = personById(people, c.who)
          const who = p ? { name: p.self ? 'You' : p.name, colour: p.self ? 'var(--color-accent-fill)' : p.colour } : c.who === 'file' ? OUTSIDE : { name: c.who, colour: 'var(--color-text-muted)' }
          const sentence = `${who.name} ${c.undone ? `undid this: ${c.text}` : c.text}`
          return (
            <li key={c.id} data-testid="lv-change" data-tone={c.tone} data-undone={c.undone ? '' : undefined} className="lv-change flex items-start gap-2 px-3.5 py-1.5">
              <span aria-hidden="true" className="lv-change-dot mt-[6px] flex-none" style={{ ['--lv-c' as string]: who.colour }} />
              <span className="flex min-w-0 flex-1 flex-col gap-0.5">
                <span data-lv-change-text="" className={cx('type-row text-[var(--color-text-primary)]', c.tone ? 'text-[var(--color-text-secondary)]' : '')}>
                  <b className="font-semibold text-[var(--color-text-primary)]">{who.name}</b> {c.undone ? `undid this: ${c.text}` : c.text}
                </span>
                <span className="type-hint flex items-baseline gap-2 text-[var(--color-text-secondary)]">
                  <span>{agoText(c.at_ms, now)}</span>
                  {c.target && onGoThere ? (
                    <button type="button" data-k2-focus="" aria-label={`Go there: ${sentence}`} onClick={() => onGoThere(c)} className="lv-link no-drag cursor-pointer">
                      Go there
                    </button>
                  ) : null}
                  {c.undoable && !c.undone && onUndo ? (
                    <button type="button" data-k2-focus="" aria-label={`Undo: ${sentence}`} aria-disabled={undoOff ? true : undefined} title={undoOff ?? undoTip} onClick={() => (undoOff ? undefined : onUndo(c))} className="lv-link no-drag cursor-pointer">
                      Undo
                    </button>
                  ) : null}
                  {c.undoable && c.undone && onRedo ? (
                    <button type="button" data-k2-focus="" aria-label={`Redo: ${sentence}`} aria-disabled={undoOff ? true : undefined} title={undoOff ?? undefined} onClick={() => (undoOff ? undefined : onRedo(c))} className="lv-link no-drag cursor-pointer">
                      Redo
                    </button>
                  ) : null}
                  {c.ask && onAsk ? (
                    <button type="button" data-k2-focus="" onClick={() => onAsk(c)} className="lv-link no-drag cursor-pointer">
                      Ask {c.ask.name}
                    </button>
                  ) : null}
                </span>
              </span>
            </li>
          )
        })}
      </ul>
    </PanelSection>
  )
}
