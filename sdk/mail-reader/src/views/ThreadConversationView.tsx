// A conversation as a column of cards, oldest first, the newest open.
// Adapted from OCmail's components/email/thread-conversation-view.tsx (MIT,
// Copyright (c) 2025 Matthieu MALVACHE, Copyright (c) 2026 Alakazam Labs;
// see ../../NOTICE). OCmail marks a message read when its card expands; here
// expanding a card is a view change only. The actions apply to the card they
// sit on.

import { useEffect, useState } from 'react'
import { displayAddress, formatDate } from '../format'
import { t } from '../i18n'
import type { ReaderMessage } from '../types'
import { EmailViewer, type MessageActions, type RenderHtml } from './EmailViewer'

export function ThreadConversationView({
  messages,
  renderHtml,
  actionsFor,
}: {
  /** Oldest first. */
  messages: ReaderMessage[]
  renderHtml: RenderHtml
  actionsFor: (m: ReaderMessage) => MessageActions
}): React.JSX.Element {
  const newest = messages.length > 0 ? messages[messages.length - 1].id : null
  const [open, setOpen] = useState<Set<string>>(() => new Set(newest ? [newest] : []))
  useEffect(() => {
    setOpen(new Set(newest ? [newest] : []))
  }, [newest])

  if (messages.length === 1) {
    return <EmailViewer message={messages[0]} renderHtml={renderHtml} actions={actionsFor(messages[0])} />
  }
  return (
    <div data-mr="thread-view" data-testid="mr-thread-view" className="flex flex-col">
      <div className="px-4 pt-3 type-hint text-[var(--color-text-muted)]">{t('messages_in_thread', { count: messages.length })}</div>
      {messages.map((m) =>
        open.has(m.id) ? (
          <div key={m.id} data-mr="thread-card" data-open="true" className="border-b border-[var(--color-border)]">
            <EmailViewer message={m} renderHtml={renderHtml} actions={actionsFor(m)} />
          </div>
        ) : (
          <button
            key={m.id}
            type="button"
            data-mr="thread-card"
            data-open="false"
            data-testid={`mr-card-${m.id}`}
            onClick={() => setOpen((s) => new Set(s).add(m.id))}
            className="w-full text-left px-4 py-2 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] flex items-center justify-between gap-2"
          >
            <span className="truncate type-row text-[var(--color-text-secondary)]">{displayAddress(m.from)}</span>
            <span className="type-hint text-[var(--color-text-muted)] flex-shrink-0">{formatDate(m.date)}</span>
          </button>
        ),
      )}
    </div>
  )
}
