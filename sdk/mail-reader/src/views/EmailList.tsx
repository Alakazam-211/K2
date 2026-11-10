// The message list, grouped by conversation. Adapted from OCmail's
// components/email/email-list.tsx (MIT, Copyright (c) 2025 Matthieu
// MALVACHE, Copyright (c) 2026 Alakazam Labs; see ../../NOTICE). OCmail
// virtualises with @tanstack/react-virtual and loads more on scroll; here a
// page is at most 100 messages and "Load more" asks for the next one. No
// selection, batch actions, sort menu or context menu.

import { useMemo } from 'react'
import { t } from '../i18n'
import { groupEmailsByThread, sortThreadGroups } from '../thread-utils'
import type { ReaderEmail, ThreadGroup } from '../types'
import { ThreadListItem } from './ThreadListItem'

export function EmailList({
  emails,
  loading,
  loadingMore,
  hasMore,
  error,
  selectedId,
  onSelect,
  onLoadMore,
}: {
  emails: ReaderEmail[]
  loading: boolean
  loadingMore: boolean
  hasMore: boolean
  error: string | null
  selectedId: string | null
  onSelect: (thread: ThreadGroup) => void
  onLoadMore: () => void
}): React.JSX.Element {
  const threads = useMemo(() => sortThreadGroups(groupEmailsByThread(emails)), [emails])
  return (
    <div data-mr="email-list" className="flex flex-col min-h-0 h-full">
      <div className="px-4 py-2 border-b border-[var(--color-border)] type-hint text-[var(--color-text-muted)]">
        {loading
          ? t('loading')
          : hasMore
            ? t('conversations_count_plus', { count: threads.length })
            : t('conversations_count_simple', { count: threads.length })}
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto">
        {error && (
          <p data-testid="mr-list-error" className="px-4 py-3 type-row text-[var(--color-status-error-text)]">
            {error}
          </p>
        )}
        {!error && !loading && threads.length === 0 && (
          <div className="px-4 py-8 text-center">
            <p className="type-row text-[var(--color-text-primary)]">{t('no_emails')}</p>
            <p className="type-hint text-[var(--color-text-muted)]">{t('no_emails_description')}</p>
          </div>
        )}
        {threads.map((thread) => (
          <ThreadListItem
            key={`${thread.latestEmail.inbox}:${thread.threadId}`}
            thread={thread}
            selected={!!selectedId && thread.emails.some((e) => e.id === selectedId)}
            onSelect={onSelect}
          />
        ))}
        {threads.length > 0 && (
          <div className="py-3 flex justify-center">
            {hasMore ? (
              <button
                type="button"
                data-testid="mr-load-more"
                disabled={loadingMore}
                onClick={onLoadMore}
                className="px-3 py-1 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] disabled:opacity-60"
              >
                {loadingMore ? t('loading_more') : t('load_more')}
              </button>
            ) : (
              <span className="type-hint text-[var(--color-text-muted)]">{t('no_more_emails')}</span>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
