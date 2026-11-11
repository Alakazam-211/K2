// One message: header, SPF/DKIM/DMARC badges, the body and its attachments,
// and the message's action bar. Adapted from the read parts of OCmail's
// components/email/email-viewer.tsx (MIT, Copyright (c) 2025 Matthieu
// MALVACHE, Copyright (c) 2026 Alakazam Labs; see ../../NOTICE).
//
// The viewer never renders HTML itself and never calls the source: the body
// goes through the host's `renderHtml` (K2: HtmlFrame, profile "inert"), and
// every action is a callback the reader wires to an explicit button. Showing a
// message changes nothing in the mailbox.
//
// The body, by kind:
//   HTML: on a light "paper" card in every look. Mail HTML is written for a
//     white page (dark text, light tables, logos with white edges), so
//     recolouring it breaks more than it fixes. The card has the look's card
//     radius (cut corners in Terminal), a hairline edge and a soft shadow, and
//     fills the pane's height (the frame can't measure its page: it runs no
//     script and has no same-origin), so it reads as a document on the desk.
//   Plain text: in the look's own colours and font, at a reading measure.

import { useMemo } from 'react'
import { displayAddress, formatDate, formatFullDate, formatRecipients, formatSize } from '../format'
import { t } from '../i18n'
import { mailFrameDocument, sanitizeEmailHtml } from '../sanitize'
import type { ReaderAttachment, ReaderMessage } from '../types'
import { IconButton, MailIcon, useMailUi, type MailTagTone } from '../ui'

/** The host's frame for mail HTML (already cleaned twice, a full document). */
export type RenderHtml = (props: { html: string; title: string }) => React.ReactNode

export interface MessageActions {
  onReply?: (m: ReaderMessage) => void
  /** Reply to the sender and everyone else on it. */
  onReplyAll?: (m: ReaderMessage) => void
  onToggleRead?: (m: ReaderMessage) => void
  onToggleStar?: (m: ReaderMessage) => void
  onArchive?: (m: ReaderMessage) => void
  onDelete?: (m: ReaderMessage) => void
  onMove?: (m: ReaderMessage, folder: string) => void
  onDownload?: (m: ReaderMessage, a: ReaderAttachment) => void
  /** Folders the Move menu offers (role or name, label). */
  moveTargets?: { value: string; label: string }[]
  starred?: boolean
  busy?: boolean
}

type AuthCheck = 'SPF' | 'DKIM' | 'DMARC'

/** What a verdict means: passed, failed, or neither (softfail, none,
 *  neutral, temperror, policy). */
export function authTone(result: string): Extract<MailTagTone, 'ok' | 'warn' | 'danger'> {
  const r = result.trim().toLowerCase()
  if (r === 'pass') return 'ok'
  if (r === 'fail' || r === 'permerror' || r === 'hardfail') return 'danger'
  return 'warn'
}

export function authTitle(check: AuthCheck, result: string): string {
  const what = t(check === 'SPF' ? 'auth_spf' : check === 'DKIM' ? 'auth_dkim' : 'auth_dmarc')
  const tone = authTone(result)
  const means = t(tone === 'ok' ? 'auth_ok' : tone === 'danger' ? 'auth_fail' : 'auth_warn', { result })
  return `${what}\n${means}`
}

function AuthBadge({ label, result }: { label: AuthCheck; result: string | null | undefined }): React.JSX.Element | null {
  const ui = useMailUi()
  if (!result) return null
  const tone = authTone(result)
  return (
    <ui.Tag
      tone={tone}
      data-mr="auth-badge"
      data-case="as-written"
      data-testid={`mr-auth-${label.toLowerCase()}`}
      title={authTitle(label, result)}
      {...ui.shape('chip')}
    >
      {label} {result.toLowerCase()}
    </ui.Tag>
  )
}

