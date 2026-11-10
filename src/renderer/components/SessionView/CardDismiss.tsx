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

import { useState, type JSX } from 'react'
import { daemonErrorText } from '@/lib/daemon-error'

/** The daemon's limit on a dismiss reason (`CLOSE_REASON_MAX_CHARS`). */
export const CARD_DISMISS_REASON_MAX = 200

const muted = 'text-[var(--thread-card-muted,var(--color-text-muted))]'
const border = 'border-[var(--thread-card-border,var(--color-border))]'

export function CardDismiss({ onDismiss }: { onDismiss: (reason?: string) => unknown }): JSX.Element {
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
      <div className="flex justify-end">
        <button
          type="button"
          data-testid="thread-card-dismiss"
          onClick={() => setOpen(true)}
          className={`cursor-pointer bg-transparent px-1 text-[10px] ${muted} hover:text-[var(--thread-card-text,var(--color-text-primary))]`}
          title="Close this card without answering it"
        >
          Dismiss
        </button>
      </div>
    )
  }
  return (
    <div data-testid="thread-card-dismiss-form" className="flex flex-col gap-1">
      <div className="flex gap-1.5">
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
          className={`min-w-0 flex-1 bg-transparent text-[11px] text-[var(--thread-card-text,var(--color-text-primary))] outline-none border ${border} px-2 py-1`}
        />
        <button
          type="button"
          data-testid="thread-card-dismiss-confirm"
          disabled={busy}
          onClick={() => void send()}
          className={`cursor-pointer px-2 py-1 text-[11px] border ${border} text-[var(--thread-card-text,var(--color-text-primary))] disabled:cursor-not-allowed disabled:opacity-60`}
        >
          Dismiss card
        </button>
        <button
          type="button"
          data-testid="thread-card-dismiss-keep"
          disabled={busy}
          onClick={keep}
          className={`cursor-pointer bg-transparent px-1 text-[11px] ${muted}`}
        >
          Keep
        </button>
      </div>
      <div className={`text-[10px] ${muted}`}>Closes the card without answering. The agent is told; nothing is decided.</div>
      {error ? (
        <div data-testid="thread-card-dismiss-error" role="alert" className="text-[10px] text-[var(--color-danger,#c33)] leading-relaxed">
          {error}
        </div>
      ) : null}
    </div>
  )
}
