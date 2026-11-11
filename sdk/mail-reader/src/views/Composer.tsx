// A small composer for a reply or a new message (K2's own; OCmail's composer
// is not ported). Plain text only. Send asks first; the reader decides what
// Send and Save draft call.
//
// A card: its title and close at the top, From, then To / Cc / Subject as
// labelled rows on the host's text field, the body on the multi-line field
// radius, attachment chips, and Send (primary), Save draft and Discard
// (quiet). ⌘↩ (Ctrl+Enter) sends; Esc closes, asking first when something
// has been written. `ComposerBody` is the body field on its own (DraftCard
// edits in it, and its To / Cc / Subject in `FieldRow`s).

import { useEffect, useRef, useState } from 'react'
import { formatSize, splitAddresses } from '../format'
import { t } from '../i18n'
import { IconButton, MailIcon, useMailUi } from '../ui'

export type ComposeMode =
  | { kind: 'new'; inbox: string }
  | { kind: 'reply'; inbox: string; id: string; subject: string; to: string; all?: boolean }

export interface ComposeValues {
  to: string[]
  cc: string[]
  subject: string
  body: string
}

/** A file on the message (shown as a chip). */
export interface ComposeAttachment {
  id: string
  name: string
  size?: number
}

export type ComposeConfirm = (o: { title: string; message: string; confirmLabel: string; destructive?: boolean }) => Promise<boolean>

/** The ⌘↩ / Ctrl+Enter chord. */
export function isSendChord(e: Pick<React.KeyboardEvent, 'key' | 'metaKey' | 'ctrlKey'>): boolean {
  return e.key === 'Enter' && (e.metaKey || e.ctrlKey)
}

/** The body field: the multi-line radius, the field edge, accent on focus. */
export function ComposerBody({
  className = '',
  ...rest
}: React.TextareaHTMLAttributes<HTMLTextAreaElement>): React.JSX.Element {
  return (
    <textarea
      data-mr="compose-body"
      className={`w-full min-h-[180px] resize-y px-3 py-2.5 type-row leading-relaxed rounded-[var(--radius-field-multiline)] border border-[var(--color-control-edge)] bg-[var(--color-bg)] text-[var(--color-text-primary)] placeholder:text-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)] ${className}`}
      {...rest}
    />
  )
}

/** An attachment chip: its icon, name and size, and × to take it off. */
export function AttachmentChip({ file, onRemove }: { file: ComposeAttachment; onRemove?: () => void }): React.JSX.Element {
  const ui = useMailUi()
  return (
    <span
      data-mr="compose-attachment"
      {...ui.shape('chip')}
      className="inline-flex items-center gap-1.5 max-w-[16rem] pl-2 pr-1 py-0.5 border border-[var(--color-border)] rounded-[var(--radius-badge)] bg-[var(--color-bg-elevated)] type-hint text-[var(--color-text-secondary)]"
    >
      <MailIcon name="attach" size={12} className="flex-shrink-0" />
      <span className="truncate text-[var(--color-text-primary)]">{file.name}</span>
      {typeof file.size === 'number' && <span className="flex-shrink-0 text-[var(--color-text-muted)]">{formatSize(file.size)}</span>}
      {onRemove && (
        <button
          type="button"
          aria-label={t('remove_attachment', { name: file.name })}
          title={t('remove_attachment', { name: file.name })}
          onClick={onRemove}
          className="flex-shrink-0 inline-flex p-0.5 rounded-[var(--radius-badge)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)]"
        >
          <MailIcon name="close" size={10} />
          {!ui.Icon && '×'}
        </button>
      )}
    </span>
  )
}

/** A labelled row: To, Cc, Subject (DraftCard's recipients use it too). */
export function FieldRow({ label, htmlFor, children }: { label: string; htmlFor: string; children: React.ReactNode }): React.JSX.Element {
  return (
    <div className="grid grid-cols-[4.5rem_minmax(0,1fr)] items-center gap-2">
      <label htmlFor={htmlFor} className="type-hint text-[var(--color-text-muted)]">
        {label}
      </label>
      {children}
    </div>
  )
}