export function MessageActionBar({ message, actions }: { message: ReaderMessage; actions: MessageActions }): React.JSX.Element {
  const a = actions
  const ui = useMailUi()
  const others = (message.to?.length ?? 0) + (message.cc?.length ?? 0) > 1
  return (
    <div data-mr="action-bar" role="toolbar" aria-label={t('message_actions')} className="flex flex-wrap items-center gap-1.5">
      {a.onReply && (
        <IconButton icon="reply" label={t('reply')} showLabel variant="default" data-mr="action" data-testid="mr-act-reply" disabled={a.busy} onClick={() => a.onReply!(message)} />
      )}
      {a.onReplyAll && others && (
        <IconButton icon="reply-all" label={t('reply_all')} showLabel variant="default" data-mr="action" data-testid="mr-act-reply-all" disabled={a.busy} onClick={() => a.onReplyAll!(message)} />
      )}
      <span aria-hidden="true" className="w-px h-4 mx-1 bg-[var(--color-border)]" />
      {a.onArchive && <IconButton icon="archive" label={t('archive')} data-mr="action" data-testid="mr-act-archive" disabled={a.busy} onClick={() => a.onArchive!(message)} />}
      {a.onDelete && (
        <IconButton icon="delete" label={t('delete')} variant="danger" data-mr="action" data-testid="mr-act-delete" disabled={a.busy} onClick={() => a.onDelete!(message)} />
      )}
      {a.onToggleStar && (
        <IconButton
          icon={a.starred ? 'starred' : 'star'}
          label={a.starred ? t('unstar') : t('star')}
          pressed={!!a.starred}
          data-mr="action"
          data-testid="mr-act-star"
          className={a.starred ? 'text-[var(--color-status-warn-text)]' : ''}
          disabled={a.busy}
          onClick={() => a.onToggleStar!(message)}
        />
      )}
      {a.onToggleRead && (
        <IconButton
          icon={message.unread ? 'mark-read' : 'mark-unread'}
          label={message.unread ? t('mark_read') : t('mark_unread')}
          data-mr="action"
          data-testid="mr-act-read"
          disabled={a.busy}
          onClick={() => a.onToggleRead!(message)}
        />
      )}
      {a.onMove && a.moveTargets && a.moveTargets.length > 0 && (
        // The shared button's look, with the native folder list over it (so
        // the keyboard and the system menu work as a select does).
        <span data-mr="move" className="relative inline-flex rounded-[var(--radius-field)] focus-within:outline focus-within:outline-2 focus-within:outline-offset-1 focus-within:outline-[var(--color-accent)]" title={t('move_to')}>
          <ui.Button variant="quiet" tabIndex={-1} aria-hidden="true" disabled={a.busy} className="inline-flex items-center gap-1.5 pointer-events-none">
            <MailIcon name="move" size={14} />
            <span>{t('move_to')}</span>
          </ui.Button>
          <select
            data-mr="action"
            data-testid="mr-act-move"
            aria-label={t('move_to')}
            disabled={a.busy}
            value=""
            onChange={(e) => {
              if (e.target.value) a.onMove!(message, e.target.value)
            }}
            className="absolute inset-0 w-full h-full opacity-0 cursor-pointer disabled:cursor-not-allowed"
          >
            <option value="">{t('move_to')}</option>
            {a.moveTargets.map((f) => (
              <option key={f.value} value={f.value}>
                {f.label}
              </option>
            ))}
          </select>
        </span>
      )}
    </div>
  )
}

/** The message body: the host's frame for HTML on a paper card, plain text
 *  in the look's colours otherwise. */
export function MessageBody({ message, renderHtml }: { message: ReaderMessage; renderHtml: RenderHtml }): React.JSX.Element {
  const ui = useMailUi()
  const doc = useMemo(
    () => (message.html && message.html.trim() ? mailFrameDocument(sanitizeEmailHtml(message.html)) : null),
    [message.html],
  )
  if (doc) {
    return (
      <div
        data-mr="html-body"
        data-testid="mr-html-body"
        {...ui.shape('card')}
        // The paper: light in every look (color-scheme: light makes Canvas
        // white), the card radius, a hairline edge and the soft card shadow.
        // Its height is the pane's view (100cqh of the message pane), at
        // least 320px, so it fills the reader and scrolls with the thread.
        className="relative overflow-hidden border border-[var(--color-border)] rounded-[var(--radius-box)] bg-[Canvas] [color-scheme:light] [box-shadow:var(--shadow-2)] h-[max(320px,calc(100cqh-2rem))]"
      >
        <div className="absolute inset-0">{renderHtml({ html: doc, title: message.subject || t('no_subject') })}</div>
      </div>
    )
  }
  return (
    <div
      data-mr="text-body"
      data-testid="mr-text-body"
      className="type-row leading-relaxed whitespace-pre-wrap break-words max-w-[72ch] text-[var(--color-text-primary)]"
    >
      {message.text ?? ''}
    </div>
  )
}

/** The first letter of a sender's name, for the round mark beside it. */
function initial(m: ReaderMessage): string {
  const n = displayAddress(m.from).trim()
  return (n.match(/[\p{L}\p{N}]/u)?.[0] ?? '?').toUpperCase()
}

