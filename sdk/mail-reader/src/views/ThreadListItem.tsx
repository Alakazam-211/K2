// One conversation in the list. Adapted from OCmail's
// components/email/thread-list-item.tsx, email-list-item.tsx and
// thread-email-item.tsx (MIT, Copyright (c) 2025 Matthieu MALVACHE,
// Copyright (c) 2026 Alakazam Labs; see ../../NOTICE): unread dot,
// sender/participants, count, date, subject, attachment mark. No checkbox,
// drag, long-press, context menu, colour tags or avatars.

import { formatDate } from '../format'
import { t } from '../i18n'
import type { ThreadGroup } from '../types'
import { MailIcon, useMailUi } from '../ui'

export function ThreadListItem({
  thread,
  selected,
  onSelect,
}: {
  thread: ThreadGroup
  selected: boolean
  onSelect: (thread: ThreadGroup) => void
}): React.JSX.Element {
  const ui = useMailUi()
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
      className={`relative w-full text-left pl-6 pr-4 py-2.5 border-b border-[var(--color-border)] transition-colors focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent)] ${
        selected ? 'bg-[var(--color-bg-elevated)]' : 'hover:bg-[var(--color-bg-hover)]'
      }`}
    >
      {selected && <span aria-hidden="true" data-mr="selected-bar" className="absolute left-0 top-0 bottom-0 w-[3px] bg-[var(--color-accent)]" />}
      {unread && (
        <span
          aria-label="unread"
          data-mr="unread-dot"
          className="absolute left-2.5 top-[1.05rem] w-2 h-2 rounded-[var(--radius-badge)] bg-[var(--color-accent)]"
        />
      )}
      <div className="flex items-baseline justify-between gap-2">
        <span
          className={`truncate type-row ${unread ? 'font-semibold text-[var(--color-text-primary)]' : 'text-[var(--color-text-primary)]'}`}
        >
          {who}
        </span>
        <span className="flex-shrink-0 flex items-center gap-1.5 type-hint tabular-nums text-[var(--color-text-muted)]">
          {thread.hasAttachment && (
            <span data-mr="attachment-mark" aria-label="has attachment" title="Has attachments" className="inline-flex">
              <MailIcon name="attach" size={12} />
              {!ui.Icon && '+'}
            </span>
          )}
          {formatDate(latest.date)}
        </span>
      </div>
      <div className="flex items-center gap-2 mt-0.5">
        <span className={`flex-1 min-w-0 truncate type-row ${unread ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-secondary)]'}`}>
          {latest.subject || t('no_subject')}
        </span>
        {thread.emailCount > 1 && (
          <span
            data-mr="thread-count"
            title={t('messages_in_thread', { count: thread.emailCount })}
            className="flex-shrink-0 min-w-[18px] px-1 text-center type-hint tabular-nums rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)] text-[var(--color-text-secondary)] border border-[var(--color-border)]"
          >
            {thread.emailCount}
          </span>
        )}
      </div>
    </button>
  )
}
