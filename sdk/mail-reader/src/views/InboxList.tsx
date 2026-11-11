// The inbox picker. Not in OCmail (it shows one account); K2's admin reader
// shows many inboxes, grouped by who they belong to. A viewer who may open
// every inbox (Owner, Admin) sees "Your mailboxes" and then "All inboxes"
// grouped by owner; anyone else sees only their own mailboxes and never
// another group (the daemon already sends only those; this is the belt).

import { inboxOwnerLabel, providerLabel } from '../format'
import { t } from '../i18n'
import type { ReaderInbox, ReaderViewer } from '../types'
import { useMailUi } from '../ui'
import { NAV_ROW, navRowTone } from './FolderList'

export interface InboxOwnerGroup {
  owner: string
  inboxes: ReaderInbox[]
}

export interface InboxGroups {
  own: ReaderInbox[]
  /** null when the viewer may not see other people's or agents' inboxes. */
  all: InboxOwnerGroup[] | null
}

export function groupInboxes(viewer: ReaderViewer | null, inboxes: ReaderInbox[]): InboxGroups {
  const own = inboxes.filter((i) => i.own)
  if (!viewer?.opensAny) return { own, all: null }
  const byOwner = new Map<string, ReaderInbox[]>()
  for (const inbox of inboxes) {
    if (inbox.own) continue
    const key = inboxOwnerLabel(inbox)
    const list = byOwner.get(key)
    if (list) list.push(inbox)
    else byOwner.set(key, [inbox])
  }
  const all = [...byOwner.entries()]
    .sort(([a], [b]) => a.localeCompare(b))
    .map(([owner, list]) => ({ owner, inboxes: [...list].sort((a, b) => a.address.localeCompare(b.address)) }))
  return { own, all }
}

function InboxRow({
  inbox,
  selected,
  onSelect,
}: {
  inbox: ReaderInbox
  selected: boolean
  onSelect: (address: string) => void
}): React.JSX.Element {
  const ui = useMailUi()
  return (
    <button
      type="button"
      data-mr="inbox-row"
      data-testid={`mr-inbox-${inbox.address}`}
      aria-current={selected ? 'true' : undefined}
      title={inbox.address}
      onClick={() => onSelect(inbox.address)}
      {...ui.shape('control')}
      className={`${NAV_ROW} ${navRowTone(selected)} px-2.5 py-1.5 flex flex-col gap-0.5`}
    >
      <span className="type-control truncate text-[var(--color-text-primary)]">{inbox.displayName || inbox.address}</span>
      <span className="type-hint truncate text-[var(--color-text-muted)]">
        {inbox.displayName ? `${inbox.address} · ` : ''}
        {providerLabel(inbox.provider)} · {inbox.kind === 'person' ? t('kind_person') : t('kind_agent')}
      </span>
    </button>
  )
}

export function InboxList({
  viewer,
  inboxes,
  selected,
  onSelect,
}: {
  viewer: ReaderViewer | null
  inboxes: ReaderInbox[]
  selected: string | null
  onSelect: (address: string) => void
}): React.JSX.Element {
  const groups = groupInboxes(viewer, inboxes)
  const nothing = groups.own.length === 0 && (groups.all === null || groups.all.length === 0)
  return (
    <nav data-mr="inbox-list" aria-label={t('inboxes')} className="flex flex-col gap-px py-2">
      {nothing && (
        <p className="px-4 type-hint text-[var(--color-text-muted)]">
          {viewer?.opensAny ? t('no_inboxes') : t('no_own_inboxes')}
        </p>
      )}
      {groups.own.length > 0 && (
        <section data-mr="inbox-group" data-testid="mr-group-own">
          <h3 className="px-4 pt-1 pb-1.5 type-section uppercase tracking-wider text-[var(--color-text-muted)]">{t('your_mailboxes')}</h3>
          {groups.own.map((i) => (
            <InboxRow key={i.address} inbox={i} selected={selected === i.address} onSelect={onSelect} />
          ))}
        </section>
      )}
      {groups.all && groups.all.length > 0 && (
        <section data-mr="inbox-group" data-testid="mr-group-all">
          <h3 className="px-4 pt-4 pb-1.5 type-section uppercase tracking-wider text-[var(--color-text-muted)]">{t('all_inboxes')}</h3>
          {groups.all.map((g) => (
            <div key={g.owner} data-mr="inbox-owner" data-testid={`mr-owner-${g.owner}`}>
              <div className="px-4 pt-2 pb-0.5 type-hint text-[var(--color-text-muted)] truncate">{g.owner}</div>
              {g.inboxes.map((i) => (
                <InboxRow key={i.address} inbox={i} selected={selected === i.address} onSelect={onSelect} />
              ))}
            </div>
          ))}
        </section>
      )}
    </nav>
  )
}
