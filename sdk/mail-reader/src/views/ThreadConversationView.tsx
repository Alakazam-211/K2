// A conversation as a column of cards, oldest first, the newest open.
// Adapted from OCmail's components/email/thread-conversation-view.tsx (MIT,
// Copyright (c) 2025 Matthieu MALVACHE, Copyright (c) 2026 Alakazam Labs;
// see ../../NOTICE). OCmail marks a message read when its card expands; here
// expanding a card is a view change only. The actions apply to the card they
// sit on. The subject heads the conversation once; `top` is shown above the
// first message (the reader puts an agent's unsent DraftCard there).

import { useEffect, useState } from 'react'
import { displayAddress, formatDate } from '../format'
import { t } from '../i18n'
import type { ReaderMessage } from '../types'
import { MailIcon } from '../ui'
import { EmailViewer, type MessageActions, type RenderHtml } from './EmailViewer'

/** The first line of a message's text, for a closed card. */
function snippet(m: ReaderMessage): string {
  // Text only (shown as text, never as markup): the plain body, else the
  // HTML's words with the tags dropped.
  const source = m.text?.trim() ? m.text : (m.html ?? '').replace(/<(style|script)[\s\S]*?<\/\1>/gi, ' ').replace(/<[^>]+>/g, ' ').replace(/&nbsp;/g, ' ')
  const raw = source.replace(/\s+/g, ' ').trim()
  return raw.length > 140 ? `${raw.slice(0, 140)}…` : raw
}

export function ThreadConversationView({
  messages,
  renderHtml,
  actionsFor,
  top,
}: {
  /** Oldest first. */
  messages: ReaderMessage[]
  renderHtml: RenderHtml
  actionsFor: (m: ReaderMessage) => MessageActions
  /** Above the first message, under the subject (a DraftCard). */
  top?: React.ReactNode
}): React.JSX.Element {
  const newest = messages.length > 0 ? messages[messages.length - 1].id : null
  const [open, setOpen] = useState<Set<string>>(() => new Set(newest ? [newest] : []))
  useEffect(() => {
    setOpen(new Set(newest ? [newest] : []))
  }, [newest])

  const subject = messages[messages.length - 1]?.subject || messages[0]?.subject || t('no_subject')
  const single = messages.length === 1
  return (
    <div data-mr="thread-view" data-testid="mr-thread-view" className="flex flex-col pb-6">
      <header className="px-5 pt-4 pb-1 flex items-baseline gap-3">
        <h2 className="flex-1 min-w-0 type-title text-[var(--color-text-primary)] break-words">{subject}</h2>
        {!single && (
          <span className="flex-shrink-0 type-hint tabular-nums text-[var(--color-text-muted)]">{t('messages_in_thread', { count: messages.length })}</span>
        )}
      </header>
      {top && <div className="px-5 pt-3">{top}</div>}
      {messages.map((m, i) =>
        open.has(m.id) ? (
          <div key={m.id} data-mr="thread-card" data-open="true" className={i < messages.length - 1 ? 'border-b border-[var(--color-border)]' : ''}>
            <EmailViewer message={m} renderHtml={renderHtml} actions={actionsFor(m)} showSubject={false} />
          </div>
        ) : (
          <button
            key={m.id}
            type="button"
            data-mr="thread-card"
            data-open="false"
            data-testid={`mr-card-${m.id}`}
            onClick={() => setOpen((s) => new Set(s).add(m.id))}
            className="w-full text-left px-5 py-2.5 border-b border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] flex items-center gap-3 focus-visible:outline focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-[var(--color-accent)]"
          >
            <span className="flex-shrink-0 w-44 truncate type-row text-[var(--color-text-primary)]">{displayAddress(m.from)}</span>
            <span className="flex-1 min-w-0 truncate type-hint text-[var(--color-text-muted)]">{snippet(m)}</span>
            {m.attachments.length > 0 && <MailIcon name="attach" size={12} className="flex-shrink-0 text-[var(--color-text-muted)]" />}
            <span className="type-hint tabular-nums text-[var(--color-text-muted)] flex-shrink-0">{formatDate(m.date)}</span>
          </button>
        ),
      )}
    </div>
  )
}
