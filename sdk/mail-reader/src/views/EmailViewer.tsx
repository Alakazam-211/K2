// One message: header, SPF/DKIM/DMARC badges, the body and its attachments,
// and the message's action bar. Adapted from the read parts of OCmail's
// components/email/email-viewer.tsx (MIT, Copyright (c) 2025 Matthieu
// MALVACHE, Copyright (c) 2026 Alakazam Labs; see ../../NOTICE).
//
// The viewer never renders HTML itself and never calls the source: the body
// goes through the host's `renderHtml` (K2: HtmlFrame, profile "inert"), and
// every action is a callback the reader wires to an explicit button. Showing a
// message changes nothing in the mailbox.

import { useMemo } from 'react'
import { displayAddress, formatFullDate, formatRecipients, formatSize } from '../format'
import { t } from '../i18n'
import { mailFrameDocument, sanitizeEmailHtml } from '../sanitize'
import type { ReaderAttachment, ReaderMessage } from '../types'

/** The host's frame for mail HTML (already cleaned twice, a full document). */
export type RenderHtml = (props: { html: string; title: string }) => React.ReactNode

export interface MessageActions {
  onReply?: (m: ReaderMessage) => void
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

function AuthBadge({ label, result }: { label: string; result: string | null | undefined }): React.JSX.Element | null {
  if (!result) return null
  const r = result.toLowerCase()
  const tone =
    r === 'pass'
      ? 'var(--color-status-ok)'
      : r === 'fail' || r === 'permerror'
        ? 'var(--color-status-error-text)'
        : 'var(--color-status-warn-text)'
  return (
    <span
      data-mr="auth-badge"
      data-testid={`mr-auth-${label.toLowerCase()}`}
      className="type-hint px-1.5 py-0.5 border"
      style={{ color: tone, borderColor: tone }}
      title={`${label}: ${result}`}
    >
      {label} {result}
    </span>
  )
}

function ActionButton({
  label,
  onClick,
  disabled,
  testId,
  destructive,
}: {
  label: string
  onClick: () => void
  disabled?: boolean
  testId: string
  destructive?: boolean
}): React.JSX.Element {
  return (
    <button
      type="button"
      data-mr="action"
      data-testid={testId}
      disabled={disabled}
      onClick={onClick}
      className={`px-2 py-0.5 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] disabled:opacity-60 ${
        destructive ? 'text-[var(--color-danger)]' : 'text-[var(--color-text-primary)]'
      }`}
    >
      {label}
    </button>
  )
}

export function MessageActionBar({ message, actions }: { message: ReaderMessage; actions: MessageActions }): React.JSX.Element {
  const a = actions
  return (
    <div data-mr="action-bar" className="flex flex-wrap items-center gap-1.5">
      {a.onReply && <ActionButton label={t('reply')} testId="mr-act-reply" disabled={a.busy} onClick={() => a.onReply!(message)} />}
      {a.onToggleRead && (
        <ActionButton
          label={message.unread ? t('mark_read') : t('mark_unread')}
          testId="mr-act-read"
          disabled={a.busy}
          onClick={() => a.onToggleRead!(message)}
        />
      )}
      {a.onToggleStar && (
        <ActionButton label={a.starred ? t('unstar') : t('star')} testId="mr-act-star" disabled={a.busy} onClick={() => a.onToggleStar!(message)} />
      )}
      {a.onMove && a.moveTargets && a.moveTargets.length > 0 && (
        <select
          data-mr="action"
          data-testid="mr-act-move"
          disabled={a.busy}
          value=""
          onChange={(e) => {
            if (e.target.value) a.onMove!(message, e.target.value)
          }}
          className="px-1 py-0.5 type-row border border-[var(--color-border)] bg-[var(--color-bg)] text-[var(--color-text-primary)]"
        >
          <option value="">{t('move_to')}</option>
          {a.moveTargets.map((f) => (
            <option key={f.value} value={f.value}>
              {f.label}
            </option>
          ))}
        </select>
      )}
      {a.onArchive && <ActionButton label={t('archive')} testId="mr-act-archive" disabled={a.busy} onClick={() => a.onArchive!(message)} />}
      {a.onDelete && (
        <ActionButton label={t('delete')} testId="mr-act-delete" destructive disabled={a.busy} onClick={() => a.onDelete!(message)} />
      )}
    </div>
  )
}

/** The message body: the host's frame for HTML, plain text otherwise. */
export function MessageBody({ message, renderHtml }: { message: ReaderMessage; renderHtml: RenderHtml }): React.JSX.Element {
  const doc = useMemo(
    () => (message.html && message.html.trim() ? mailFrameDocument(sanitizeEmailHtml(message.html)) : null),
    [message.html],
  )
  if (doc) {
    return (
      <div data-mr="html-body" data-testid="mr-html-body" className="min-h-[240px] h-[60vh] border border-[var(--color-border)]">
        {renderHtml({ html: doc, title: message.subject || t('no_subject') })}
      </div>
    )
  }
  return (
    <pre data-mr="text-body" data-testid="mr-text-body" className="type-row whitespace-pre-wrap break-words font-sans text-[var(--color-text-primary)]">
      {message.text ?? ''}
    </pre>
  )
}

export function EmailViewer({
  message,
  renderHtml,
  actions,
}: {
  message: ReaderMessage
  renderHtml: RenderHtml
  actions: MessageActions
}): React.JSX.Element {
  const auth = message.authResults
  return (
    <article data-mr="email-viewer" data-testid="mr-viewer" className="flex flex-col gap-3 p-4">
      <header className="flex flex-col gap-1">
        <h2 className="type-title text-[var(--color-text-primary)]">{message.subject || t('no_subject')}</h2>
        <div className="type-row text-[var(--color-text-secondary)]">
          <span className="text-[var(--color-text-primary)]">{displayAddress(message.from)}</span>
          {message.from?.address && message.from?.name ? <span className="text-[var(--color-text-muted)]"> &lt;{message.from.address}&gt;</span> : null}
        </div>
        {message.to && message.to.length > 0 && (
          <div className="type-hint text-[var(--color-text-muted)]">
            {t('to')}: {formatRecipients(message.to)}
            {message.cc && message.cc.length > 0 ? ` · ${t('cc')}: ${formatRecipients(message.cc)}` : ''}
          </div>
        )}
        <div className="type-hint text-[var(--color-text-muted)]">{formatFullDate(message.date)}</div>
        {auth && (auth.spf || auth.dkim || auth.dmarc) && (
          <div className="flex flex-wrap gap-1.5 pt-1">
            <AuthBadge label="SPF" result={auth.spf} />
            <AuthBadge label="DKIM" result={auth.dkim} />
            <AuthBadge label="DMARC" result={auth.dmarc} />
          </div>
        )}
      </header>
      <MessageActionBar message={message} actions={actions} />
      <div className="type-hint text-[var(--color-text-muted)]">
        {t('untrusted')}
        {message.html ? ` ${t('remote_blocked')} ${t('links_inert')}` : ''}
      </div>
      <MessageBody message={message} renderHtml={renderHtml} />
      {message.attachments.length > 0 && (
        <section data-mr="attachments" className="flex flex-col gap-1">
          <h3 className="type-section uppercase tracking-wider text-[var(--color-text-muted)]">{t('attachments')}</h3>
          {message.attachments.map((att) => (
            <div key={att.index} className="flex items-center gap-2 type-row">
              <span className="truncate text-[var(--color-text-primary)]">{att.filename || `#${att.index}`}</span>
              <span className="type-hint text-[var(--color-text-muted)]">
                {att.mime} · {formatSize(att.size)}
              </span>
              {actions.onDownload && (
                <button
                  type="button"
                  data-testid={`mr-download-${att.index}`}
                  disabled={actions.busy}
                  onClick={() => actions.onDownload!(message, att)}
                  className="ml-auto px-2 py-0.5 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)]"
                >
                  {t('download')}
                </button>
              )}
            </div>
          ))}
        </section>
      )}
    </article>
  )
}
