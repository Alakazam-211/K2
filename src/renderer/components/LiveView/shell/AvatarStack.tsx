// The header's avatar stack: everyone else on the file, overlapping; away
// people faded; a `+N` at the end whose tooltip names the rest. Clicking
// anyone follows them.

import { avatarStack, type LivePerson } from '@/lib/live-view/shared/people'
import { LiveAvatar } from './LiveAvatar'

/** The ring round an avatar in the stack: its colour (hover, whom you follow) is an edge with a band of
 *  the background inside it, so a light presence fill never runs into it. Two nested shapes, so
 *  Terminal's cut corners keep the edge all the way round. */
function Ring({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <span data-k2-chamfer="surface" data-k2-avatar="" className="lv-ring inline-flex p-[1.25px]" style={{ ['--k2-ch-cut' as string]: '7px', ['--k2-ch-bw' as string]: '0px' }}>
      <span data-k2-chamfer="surface" data-k2-avatar="" className="lv-ring-gap inline-flex p-[0.75px]" style={{ ['--k2-ch-cut' as string]: '6.5px', ['--k2-ch-bw' as string]: '0px' }}>
        {children}
      </span>
    </span>
  )
}

export function AvatarStack({
  people,
  max = 4,
  followingId = null,
  grey = false,
  editor = false,
  onFollow,
}: {
  /** Everyone on the file (you are left out here). */
  people: readonly LivePerson[]
  max?: number
  followingId?: string | null
  /** Not connected: greyed, and not followable. */
  grey?: boolean
  /** The editors' stack: you first, then who's here (no away people). */
  editor?: boolean
  /** None: the avatars only show who's here (Follow comes with E4b). */
  onFollow?: (id: string) => void
}): React.JSX.Element | null {
  const { shown, more } = avatarStack(people, max, editor)
  if (shown.length === 0) return null
  return (
    <div role="group" aria-label={onFollow ? 'Who’s here: click someone to follow them' : 'Who’s here'} className="lv-stack flex flex-none items-center" data-testid="lv-stack">
      {shown.map((p) =>
        p.self || !onFollow ? (
          <span key={p.id} data-lv-stack-item="" title={p.self ? 'You' : `${p.name}${p.kind === 'agent' ? ' (agent)' : ''}`} className="inline-flex rounded-[var(--radius-pill)]">
            <Ring>
              <LiveAvatar person={p} size={22} grey={grey} />
            </Ring>
          </span>
        ) : (
        <button
          key={p.id}
          type="button"
          data-k2-focus=""
          data-lv-stack-item=""
          disabled={grey}
          aria-label={`Follow ${p.name}`}
          aria-pressed={followingId === p.id}
          title={`${p.name}${p.kind === 'agent' ? ' (agent)' : ''}${p.away ? ', away' : ''}: follow ${p.kind === 'agent' ? 'it' : 'them'}`}
          onClick={() => onFollow(p.id)}
          className="no-drag inline-flex cursor-pointer rounded-[var(--radius-pill)] p-0 disabled:cursor-default"
        >
          <Ring>
            <LiveAvatar person={p} size={22} grey={grey} />
          </Ring>
        </button>
        ),
      )}
      {more.length > 0 ? (
        <span
          data-testid="lv-stack-more"
          data-lv-stack-item=""
          data-k2-chamfer="chip"
          style={{ marginLeft: '-2px' }}
          title={`Also here: ${more.map((p) => p.name).join(', ')}`}
          className="type-hint inline-flex h-[22px] min-w-[22px] items-center justify-center rounded-[var(--radius-pill)] border border-[var(--color-control-edge)] bg-[var(--color-bg-elevated)] px-1 font-medium text-[var(--color-text-secondary)]"
        >
          +{more.length}
        </span>
      ) : null}
    </div>
  )
}