export function Attachments({ message, actions }: { message: ReaderMessage; actions: MessageActions }): React.JSX.Element | null {
  const ui = useMailUi()
  if (message.attachments.length === 0) return null
  return (
    <section data-mr="attachments" aria-label={t('attachments')} className="flex flex-col gap-2">
      <h3 className="flex items-center gap-1.5 type-section uppercase tracking-wider text-[var(--color-text-muted)]">
        <MailIcon name="attach" size={12} />
        {t('attachments')} · {message.attachments.length}
      </h3>
      <ul className="flex flex-wrap gap-2">
        {message.attachments.map((att) => (
          <li key={att.index}>
            <ui.Card raised data-mr="attachment" {...ui.shape('card')} className="flex items-center gap-2.5 pl-3 pr-1.5 py-1.5 max-w-[22rem]">
              <MailIcon name="file" size={16} className="flex-shrink-0 text-[var(--color-text-muted)]" />
              <span className="min-w-0 flex flex-col">
                <span className="truncate type-row text-[var(--color-text-primary)]" title={att.filename || `#${att.index}`}>
                  {att.filename || `#${att.index}`}
                </span>
                <span className="type-hint text-[var(--color-text-muted)] truncate">{formatSize(att.size)}</span>
              </span>
              {actions.onDownload && (
                <IconButton
                  icon="download"
                  label={`${t('download')} ${att.filename || ''}`.trim()}
                  data-testid={`mr-download-${att.index}`}
                  disabled={actions.busy}
                  className="flex-shrink-0"
                  onClick={() => actions.onDownload!(message, att)}
                />
              )}
            </ui.Card>
          </li>
        ))}
      </ul>
    </section>
  )
}

export function EmailViewer({
  message,
  renderHtml,
  actions,
  showSubject = true,
}: {
  message: ReaderMessage
  renderHtml: RenderHtml
  actions: MessageActions
  /** Off inside a conversation, where the subject heads the whole thread. */
  showSubject?: boolean
}): React.JSX.Element {
  const ui = useMailUi()
  const auth = message.authResults
  return (
    <article data-mr="email-viewer" data-testid="mr-viewer" className="flex flex-col gap-3 px-5 py-4">
      {showSubject && <h2 className="type-title text-[var(--color-text-primary)] break-words">{message.subject || t('no_subject')}</h2>}
      <header className="flex items-start gap-3">
        <span
          aria-hidden="true"
          {...ui.shape('chip')}
          className="flex-shrink-0 w-8 h-8 flex items-center justify-center rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] type-name text-[var(--color-text-secondary)]"
        >
          {initial(message)}
        </span>
        <div className="flex-1 min-w-0 flex flex-col gap-0.5">
          <div className="flex items-baseline gap-2 min-w-0">
            <span className="truncate type-row">
              <span className="font-semibold text-[var(--color-text-primary)]">{displayAddress(message.from)}</span>
              {message.from?.address && message.from?.name ? <span className="text-[var(--color-text-muted)]"> &lt;{message.from.address}&gt;</span> : null}
            </span>
            <time className="ml-auto flex-shrink-0 type-hint tabular-nums text-[var(--color-text-muted)]" dateTime={message.date} title={formatFullDate(message.date)}>
              {formatDate(message.date)}
            </time>
          </div>
          {message.to && message.to.length > 0 && (
            <div className="type-hint text-[var(--color-text-muted)] truncate">
              {t('to')}: {formatRecipients(message.to)}
              {message.cc && message.cc.length > 0 ? ` · ${t('cc')}: ${formatRecipients(message.cc)}` : ''}
            </div>
          )}
          {auth && (auth.spf || auth.dkim || auth.dmarc) && (
            <div data-mr="auth" className="flex flex-wrap items-center gap-1.5 pt-1">
              <AuthBadge label="SPF" result={auth.spf} />
              <AuthBadge label="DKIM" result={auth.dkim} />
              <AuthBadge label="DMARC" result={auth.dmarc} />
            </div>
          )}
        </div>
      </header>
      <MessageActionBar message={message} actions={actions} />
      <p data-mr="untrusted" className="type-hint text-[var(--color-text-muted)]">
        {t('untrusted')}
        {message.html ? ` ${t('remote_blocked')} ${t('links_inert')}` : ''}
      </p>
      <MessageBody message={message} renderHtml={renderHtml} />
      <Attachments message={message} actions={actions} />
    </article>
  )
}
