// Dismiss for a waiting Thread card (prd-popup-close-and-card-dismiss-v1
// B8, 0.46.11). Rosson: "cards should have some kind of 'dismiss' system
// because sometimes the question is no longer relevant."
//
// A small "Dismiss" under the card. Pressed, it opens one line: an optional
// reason (at most 200 characters), "Dismiss card" and "Keep". Esc keeps the
// card. The daemon closes it as dismissed (shared by every client, kept in
// the history), tells the agent, and a permission card grants and denies
// nothing. A refusal is shown on the card in the daemon's words.
//
// K2 Design owns the look: Thread card tokens only.

import { useState, type JSX, type ReactNode } from 'react'
import { daemonErrorText } from '@/lib/daemon-error'
import { CARD_DANGER, CARD_INPUT_CLASS, CARD_MUTED, CARD_TEXT, CardButton } from './cardControls'

/** The daemon's limit on a dismiss reason (`CLOSE_REASON_MAX_CHARS`). */
export const CARD_DISMISS_REASON_MAX = 200

/** The card's last row: its own muted line on the left (the time frame),
 *  Dismiss on the right, its text on the card's content edge. Pressed, the
 *  row becomes the card's inline step, with the same field and buttons as
 *  a permission card's Deny: the reason on its own line, then "Dismiss
 *  card" and "Keep" (shared card controls, sized to the card). Put it
 *  inside the card, last. */
export function CardDismiss({
  onDismiss,
  aside,
}: {
  onDismiss: (reason?: string) => unknown
  /** The card's own footer line (its time frame), kept beside Dismiss. */
  aside?: ReactNode
}): JSX.Element {
  const [open, setOpen] = useState(false)
  const [reason, setReason] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const keep = (): void => {
    setOpen(false)
    setReason('')
    setError(null)
  }
  const send = async (): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      await onDismiss(reason.trim() || undefined)
    } catch (err) {
      console.warn('[thread] dismiss failed:', err)
      setError(daemonErrorText(err) || 'K2 couldn’t dismiss this card.')
      setBusy(false)
      return
    }
    setBusy(false)
    setOpen(false)
  }
  if (!open) {
    return (
      <div className="flex min-h-5 items-center gap-2">
        <div className="min-w-0 flex-1">{aside}</div>
        <button
          type="button"
          data-testid="thread-card-dismiss"
          onClick={() => setOpen(true)}
          className="-mr-1 flex-shrink-0 cursor-pointer bg-transparent px-1 py-0.5 text-[10px] font-medium text-[var(--thread-card-muted,var(--color-text-muted))] underline-offset-2 transition-colors hover:text-[var(--thread-card-text,var(--color-text-primary))] hover:underline"
          title="Close this card without answering it"
        >
          Dismiss
        </button>
      </div>
    )
  }
  return (
    <div data-testid="thread-card-dismiss-form" className="flex flex-col gap-1">
      {aside}
      <input
        autoFocus
        data-testid="thread-card-dismiss-reason"
        value={reason}
        maxLength={CARD_DISMISS_REASON_MAX}
        disabled={busy}
        onChange={(e) => setReason(e.target.value.replace(/[\r\n]+/g, ' '))}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            e.stopPropagation()
            keep()
          } else if (e.key === 'Enter') {
            e.preventDefault()
            void send()
          }
        }}
        placeholder="Why? (optional)"
        aria-label="Why you are dismissing this card (optional)"
        className={CARD_INPUT_CLASS}
        style={{ color: CARD_TEXT }}
      />
      <div className="flex items-center gap-1.5">
        <CardButton label={busy ? 'Dismissing…' : 'Dismiss card'} testId="thread-card-dismiss-confirm" disabled={busy} onClick={() => void send()} />
        <CardButton label="Keep" testId="thread-card-dismiss-keep" disabled={busy} onClick={keep} />
      </div>
      <div className="text-[10px] leading-relaxed" style={{ color: CARD_MUTED }}>
        Closes the card without answering. The agent is told; nothing is decided.
      </div>
      {error ? (
        <div data-testid="thread-card-dismiss-error" role="alert" className="text-[10px] leading-relaxed" style={{ color: CARD_DANGER }}>
          {error}
        </div>
      ) : null}
    </div>
  )
}
