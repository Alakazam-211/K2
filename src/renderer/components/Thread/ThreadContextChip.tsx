// prd-garden-column-collapse-and-agent-list-v1 §4 (0.46.3 r10) — the Board
// context chip. A Garden widget's line (`k2.conversation.setContext`) is
// never hidden: the person sees it in the message box before they send
// (and can remove it), and on the sent message it reads as the widget's,
// not theirs. The words are fixed here; the widget supplies only the text.

import type { JSX } from 'react'
import type { ThreadContextDoc } from '@/components/SessionView/overlayThread'

const CHIP_STYLE = {
  fontSize: '0.8em',
  padding: '3px 4px 3px 10px',
  borderRadius: 999,
  background: 'var(--zen-surface-raised)',
  border: '1px dashed var(--zen-border)',
  color: 'var(--zen-text)',
} as const

/** In the message box: what rides the next send, with its ×. */
export function ThreadContextComposeChip({
  text,
  widget,
  onRemove,
}: {
  text: string
  /** The widget's folder name (author-chosen; shown as its source). */
  widget: string
  onRemove: () => void
}): JSX.Element {
  return (
    <div className="flex flex-wrap gap-1.5" style={{ marginBottom: 6 }} data-zen-context="">
      <span
        data-zen-context-chip=""
        role="note"
        aria-label={`Context from the ${widget} widget, sent with your message: ${text}`}
        title={`From the ${widget} widget. It goes with your next message, marked as the widget’s, not yours.`}
        className="inline-flex max-w-full items-center gap-1.5"
        style={CHIP_STYLE}
      >
        <span className="min-w-0 truncate" data-zen-context-text="">
          {text}
        </span>
        <button
          type="button"
          aria-label="Remove the context"
          data-zen-context-remove=""
          data-zen-soft-button=""
          onClick={(e) => {
            e.stopPropagation()
            onRemove()
          }}
          className="flex flex-shrink-0 items-center justify-center cursor-pointer"
          style={{ width: 18, height: 18, borderRadius: 999, color: 'var(--zen-text-muted)' }}
        >
          ×
        </button>
      </span>
    </div>
  )
}

/** Where the chip was, after its context didn't go ("Not sent: dtl is older"). */
export function ThreadContextNote({ note, onDismiss }: { note: string; onDismiss: () => void }): JSX.Element {
  return (
    <div
      role="status"
      data-zen-context-note=""
      className="flex items-center gap-1.5"
      style={{ marginBottom: 6, fontSize: '0.78em', color: 'var(--zen-text-muted)' }}
    >
      <span className="min-w-0 truncate">Board context · {note}</span>
      <button
        type="button"
        aria-label="Dismiss"
        data-zen-soft-button=""
        onClick={onDismiss}
        className="flex flex-shrink-0 items-center justify-center cursor-pointer"
        style={{ width: 18, height: 18, borderRadius: 999, color: 'var(--zen-text-muted)' }}
      >
        ×
      </button>
    </div>
  )
}

/** On a sent message: the widget's line, marked as the widget's. */
export function ThreadContextLine({ context, mine }: { context: ThreadContextDoc; mine: boolean }): JSX.Element {
  return (
    <span
      data-thread-context=""
      title={`Context from the ${context.widget} widget (${context.placement}), not typed by the sender`}
      className="inline-flex max-w-full items-center gap-1"
      style={{
        alignSelf: mine ? 'flex-end' : 'flex-start',
        padding: '1px 8px',
        borderRadius: 999,
        border: '1px dashed var(--zen-border)',
        fontSize: '0.72em',
        color: 'var(--zen-text-muted)',
      }}
    >
      <span className="flex-shrink-0">{context.widget}:</span>
      <span className="min-w-0 truncate" data-thread-context-text="">
        {context.text}
      </span>
    </span>
  )
}
