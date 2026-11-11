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
import { MailIcon, useMailUi } from '../ui'

export function EmailList({
  emails,
  loading,
  loadingMore,
  hasMore,
  error,
  selectedId,
  onSelect,
  onLoadMore,
  onRetry,
}: {
  emails: ReaderEmail[]
  loading: boolean
  loadingMore: boolean
  hasMore: boolean
  error: string | null
  selectedId: string | null
  onSelect: (thread: ThreadGroup) => void
  onLoadMore: () => void
  /** The list's read failed: ask again. */
  onRetry?: () => void
}): React.JSX.Element {
  const ui = useMailUi()
  const threads = useMemo(() => sortThreadGroups(groupEmailsByThread(emails)), [emails])
  return (
    <div data-mr="email-list" className="flex flex-col min-h-0 h-full">
      <div className="px-4 py-1.5 border-b border-[var(--color-border)] type-hint text-[var(--color-text-muted)]" aria-live="polite">
        {error
          ? '—'
          : loading
          ? t('loading')
          : hasMore
            ? t('conversations_count_plus', { count: threads.length })
            : t('conversations_count_simple', { count: threads.length })}
      </div>
      <div className="flex-1 min-h-0 overflow-y-auto">
        {error && (
          <div role="alert" className="px-4 py-6 flex flex-col items-center text-center gap-1">
            <p className="type-row text-[var(--color-status-error-text)]">{t('list_error_title')}</p>
            <p data-testid="mr-list-error" className="type-hint text-[var(--color-text-muted)] max-w-[40ch]">
              {error}
            </p>
            {onRetry && (
              <ui.Button variant="default" className="mt-2" onClick={onRetry}>
                {t('try_again')}
              </ui.Button>
            )}
          </div>
        )}
        {!error && loading && threads.length === 0 && (
          <div data-mr="list-skeleton" aria-hidden="true" className="flex flex-col">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="pl-6 pr-4 py-3 border-b border-[var(--color-border)] flex flex-col gap-2">
                <span className="h-2.5 w-2/5 rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)]" />
                <span className="h-2.5 w-4/5 rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)]" />
              </div>
            ))}
          </div>
        )}
        {!error && !loading && threads.length === 0 && (
          <div data-mr="list-empty" className="px-4 py-10 flex flex-col items-center text-center gap-1">
            <MailIcon name="inbox" size={22} className="mb-1 text-[var(--color-text-muted)]" />
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
              <ui.Button variant="default" data-testid="mr-load-more" disabled={loadingMore} onClick={onLoadMore}>
                {loadingMore ? t('loading_more') : t('load_more')}
              </ui.Button>
            ) : (
              <span className="type-hint text-[var(--color-text-muted)]">{t('no_more_emails')}</span>
            )}
          </div>
        )}
      </div>
    </div>
  )
}
