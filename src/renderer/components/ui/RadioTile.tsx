import { forwardRef } from 'react'
import { cx } from './cx'
import { Radio, type RadioProps } from './Radio'

export interface RadioTileProps extends Omit<RadioProps, 'children'> {
  /** Shown before the words: an icon chip or a picture. */
  leading?: React.ReactNode
  /** The words: a title, then a line or two. */
  children: React.ReactNode
  /** Attributes for the tile itself (a `data-*` hook for tests and shots). */
  tileProps?: Record<string, string | undefined>
}

/** One choice in a radio group, drawn as a card you pick: a chamfered tile in
 *  Terminal, a Glass tile elsewhere, with the shared radio at its right. The
 *  native radio does the work, so arrows move the choice inside the group
 *  (every tile of a group shares one `name`), Space picks, and Enter submits
 *  the form it sits in.
 *
 *  Its edge says its state: the card's hairline at rest, the control edge
 *  under the pointer, and the accent as text (4.5:1 on every card, so well
 *  over 3:1) when picked, with the radio's dot. Glass draws that picked edge
 *  2px wide and Terminal tints the fill (styles/glass/overrides.css,
 *  [data-radio-tile]). Keyboard focus is the radio's own ring. */
export const RadioTile = forwardRef<HTMLInputElement, RadioTileProps>(function RadioTile(
  { leading, children, tileProps, checked, disabled, className, ...radio },
  ref,
) {
  return (
    <label
      data-radio-tile=""
      data-checked={checked ? '' : undefined}
      data-k2-chamfer="surface"
      data-material="tile"
      className={cx(
        'relative flex items-start gap-3 px-3.5 py-3 border rounded-[var(--radius-box)] transition-colors',
        disabled ? 'cursor-not-allowed opacity-60' : 'cursor-pointer',
        checked
          ? 'border-[var(--color-accent-text)] bg-[color-mix(in_srgb,var(--color-accent)_8%,var(--color-bg-surface))]'
          : cx('border-[var(--color-border)] bg-[var(--color-bg-surface)]', !disabled && 'hover:border-[var(--color-control-edge)]'),
        className,
      )}
      {...tileProps}
    >
      {leading}
      <span className="min-w-0 flex-1">{children}</span>
      <Radio ref={ref} checked={checked} disabled={disabled} className="mt-0.5 flex-shrink-0" {...radio} />
    </label>
  )
})

/** The square that holds a tile's icon: the accent, softly. */
export function TileIcon({ children }: { children: React.ReactNode }): React.JSX.Element {
  return (
    <span
      aria-hidden="true"
      data-k2-chamfer="chip"
      data-tile-icon=""
      className="flex h-8 w-8 flex-shrink-0 items-center justify-center rounded-[var(--radius-field-multiline)] bg-[color-mix(in_srgb,var(--color-accent)_14%,transparent)] text-[var(--color-accent-text)]"
    >
      {children}
    </span>
  )
}
