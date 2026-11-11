// A reply an agent drafted, shown at the top of the conversation it answers
// (K2's own; not from OCmail). Unmistakably unsent in every look: a header
// that says "Draft by <agent> · not sent", a dashed warn-tinted edge on a
// faint warn wash, and Send / Discard of its own. It is edited in place in
// the composer's body field.
//
// FOR THE BUILDER ADDING THE DATA: nothing feeds this yet. When the daemon
// serves an agent's pending reply for a thread, pass it to MailReader's
// `threadTop` slot, e.g.
//
//   threadTop={(messages) => draft && (
//     <DraftCard
//       text={draft.body}
//       agent={draft.workspace}
//       updatedAt={draft.updatedAt}
//       onChange={(text) => saveDraftDebounced(draft.id, text)}
//       onSend={(text) => sendDraft(draft.id, text)}   // ask first, as Send does
//       onDiscard={() => dropDraft(draft.id)}
//     />)}
//
// The card holds the text while it is edited (`text` seeds it and replaces
// it when it changes from outside); `onChange` reports every edit so the host
// can save it. Discard asks first (through `confirm`) and only then calls
// `onDiscard`. Send hands the current text to `onSend`; the host confirms and
// sends, as the reader's own Send does.

import { useEffect, useState } from 'react'
import { formatDate, formatFullDate } from '../format'
import { t } from '../i18n'
import { MailIcon, useMailUi } from '../ui'
import { ComposerBody, isSendChord, type ComposeConfirm } from './Composer'

export interface DraftCardProps {
  /** The draft's body (plain text). */
  text: string
  /** The agent (workspace) that wrote it, as people know it. */
  agent: string
  /** When it was last written (ISO 8601). */
  updatedAt?: string | null
  /** Every edit, for the host to save. */
  onChange?: (text: string) => void
  /** Send it (the host asks first). */
  onSend: (text: string) => void
  /** Throw it away (after the card's own ask). */
  onDiscard: () => void
  /** Asks before Discard (none: discards at once). */
  confirm?: ComposeConfirm
  /** A send or discard is under way. */
  busy?: boolean
}

export function DraftCard({ text, agent, updatedAt, onChange, onSend, onDiscard, confirm, busy = false }: DraftCardProps): React.JSX.Element {
  const ui = useMailUi()
  const [value, setValue] = useState(text)
  useEffect(() => setValue(text), [text])
  const canSend = value.trim().length > 0 && !busy

  const discard = async (): Promise<void> => {
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

  return (
    <section
      data-mr="draft-card"
      data-testid="mr-draft-card"
      aria-label={`${t('draft_by', { agent })}, ${t('draft_not_sent')}`}
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
      <p className="type-hint text-[var(--color-text-secondary)] -mt-1">{t('draft_hint')}</p>
      <ComposerBody
        data-testid="mr-draft-body"
        aria-label={t('draft_by', { agent })}
        value={value}
        className="min-h-[140px]"
        onChange={(e) => {
          setValue(e.target.value)
          onChange?.(e.target.value)
        }}
        onKeyDown={(e) => {
          if (isSendChord(e)) {
            e.preventDefault()
            if (canSend) onSend(value)
          }
        }}
      />
      <div className="flex items-center gap-2">
        <ui.Button variant="primary" size="md" data-testid="mr-draft-send" disabled={!canSend} onClick={() => onSend(value)}>
          {t('send')}
        </ui.Button>
        <span className="type-hint text-[var(--color-text-muted)]">{t('send_hint').split(' · ')[0]}</span>
        <ui.Button variant="quiet" size="md" className="ml-auto" data-testid="mr-draft-discard" disabled={busy} onClick={() => void discard()}>
          {t('discard')}
        </ui.Button>
      </div>
    </section>
  )
}
