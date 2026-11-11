// A reply an agent drafted, shown at the top of the conversation it answers
// (K2's own; not from OCmail). Unmistakably unsent in every look: a header
// that says "Draft by <agent> · not sent", a dashed warn-tinted edge on a
// faint warn wash, and Send / Discard of its own. It is edited in place in
// the composer's body field, which grows to fit the draft (up to most of
// the window, then scrolls).
//
// MailReader feeds it a thread's drafts (`draft: true`); a host can also show
// one through the `threadTop` slot, e.g.
//
//   <DraftCard
//     text={draft.body}
//     agent={draft.workspace}
//     updatedAt={draft.updatedAt}
//     to={draft.to} cc={draft.cc} subject={draft.subject}
//     onChange={(text) => saveDraftDebounced(draft.id, { body: text })}
//     onChangeHeaders={(h) => saveDraftDebounced(draft.id, h)}
//     onSend={(text, headers) => sendDraft(draft.id, text, headers)}  // ask first, as Send does
//     onDiscard={() => dropDraft(draft.id)}
//   />
//
// The card holds the text while it is edited (`text` seeds it and replaces
// it when it changes from outside); `onChange` reports every edit so the host
// can save it. Discard asks first (through `confirm`) and only then calls
// `onDiscard`. Send hands the current text to `onSend`; the host confirms and
// sends, as the reader's own Send does.
//
// Recipients and subject: given `to`, `cc` or `subject`, the card shows them
// as one line ("To Sam Okafor · Cc 2 people · Re: Field trip") with Edit,
// which opens the composer's own To / Cc / Subject fields. Every edit goes to
// `onChangeHeaders`, and Send hands the headers to `onSend` as well. Without
// them the card shows the text alone, as before.
//
// Read-only (`readOnly`): a draft K2 can show but not change (a Microsoft 365
// draft, or one an IMAP server saved without saying where). It still says
// "Draft by <agent> · not sent", shows the text and recipients as plain text,
// and says where it can be edited: the data's own words (`note`), else the
// provider's app. Send and Discard are not shown at all: they can't work
// here, and a disabled Send beside "not sent" would read as "not yet" rather
// than "not here". `openUrl` adds an "Open in Outlook" link when the data
// offers one.

import { useEffect, useId, useMemo, useRef, useState } from 'react'
import { formatDate, formatFullDate, splitAddresses } from '../format'
import { t } from '../i18n'
import type { InboxProvider, ReaderAddress } from '../types'
import { MailIcon, useMailUi } from '../ui'
import { ComposerBody, FieldRow, isSendChord, type ComposeConfirm } from './Composer'

/** A draft's recipients and subject as the person leaves them. */
export interface DraftHeaders {
  to: string[]
  cc: string[]
  subject: string
}

/** Why a draft can't be changed here, and where it can. */
export interface DraftReadOnly {
  /** The service that holds it: picks the default note ("…in Outlook"). */
  provider?: InboxProvider | null
  /** The data's own words for why (replaces the default note). */
  note?: string | null
  /** Where the draft opens in its own app, when the data offers one. */
  openUrl?: string | null
  /** Opens `openUrl` (default: a plain link in a new window). */
  onOpen?: (url: string) => void
}

/** A recipient: an address, or a name and address. */
export type DraftRecipient = ReaderAddress | string

export interface DraftCardProps {
  /** The draft's body (plain text). */
  text: string
  /** The agent (workspace) that wrote it, as people know it. */
  agent: string
  /** When it was last written (ISO 8601). */
  updatedAt?: string | null
  /** Every edit, for the host to save. */
  onChange?: (text: string) => void
  /** Send it (the host asks first). `headers` come along when the card
   *  shows recipients. Not called on a read-only draft. */
  onSend?: (text: string, headers?: DraftHeaders) => void
  /** Throw it away (after the card's own ask). Not called on a read-only
   *  draft. */
  onDiscard?: () => void
  /** Asks before Discard (none: discards at once). */
  confirm?: ComposeConfirm
  /** A send or discard is under way. */
  busy?: boolean
  /** Recipients. Any of `to`, `cc`, `subject` shows the recipients line. */
  to?: readonly DraftRecipient[]
  cc?: readonly DraftRecipient[]
  subject?: string | null
  /** Every edit to To, Cc or Subject, for the host to save. */
  onChangeHeaders?: (headers: DraftHeaders) => void
  /** Shown, not editable: `true`, or why and where to edit it. */
  readOnly?: boolean | DraftReadOnly
}

