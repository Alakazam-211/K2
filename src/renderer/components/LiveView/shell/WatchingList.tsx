// Watching: who's here, one line each (name, "agent", what they're doing
// or where they are), you first. "Watching N" counts only people who are
// here; away people sit under a quiet "Away" line, faded. Clicking someone
// follows them, and the one you follow says "You're following". While the
// tab isn't connected it reads "as you last saw", greyed, with nothing to
// click.

import { cx } from '@/components/ui'
import { watching, type LivePerson } from '@/lib/live-view/shared/people'
import { LiveAvatar } from './LiveAvatar'
import { PanelSection } from './PanelSection'

function Row({ p, following, stale, onFollow }: { p: LivePerson; following: boolean; stale: boolean; onFollow?: (id: string) => void }): React.JSX.Element {
  const status = p.doing ?? p.place ?? (p.kind === 'agent' ? 'in the file, not editing' : 'watching')
  const inner = (
    <>
      <LiveAvatar person={p} size={22} grey={stale} />
      <span className="flex min-w-0 flex-1 flex-col">
        <span className="type-row flex items-baseline gap-1.5 text-[var(--color-text-primary)]">
          <span className="whitespace-nowrap">{p.self ? 'You' : p.name}</span>
          {p.kind === 'agent' ? <span className="lv-caption">agent</span> : null}
        </span>
        <span className={cx('lv-status type-hint text-[var(--color-text-secondary)]', stale ? 'italic' : '')} title={p.doingTip ?? status}>
          {status}
        </span>
      </span>
      {following ? (
        <span data-k2-chamfer="chip" className="lv-caption flex-none rounded-[var(--radius-badge)] border border-[var(--color-control-edge)] px-1.5">
          You’re following
        </span>
      ) : null}
    </>
  )
  const cls = 'lv-person flex w-full items-start gap-2.5 px-3.5 py-1.5 text-left'
  if (p.self || stale || !onFollow)
    return (
      <div data-testid={`lv-person-${p.id}`} data-away={p.away ? '' : undefined} className={cls}>
        {inner}
      </div>
    )
  return (
    <button
      type="button"
      data-k2-focus=""
      data-testid={`lv-person-${p.id}`}
      data-away={p.away ? '' : undefined}
      aria-pressed={following}
      aria-label={`${p.name}${p.kind === 'agent' ? ' agent' : ''}, ${status}${p.away ? ', away' : ''}${following ? '. You’re following them' : ''}`}
      title={following ? `You’re following ${p.name}` : `Follow ${p.name}`}
      onClick={() => onFollow(p.id)}
      className={cx(cls, 'lv-row-btn no-drag cursor-pointer rounded-[var(--radius-selector)]', following ? 'lv-row-on' : '')}
      style={{ ['--lv-c' as string]: p.colour }}
    >
      {inner}
    </button>
  )
}

export function WatchingList({
  people,
  followingId = null,
  stale = false,
  title = 'Watching',
  countAway = false,
  staleHead,
  action,
  onFollow,
}: {
  people: readonly LivePerson[]
  followingId?: string | null
  /** Not connected: as you last saw them. */
  stale?: boolean
  title?: string
  /** The editors' heads: "Away N" counts too. */
  countAway?: boolean
  /** The editors' head while not connected ("Last seen 3", then why); else "as you last saw". */
  staleHead?: { title: string; line: string }
  /** At the heading's end ("Live edits"). */
  action?: React.ReactNode
  /** None: rows only show who's here (Follow comes with E4b). */
  onFollow?: (id: string) => void
}): React.JSX.Element {
  const w = watching(people)
  const head = stale && staleHead ? staleHead : null
  return (
    <PanelSection id="watching" title={head?.title ?? title} count={stale && !head ? 'as you last saw' : w.count} countTestId="lv-watching-count" action={action}>
      {head ? <p className="lv-stale-line type-hint m-0 px-3.5 pb-1 text-[var(--color-text-secondary)]">{head.line}</p> : null}
      <div data-testid="lv-watching" data-stale={stale ? '' : undefined}>
        {w.here.map((p) => (
          <Row key={p.id} p={p} following={followingId === p.id} stale={stale} onFollow={onFollow} />
        ))}
      </div>
      {w.away.length > 0 ? (
        <div data-testid="lv-away" className="lv-away">
          <div className="lv-caption px-3.5 pb-0.5 pt-1.5">
            Away{countAway ? <span data-testid="lv-away-count"> {w.away.length}</span> : null}
          </div>
          {w.away.map((p) => (
            <Row key={p.id} p={p} following={followingId === p.id} stale={stale} onFollow={onFollow} />
          ))}
        </div>
      ) : null}
    </PanelSection>
  )
}
