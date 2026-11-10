// A small composer for a reply or a new message (K2's own; OCmail's composer
// is not ported). Plain text only, no attachments. Send asks first; the
// reader decides what Send and Save draft call.

import { useState } from 'react'
import { splitAddresses } from '../format'
import { t } from '../i18n'

export type ComposeMode =
  | { kind: 'new'; inbox: string }
  | { kind: 'reply'; inbox: string; id: string; subject: string; to: string }

export interface ComposeValues {
  to: string[]
  cc: string[]
  subject: string
  body: string
}

export function Composer({
  mode,
  busy,
  onSend,
  onSaveDraft,
  onCancel,
}: {
  mode: ComposeMode
  busy: boolean
  onSend: (v: ComposeValues) => void
  onSaveDraft: (v: ComposeValues) => void
  onCancel: () => void
}): React.JSX.Element {
  const reply = mode.kind === 'reply'
  const [to, setTo] = useState(reply ? mode.to : '')
  const [cc, setCc] = useState('')
  const [subject, setSubject] = useState(reply ? (/^re:/i.test(mode.subject) ? mode.subject : `Re: ${mode.subject}`) : '')
  const [body, setBody] = useState('')
  const values = (): ComposeValues => ({ to: splitAddresses(to), cc: splitAddresses(cc), subject: subject.trim(), body })
  const canSend = body.trim().length > 0 && (reply || (splitAddresses(to).length > 0 && subject.trim().length > 0))
  const field = 'w-full px-2 py-1 type-row border border-[var(--color-border)] bg-[var(--color-bg)] text-[var(--color-text-primary)]'
  return (
    <form
      data-mr="composer"
      data-testid="mr-composer"
      className="flex flex-col gap-2 p-4 border-t border-[var(--color-border)]"
      onSubmit={(e) => {
        e.preventDefault()
        if (canSend && !busy) onSend(values())
      }}
    >
      <div className="type-hint text-[var(--color-text-muted)]">
        {t('from')}: {mode.inbox}
      </div>
      <label className="flex flex-col gap-0.5 type-hint text-[var(--color-text-muted)]">
        {t('to')}
        <input
          data-testid="mr-compose-to"
          className={field}
          value={to}
          disabled={reply}
          placeholder={t('to_placeholder')}
          onChange={(e) => setTo(e.target.value)}
        />
      </label>
      {!reply && (
        <label className="flex flex-col gap-0.5 type-hint text-[var(--color-text-muted)]">
          {t('cc')}
          <input data-testid="mr-compose-cc" className={field} value={cc} onChange={(e) => setCc(e.target.value)} />
        </label>
      )}
      <label className="flex flex-col gap-0.5 type-hint text-[var(--color-text-muted)]">
        {t('subject')}
        <input
          data-testid="mr-compose-subject"
          className={field}
          value={subject}
          disabled={reply}
          onChange={(e) => setSubject(e.target.value)}
        />
      </label>
      <label className="flex flex-col gap-0.5 type-hint text-[var(--color-text-muted)]">
        {t('body')}
        <textarea data-testid="mr-compose-body" className={`${field} min-h-[160px]`} value={body} onChange={(e) => setBody(e.target.value)} />
      </label>
      <div className="flex gap-2">
        <button
          type="submit"
          data-testid="mr-compose-send"
          disabled={!canSend || busy}
          className="px-3 py-1 type-row bg-[var(--color-accent-fill)] text-[var(--color-on-accent)] disabled:opacity-60"
        >
          {busy ? t('sending') : t('send')}
        </button>
        <button
          type="button"
          data-testid="mr-compose-draft"
          disabled={body.trim().length === 0 || busy}
          onClick={() => onSaveDraft(values())}
          className="px-3 py-1 type-row border border-[var(--color-border)] hover:bg-[var(--color-bg-hover)] disabled:opacity-60"
        >
          {t('save_draft')}
        </button>
        <button type="button" data-testid="mr-compose-cancel" onClick={onCancel} className="px-3 py-1 type-row text-[var(--color-text-secondary)]">
          {t('cancel')}
        </button>
      </div>
    </form>
  )
}