export function Composer({
  mode,
  busy,
  onSend,
  onSaveDraft,
  onCancel,
  confirm,
  attachments = [],
  onAttach,
  onRemoveAttachment,
}: {
  mode: ComposeMode
  busy: boolean
  onSend: (v: ComposeValues) => void
  onSaveDraft: (v: ComposeValues) => void
  onCancel: () => void
  /** Asks before Esc or Discard throws away what was written (none: closes). */
  confirm?: ComposeConfirm
  /** Files on the message, as chips. */
  attachments?: ComposeAttachment[]
  /** Pick a file. Without it Attach is off and says why (the reader's sends
   *  can't carry files yet). */
  onAttach?: () => void
  onRemoveAttachment?: (id: string) => void
}): React.JSX.Element {
  const ui = useMailUi()
  const reply = mode.kind === 'reply'
  const [to, setTo] = useState(reply ? mode.to : '')
  const [cc, setCc] = useState('')
  const [subject, setSubject] = useState(reply ? (/^re:/i.test(mode.subject) ? mode.subject : `Re: ${mode.subject}`) : '')
  const [body, setBody] = useState('')
  const formRef = useRef<HTMLFormElement>(null)
  const idp = reply ? `mr-c-${mode.id}` : 'mr-c-new'
  const values = (): ComposeValues => ({ to: splitAddresses(to), cc: splitAddresses(cc), subject: subject.trim(), body })
  const canSend = body.trim().length > 0 && (reply || (splitAddresses(to).length > 0 && subject.trim().length > 0))
  const written = body.trim().length > 0 || (!reply && (to.trim().length > 0 || cc.trim().length > 0 || subject.trim().length > 0))

  const send = (): void => {
    if (canSend && !busy) onSend(values())
  }
  const discard = async (): Promise<void> => {
    if (written && confirm) {
      const ok = await confirm({
        title: t('confirm_discard_title'),
        message: t('confirm_discard_message'),
        confirmLabel: t('discard'),
        destructive: true,
      })
      if (!ok) return
    }
    onCancel()
  }

  // A reply starts in the body; a new message in To.
  useEffect(() => {
    const first = formRef.current?.querySelector<HTMLElement>(reply ? '[data-mr="compose-body"]' : `#${idp}-to`)
    first?.focus()
  }, [reply, idp])

  return (
    <ui.Card
      data-mr="composer-card"
      {...ui.shape('card')}
      className="mx-5 my-4"
    >
      <form
        ref={formRef}
        data-mr="composer"
        data-testid="mr-composer"
        className="flex flex-col gap-3 p-4"
        onSubmit={(e) => {
          e.preventDefault()
          send()
        }}
        onKeyDown={(e) => {
          if (isSendChord(e)) {
            e.preventDefault()
            send()
          } else if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            void discard()
          }
        }}
      >
        <div className="flex items-center gap-2">
          <MailIcon name={reply ? (mode.all ? 'reply-all' : 'reply') : 'compose'} size={14} className="text-[var(--color-text-muted)]" />
          <h3 className="flex-1 min-w-0 truncate type-heading text-[var(--color-text-primary)]">
            {reply ? t('reply_title', { to: mode.to }) : t('new_message_title')}
          </h3>
          <IconButton icon="close" label={t('discard')} data-testid="mr-compose-close" onClick={() => void discard()} />
        </div>
        <div className="flex flex-col gap-2">
          <FieldRow label={t('from')} htmlFor={`${idp}-from`}>
            <span id={`${idp}-from`} className="type-row text-[var(--color-text-secondary)] truncate px-2">
              {mode.inbox}
            </span>
          </FieldRow>
          <FieldRow label={t('to')} htmlFor={`${idp}-to`}>
            <ui.TextField
              id={`${idp}-to`}
              data-testid="mr-compose-to"
              className="w-full"
              value={to}
              disabled={reply}
              placeholder={t('to_placeholder')}
              onChange={(e) => setTo(e.target.value)}
            />
          </FieldRow>
          {!reply && (
            <FieldRow label={t('cc')} htmlFor={`${idp}-cc`}>
              <ui.TextField id={`${idp}-cc`} data-testid="mr-compose-cc" className="w-full" value={cc} onChange={(e) => setCc(e.target.value)} />
            </FieldRow>
          )}
          <FieldRow label={t('subject')} htmlFor={`${idp}-subject`}>
            <ui.TextField
              id={`${idp}-subject`}
              data-testid="mr-compose-subject"
              className="w-full"
              value={subject}
              disabled={reply}
              onChange={(e) => setSubject(e.target.value)}
            />
          </FieldRow>
        </div>
        <ComposerBody data-testid="mr-compose-body" aria-label={t('body')} placeholder={t('body')} value={body} onChange={(e) => setBody(e.target.value)} />
        {attachments.length > 0 && (
          <div data-mr="compose-attachments" className="flex flex-wrap gap-1.5">
            {attachments.map((f) => (
              <AttachmentChip key={f.id} file={f} onRemove={onRemoveAttachment ? () => onRemoveAttachment(f.id) : undefined} />
            ))}
          </div>
        )}
        <div className="flex flex-wrap items-center gap-2">
          <ui.Button type="submit" variant="primary" size="md" data-testid="mr-compose-send" disabled={!canSend || busy}>
            {busy ? t('sending') : t('send')}
          </ui.Button>
          <ui.Button
            variant="default"
            size="md"
            data-testid="mr-compose-draft"
            disabled={body.trim().length === 0 || busy}
            onClick={() => onSaveDraft(values())}
          >
            {t('save_draft')}
          </ui.Button>
          <IconButton
            icon="attach"
            label={onAttach ? t('attach') : t('attach_unavailable')}
            data-testid="mr-compose-attach"
            disabled={!onAttach || busy}
            onClick={onAttach}
          />
          <span className="type-hint text-[var(--color-text-muted)] hidden @[34rem]:inline">{t('send_hint')}</span>
          <ui.Button variant="quiet" size="md" className="ml-auto" data-testid="mr-compose-cancel" onClick={() => void discard()}>
            {t('discard')}
          </ui.Button>
        </div>
      </form>
    </ui.Card>
  )
}
