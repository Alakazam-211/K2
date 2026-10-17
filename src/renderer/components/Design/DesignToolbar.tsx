// Design tab controls (DD7, reworked Rosson 2026-10-16).
//
//  - `DesignPaletteButton` sits in the Browser chrome bar after Back /
//    Forward, before the URL. It IS the Select toggle: accent colour while
//    Select is on (the default when the tab opens), the normal icon colour
//    when off. No separate Select button and no S hotkey.
//  - `DesignToolbar` is the strip above the page: the note count and
//    Send / Retry.

import { K2_ICON_PX, PaletteIcon } from '@/components/icons/K2Icons'

export interface DesignPaletteButtonProps {
  /** Select (pick an element) is on. */
  armed: boolean
  onToggle: () => void
  /** Select cannot run here (web client, remote loopback refused…). */
  disabled?: boolean
}

/** The palette in the chrome bar: the Select toggle. */
export function DesignPaletteButton({ armed, onToggle, disabled = false }: DesignPaletteButtonProps): React.JSX.Element {
  return (
    <button
      type="button"
      aria-pressed={armed}
      aria-label="Select an element"
      title={armed ? 'Selecting: click an element on the page (Esc stops)' : 'Select an element on the page'}
      disabled={disabled}
      onClick={onToggle}
      data-design-palette={armed ? 'on' : 'off'}
      className={`ml-1 flex h-6 w-6 flex-shrink-0 items-center justify-center cursor-pointer disabled:opacity-30 ${
        armed
          ? 'text-[var(--color-accent)] hover:text-[var(--color-accent-hover,var(--color-accent))]'
          : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text)]'
      }`}
    >
      <PaletteIcon size={K2_ICON_PX} />
    </button>
  )
}

export interface DesignToolbarProps {
  noteCount: number
  sending: boolean
  /** The last Send stored a Thread item the agent did not get yet. */
  canRetry: boolean
  onSend: () => void
  onRetry: () => void
  /** Select is on (only changes the hint). */
  armed?: boolean
}

export function DesignToolbar({
  noteCount,
  sending,
  canRetry,
  onSend,
  onRetry,
  armed = false,
}: DesignToolbarProps): React.JSX.Element {
  const notesLabel = noteCount === 1 ? '1 note' : `${noteCount} notes`
  return (
    <div
      className="flex items-center gap-2 px-2 h-7 border-b border-[var(--color-border)] text-xs no-drag"
      data-testid="design-toolbar"
    >
      <span className="text-[var(--color-text-muted)]" data-testid="design-note-count">
        {notesLabel}
      </span>
      <span className="min-w-0 truncate text-[10px] text-[var(--color-text-muted)]" data-testid="design-hint">
        {armed ? 'Click an element to add a note' : 'Turn on the palette to pick an element'}
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
