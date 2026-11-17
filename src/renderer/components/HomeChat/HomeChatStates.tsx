// A Home chat that failed to open: what went wrong and the next step.
//
// The reason is the muted text colour at full strength (the Thread's empty
// state colour, 4.5:1 on the page), in sentence case, and Try again reads
// the chat again. The button lives inside the Thread box, so it takes the
// Thread's shape: cut corners in Terminal (`data-chamfer`), a pill in Glass.

import type { JSX } from 'react'
import { ThreadChatEmpty } from '@/components/Thread/ThreadChat'
import { CHAMFER_CUT, chamferVars } from '@/components/Thread/chamfer'

/** A daemon reason as a sentence: a capital first letter and a full stop. */
export function reasonSentence(reason: string): string {
  const t = reason.trim()
  if (!t) return ''
  const s = t[0].toUpperCase() + t.slice(1)
  return /[.!?…]$/.test(s) ? s : `${s}.`
}

const FILL = 'var(--zen-surface-raised)'
const EDGE = 'var(--zen-control-edge, var(--zen-border))'

export function HomeChatErrorState({ reason, onRetry }: { reason: string | null; onRetry: () => void }): JSX.Element {
  const why = reasonSentence(reason ?? '')
  return (
    <ThreadChatEmpty testId="home-chat-error">
      <p style={{ color: 'var(--zen-text)' }}>Couldn't open this chat.</p>
      {why && (
        <p className="mt-1" data-testid="home-chat-error-reason">
          {why}
        </p>
      )}
      <button
        type="button"
        className="k2-focus mt-3 inline-flex cursor-pointer items-center justify-center rounded-[var(--radius-pill)]"
        data-testid="home-chat-retry"
        data-zen-soft-button=""
        data-chamfer=""
        onClick={onRetry}
        style={{
          minHeight: 28,
          padding: '0 14px',
          border: `1px solid ${EDGE}`,
          background: FILL,
          color: 'var(--zen-text)',
          fontSize: 'var(--zen-type-sm, 0.8em)',
          ...chamferVars({ cut: CHAMFER_CUT.button, fill: FILL, edge: EDGE, border: 1 }),
        }}
      >
        Try again
      </button>
    </ThreadChatEmpty>
  )
}
