/**
 * TerminalPane's two non-grid states, kept out of the pane's own file.
 *
 * - `TerminalHeldLabel`: the pane's spawn is held (`holdSpawn`, the pinned
 *   Chat's first ensure). Drawn inside the grid, so the grid itself (and its
 *   measure for the spawn) is the same element when the hold lifts.
 * - `TerminalErrorBox`: a failed spawn or attach. A view that shows the
 *   Thread keeps it and puts this in the terminal's place; a terminal-only
 *   view shows just this, as it always has.
 */

export function TerminalHeldLabel({ text = 'Loading session…' }: { text?: string }): React.JSX.Element {
  return (
    <div
      data-testid="terminal-pane-held"
      className="text-xs text-[var(--color-text-muted)]"
      style={{
        position: 'absolute',
        inset: 0,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontFamily: 'var(--font-sans, inherit)',
        whiteSpace: 'normal',
        background: 'var(--color-bg)',
        zIndex: 2,
        pointerEvents: 'none',
      }}
    >
      {text}
    </div>
  )
}

export function TerminalErrorBox({ message }: { message: string }): React.JSX.Element {
  return (
    <div
      data-testid="terminal-pane-error"
      style={{
        padding: 16,
        color: '#ff6666',
        fontFamily: 'monospace',
        fontSize: 12,
        whiteSpace: 'pre-wrap',
      }}
    >
      Kessel: {message}
    </div>
  )
}
