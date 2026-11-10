// One conversation in the list. Adapted from OCmail's
// components/email/thread-list-item.tsx, email-list-item.tsx and
// thread-email-item.tsx (MIT, Copyright (c) 2025 Matthieu MALVACHE,
// Copyright (c) 2026 Alakazam Labs; see ../../NOTICE): unread dot,
// sender/participants, count, date, subject, attachment mark. No checkbox,
// drag, long-press, context menu, colour tags or avatars.

import { formatDate } from '../format'
import { t } from '../i18n'
import type { ThreadGroup } from '../types'

export function ThreadListItem({
  thread,
  selected,
  onSelect,
}: {
  thread: ThreadGroup
  selected: boolean
  onSelect: (thread: ThreadGroup) => void
}): React.JSX.Element {
  const latest = thread.latestEmail
  const unread = thread.hasUnread
  const who =
    thread.participantNames.length > 0
      ? thread.participantNames.join(', ')
      : latest.from?.name || latest.from?.address || t('unknown_sender')
  return (
    <button
      type="button"
      data-mr="thread-row"
      data-testid={`mr-thread-${latest.id}`}
      data-unread={unread ? 'true' : 'false'}
      aria-current={selected ? 'true' : undefined}
      onClick={() => onSelect(thread)}
      className={`relative w-full text-left px-4 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] ${
        selected ? 'bg-[var(--color-accent-soft)]' : ''
      }`}
    >
      {unread && (
        <span
          aria-label="unread"
          data-mr="unread-dot"
          className="absolute left-1.5 top-1/2 -translate-y-1/2 w-1.5 h-1.5 bg-[var(--color-accent)]"
        />
      )}
      <div className="flex items-center justify-between gap-2">
        <span
          className={`truncate type-row ${unread ? 'font-semibold text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}
        >
          {who}
          {thread.emailCount > 1 && <span className="ml-1 type-hint text-[var(--color-text-muted)]">{thread.emailCount}</span>}
        </span>
        <span className="flex-shrink-0 type-hint tabular-nums text-[var(--color-text-muted)]">
          {thread.hasAttachment && <span data-mr="attachment-mark" className="mr-1" aria-label="has attachment">⎘</span>}
          {formatDate(latest.date)}
        </span>
      </div>
      <div className={`truncate type-row ${unread ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}>
        {latest.subject || t('no_subject')}
      </div>
    </button>
  )
}
