// Design tab toolbar (DD7): a visible Select toggle (S inside the page does
// the same), the tray count, and Send / Retry. The palette icon comes from
// `K2Icons.tsx` at integration (`icon` prop); this file draws no glyphs.

import type { ReactNode } from 'react'

export interface DesignToolbarProps {
  /** Select (pick an element) is on. */
  armed: boolean
  onToggleSelect: () => void
  noteCount: number
  sending: boolean
  /** The last Send stored a Thread item the agent did not get yet. */
  canRetry: boolean
  onSend: () => void
  onRetry: () => void
  /** Optional leading icon (the palette from K2Icons). */
  icon?: ReactNode
  /** Select cannot run here (web client, remote loopback refused…). */
  selectDisabled?: boolean
}

export function DesignToolbar({
  armed,
  onToggleSelect,
  noteCount,
  sending,
  canRetry,
  onSend,
  onRetry,
  icon,
  selectDisabled = false,
}: DesignToolbarProps): React.JSX.Element {
  const notesLabel = noteCount === 1 ? '1 note' : `${noteCount} notes`
  return (
    <div
      className="flex items-center gap-2 px-2 h-7 border-b border-[var(--color-border)] text-xs no-drag"
      data-testid="design-toolbar"
    >
      {icon ? <span className="flex-shrink-0 text-[var(--color-text-muted)]">{icon}</span> : null}
      <button
        type="button"
        aria-pressed={armed}
        disabled={selectDisabled}
        onClick={onToggleSelect}
        title={armed ? 'Stop selecting (S or Esc)' : 'Select an element on the page (S)'}
        className={`flex items-center gap-1.5 px-2 h-5 cursor-pointer disabled:opacity-50 ${
          armed
            ? 'bg-[var(--color-accent)] text-[var(--color-on-accent)]'
            : 'border border-[var(--color-border)] text-[var(--color-text-primary)]'
        }`}
      >
        Select
        <kbd aria-hidden="true" className="text-[10px] opacity-70 font-mono">
          S
        </kbd>
      </button>
      <span className="text-[var(--color-text-muted)]" data-testid="design-note-count">
        {notesLabel}
      </span>
      <span className="flex-1" />
      {canRetry ? (
        <button
          type="button"
          onClick={onRetry}
          disabled={sending}
          className="px-2 h-5 border border-[var(--color-border)] cursor-pointer disabled:opacity-50"
        >
          {sending ? 'Retrying…' : 'Retry'}
        </button>
      ) : (
        <button
          type="button"
          onClick={onSend}
          disabled={sending || noteCount === 0}
          title={noteCount === 0 ? 'Add a note first' : 'Send the notes to the agent and the Thread'}
          className="px-2 h-5 bg-[var(--color-accent)] text-[var(--color-on-accent)] cursor-pointer disabled:opacity-50"
        >
          {sending ? 'Sending…' : 'Send'}
        </button>
      )}
    </div>
  )
}