const APP: Partial<Record<InboxProvider, 'app_microsoft' | 'app_gmail'>> = {
  microsoft: 'app_microsoft',
  gmail: 'app_gmail',
}

function asAddress(r: DraftRecipient): ReaderAddress {
  return typeof r === 'string' ? { address: r } : r
}

function addressesOf(list: readonly DraftRecipient[] | undefined): string[] {
  return (list ?? []).map((r) => asAddress(r).address).filter((a) => a.length > 0)
}

/** "Sam Okafor <sam@example.com>", or the address alone. */
function fullName(a: ReaderAddress): string {
  return a.name ? `${a.name} <${a.address}>` : a.address
}

export function DraftCard({
  text,
  agent,
  updatedAt,
  onChange,
  onSend,
  onDiscard,
  confirm,
  busy = false,
  to,
  cc,
  subject,
  onChangeHeaders,
  readOnly,
}: DraftCardProps): React.JSX.Element {
  const ui = useMailUi()
  const idp = `mr-draft-${useId().replace(/[^a-zA-Z0-9_-]/g, '')}`
  const locked = readOnly === true || (typeof readOnly === 'object' && readOnly !== null)
  const ro: DraftReadOnly = typeof readOnly === 'object' && readOnly !== null ? readOnly : {}
  const app = ro.provider ? APP[ro.provider] : undefined
  const note = ro.note?.trim() || (app ? t('draft_readonly_in', { app: t(app) }) : t('draft_readonly_elsewhere'))

  const [value, setValue] = useState(text)
  useEffect(() => setValue(text), [text])

  // ── Recipients and subject ──
  const hasHeaders = to !== undefined || cc !== undefined || subject !== undefined
  const toKey = addressesOf(to).join(', ')
  const ccKey = addressesOf(cc).join(', ')
  const subjectKey = subject ?? ''
  const [toText, setToText] = useState(toKey)
  const [ccText, setCcText] = useState(ccKey)
  const [subjectText, setSubjectText] = useState(subjectKey)
  // Replaced from outside only when it differs from what's typed (so a
  // trailing comma or space survives the host echoing the edit back).
  useEffect(() => setToText((cur) => (splitAddresses(cur).join(', ') === toKey ? cur : toKey)), [toKey])
  useEffect(() => setCcText((cur) => (splitAddresses(cur).join(', ') === ccKey ? cur : ccKey)), [ccKey])
  useEffect(() => setSubjectText((cur) => (cur.trim() === subjectKey.trim() ? cur : subjectKey)), [subjectKey])
  const [open, setOpen] = useState(false)
  const fieldsRef = useRef<HTMLDivElement>(null)
  const opened = useRef(false)
  // Edit opens on To (the host's field may not take a ref).
  useEffect(() => {
    if (open && opened.current && !locked) fieldsRef.current?.querySelector<HTMLInputElement>('input')?.focus()
  }, [open, locked])

  const headers = (next: Partial<{ to: string; cc: string; subject: string }> = {}): DraftHeaders => ({
    to: splitAddresses(next.to ?? toText),
    cc: splitAddresses(next.cc ?? ccText),
    subject: (next.subject ?? subjectText).trim(),
  })
  const current = headers()
  // The names the data gave, for the one-line summary.
  const names = useMemo(() => {
    const m = new Map<string, string>()
    for (const r of [...(to ?? []), ...(cc ?? [])]) {
      const a = asAddress(r)
      if (a.name) m.set(a.address.toLowerCase(), a.name)
    }
    return m
  }, [to, cc])
  const shown = (address: string): string => names.get(address.toLowerCase()) ?? address
  const blocker = hasHeaders
    ? current.to.length === 0
      ? t('draft_needs_to')
      : current.subject.length === 0
        ? t('draft_needs_subject')
        : null
    : null
  const canSend = value.trim().length > 0 && !busy && !blocker && !!onSend

  const send = (): void => {
    if (!canSend || !onSend) return
    if (hasHeaders) onSend(value, current)
    else onSend(value)
  }

  const editHeader = (field: 'to' | 'cc' | 'subject', next: string): void => {
    if (field === 'to') setToText(next)
    else if (field === 'cc') setCcText(next)
    else setSubjectText(next)
    onChangeHeaders?.(headers({ [field]: next }))
  }

  const discard = async (): Promise<void> => {
    if (!onDiscard) return
    if (confirm) {
      const ok = await confirm({
        title: t('confirm_discard_draft_title'),
        message: t('confirm_discard_draft_message', { agent }),
        confirmLabel: t('discard'),
        destructive: true,
      })
      if (!ok) return
    }
    onDiscard()
  }

  const summary = (
    <p data-testid="mr-draft-headers-summary" className="flex-1 min-w-0 truncate type-hint text-[var(--color-text-secondary)]">
      <span className="text-[var(--color-text-muted)]">{t('to')}</span>{' '}
      <span className="text-[var(--color-text-primary)]">
        {current.to.length === 0
          ? t('draft_no_recipients')
          : current.to.length === 1
            ? shown(current.to[0])
            : `${shown(current.to[0])} ${t('draft_more', { count: current.to.length - 1 })}`}
      </span>
      {current.cc.length > 0 && (
        <>
          <span aria-hidden="true"> · </span>
          <span className="text-[var(--color-text-muted)]">{t('cc')}</span>{' '}
          <span className="text-[var(--color-text-primary)]">
            {current.cc.length === 1 ? shown(current.cc[0]) : t('draft_cc_count', { count: current.cc.length })}
          </span>
        </>
      )}
      <span aria-hidden="true"> · </span>
      <span className="sr-only">{t('subject')}: </span>
      <span className="text-[var(--color-text-primary)]">{current.subject || t('no_subject')}</span>
    </p>
  )

  const toggleLabel = locked
    ? open
      ? t('draft_hide_headers')
      : t('draft_show_headers')
    : open
      ? t('draft_done_headers')
      : t('draft_edit_headers')

  const headerBlock = hasHeaders && (
    <div data-mr="draft-headers" className="flex flex-col gap-2">
      <div className="flex items-center gap-2 min-w-0">
        {open ? (
          <h4 className="flex-1 min-w-0 truncate type-hint text-[var(--color-text-muted)]">{t('draft_headers')}</h4>
        ) : (
          summary
        )}
        <ui.Button
          variant="quiet"
          size="sm"
          className="flex-shrink-0"
          data-testid="mr-draft-headers-toggle"
          aria-expanded={open}
          aria-controls={`${idp}-headers`}
          aria-label={!locked && !open ? t('draft_edit_headers_label') : undefined}
          onClick={() => {
            opened.current = true
            setOpen((o) => !o)
          }}
        >
          {toggleLabel}
        </ui.Button>
      </div>
      {open &&
        (locked ? (
          // Plain text: no field edges, nothing to focus.
          <dl id={`${idp}-headers`} data-mr="draft-headers-plain" data-testid="mr-draft-headers" className="flex flex-col gap-1.5">
            {(
              [
                ['to', (to ?? []).map(asAddress).map(fullName).join(', ') || t('draft_no_recipients')],
                ['cc', (cc ?? []).map(asAddress).map(fullName).join(', ')],
                ['subject', subject || t('no_subject')],
              ] as const
            )
              .filter(([, v]) => v.length > 0)
              .map(([k, v]) => (
                <div key={k} className="grid grid-cols-[4.5rem_minmax(0,1fr)] items-baseline gap-2">
                  <dt className="type-hint text-[var(--color-text-muted)]">{t(k)}</dt>
                  <dd data-testid={`mr-draft-${k}`} className="type-row text-[var(--color-text-primary)] break-words">
                    {v}
                  </dd>
                </div>
              ))}
          </dl>
        ) : (
          <div ref={fieldsRef} id={`${idp}-headers`} data-mr="draft-headers-fields" data-testid="mr-draft-headers" className="flex flex-col gap-2">
            <FieldRow label={t('to')} htmlFor={`${idp}-to`}>
              <ui.TextField
                id={`${idp}-to`}
                data-testid="mr-draft-to"
                className="w-full"
                value={toText}
                placeholder={t('to_placeholder')}
                disabled={busy}
                onChange={(e) => editHeader('to', e.target.value)}
              />
            </FieldRow>
            <FieldRow label={t('cc')} htmlFor={`${idp}-cc`}>
              <ui.TextField
                id={`${idp}-cc`}
                data-testid="mr-draft-cc"
                className="w-full"
                value={ccText}
                disabled={busy}
                onChange={(e) => editHeader('cc', e.target.value)}
              />
            </FieldRow>
            <FieldRow label={t('subject')} htmlFor={`${idp}-subject`}>
              <ui.TextField
                id={`${idp}-subject`}
                data-testid="mr-draft-subject"
                className="w-full"
                value={subjectText}
                disabled={busy}
                onChange={(e) => editHeader('subject', e.target.value)}
              />
            </FieldRow>
          </div>
        ))}
    </div>
  )

  return (
    <section
      data-mr="draft-card"
      data-testid="mr-draft-card"
      data-readonly={locked ? 'true' : undefined}
      aria-label={`${t('draft_by', { agent })}, ${t('draft_not_sent')}${locked ? `. ${note}` : ''}`}
      // Dashed: nothing sent looks like this. The warn tone, faintly, on the
      // fill and firmly on the edge and the label.
      className="flex flex-col gap-3 p-4 rounded-[var(--radius-box)] border-2 border-dashed border-[color-mix(in_srgb,var(--color-status-warn)_70%,transparent)] bg-[color-mix(in_srgb,var(--color-status-warn)_7%,var(--color-bg))]"
    >
      <header className="flex items-center gap-2 min-w-0">
        <MailIcon name="agent" size={14} className="flex-shrink-0 text-[var(--color-status-warn-text)]" />
        <h3 className="min-w-0 truncate type-name text-[var(--color-text-primary)]">{t('draft_by', { agent })}</h3>
        <ui.Tag tone="warn" data-testid="mr-draft-unsent" {...ui.shape('chip')}>
          {t('draft_not_sent')}
        </ui.Tag>
        {updatedAt && (
          <time className="ml-auto flex-shrink-0 type-hint tabular-nums text-[var(--color-text-muted)]" dateTime={updatedAt} title={formatFullDate(updatedAt)}>
            {t('draft_updated', { time: formatDate(updatedAt) })}
          </time>
        )}
      </header>
      {locked ? (
        <p data-testid="mr-draft-readonly" className="type-hint text-[var(--color-text-secondary)] -mt-1">
          {note}
          {ro.openUrl && (
            <>
              {' '}
              <a
                data-testid="mr-draft-open"
                href={ro.openUrl}
                target="_blank"
                rel="noopener noreferrer"
                className="text-[var(--color-accent)] underline underline-offset-2 hover:text-[var(--color-text-primary)]"
                onClick={(e) => {
                  if (!ro.onOpen || !ro.openUrl) return
                  e.preventDefault()
                  ro.onOpen(ro.openUrl)
                }}
              >
                {app ? t('draft_open_in', { app: t(app) }) : t('draft_open_elsewhere')}
              </a>
            </>
          )}
        </p>
      ) : (
        <p className="type-hint text-[var(--color-text-secondary)] -mt-1">{t('draft_hint')}</p>
      )}
      {headerBlock}
      {locked ? (
        text.trim().length > 0 ? (
          <div
            data-mr="draft-text"
            data-testid="mr-draft-body"
            className="whitespace-pre-wrap break-words type-row leading-relaxed text-[var(--color-text-primary)]"
          >
            {text}
          </div>
        ) : (
          <p data-testid="mr-draft-body" className="type-hint text-[var(--color-text-muted)]">
            {t('draft_no_text')}
          </p>
        )
      ) : (
        <ComposerBody
          data-testid="mr-draft-body"
          aria-label={t('draft_by', { agent })}
          value={value}
          autoGrow
          // Fits the whole draft up to most of the window, then scrolls.
          className="min-h-[140px] max-h-[min(60vh,36rem)]"
          onChange={(e) => {
            setValue(e.target.value)
            onChange?.(e.target.value)
          }}
          onKeyDown={(e) => {
            if (isSendChord(e)) {
              e.preventDefault()
              send()
            }
          }}
        />
      )}
      {!locked && (
        <div className="flex items-center gap-2">
          <ui.Button variant="primary" size="md" data-testid="mr-draft-send" disabled={!canSend} onClick={send}>
            {t('send')}
          </ui.Button>
          <span data-testid={blocker ? 'mr-draft-blocker' : undefined} className="type-hint text-[var(--color-text-muted)]">
            {blocker ?? t('send_hint').split(' · ')[0]}
          </span>
          <ui.Button variant="quiet" size="md" className="ml-auto" data-testid="mr-draft-discard" disabled={busy || !onDiscard} onClick={() => void discard()}>
            {t('discard')}
          </ui.Button>
        </div>
      )}
    </section>
  )
}
