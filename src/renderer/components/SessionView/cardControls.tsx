// The Thread cards' shared controls: the `--thread-card-*` tokens, the card
// button (ChoiceCard's chip, side by side) and the one-line field. The
// permission card's Deny box and a card's Dismiss line use these, so every
// inline step on a card has the same field and buttons, sized to the card,
// in both themes (chamfered in K2's square Style, the control radius
// elsewhere: a pill in Glass).
//
// Words on a card read at 4.5:1 in every look: the primary button is the
// accent's fill under its own label colour (as the Settings accent button),
// a link is the accent's text colour, and a warning or OK tag uses the
// Style's text grade of that colour, never the bright status dot colour.
// Each falls back to the older token, so a Garden that sets none of these
// keeps its own look.

import type { InputHTMLAttributes, JSX } from 'react'
import { chamferCutFor, chamferVars } from '@/components/Thread/chamfer'

export const CARD_EDGE = 'var(--thread-card-border,var(--color-border))'
export const CARD_ACCENT = 'var(--thread-card-accent,var(--color-accent))'
export const CARD_FILL = 'var(--thread-card-bg,var(--color-bg))'
export const CARD_TEXT = 'var(--thread-card-text,var(--color-text-primary))'
export const CARD_TEXT2 = 'var(--thread-card-text-secondary,var(--color-text-secondary))'
export const CARD_MUTED = 'var(--thread-card-muted,var(--color-text-muted))'
export const CARD_DANGER = 'var(--zen-danger,var(--color-status-error))'
export const CARD_RADIUS = 'rounded-[var(--thread-card-radius,0px)]'
/** The primary button's fill, and its label (4.5:1 on it). */
export const CARD_ACCENT_FILL = 'var(--thread-card-accent-fill,var(--thread-card-accent,var(--color-accent)))'
export const CARD_ACCENT_FILL_HOVER = `var(--thread-card-accent-fill-hover,${CARD_ACCENT_FILL})`
export const CARD_ON_ACCENT = 'var(--thread-card-on-accent,var(--color-on-accent))'
/** A link's words ("Open in Settings"). */
export const CARD_LINK = `var(--thread-card-link,${CARD_ACCENT})`
/** Words in the warning, OK and error colours. */
export const CARD_WARN_TEXT = 'var(--thread-card-warn-text,var(--zen-needs-you,var(--color-status-warn-text)))'
export const CARD_OK_TEXT = 'var(--thread-card-ok-text,var(--zen-ok,var(--color-status-ok)))'
export const CARD_DANGER_TEXT = `var(--zen-danger-text,${CARD_DANGER})`
/** A control's edge (a field, an outlined button): 3:1 on the card, as the
 *  Settings fields' edge. */
export const CARD_CONTROL_EDGE = `var(--thread-card-control-edge,${CARD_EDGE})`
/** A field's placeholder: the muted words, 4.5:1 on the card. */
export const CARD_PLACEHOLDER_CLASS = 'placeholder:text-[var(--thread-card-muted,var(--color-text-muted))]'
/** The strong danger fill (as the confirm dialog's destructive button) and its label. */
export const CARD_DANGER_FILL = `var(--thread-card-danger-fill,${CARD_DANGER})`
export const CARD_ON_DANGER = 'var(--thread-card-on-danger,var(--color-on-danger))'
/** A control's corner: a button, a field or a tag. */
export const CARD_CONTROL_RADIUS = 'rounded-[var(--thread-card-control-radius,var(--thread-card-radius,var(--radius-field)))]'

/** A card's one-line field: cut corners in the square Style (the box draws
 *  the edge, which turns the accent while the field has focus), the
 *  control's corner elsewhere. */
export function CardField({ testId, className, ...input }: InputHTMLAttributes<HTMLInputElement> & { testId?: string }): JSX.Element {
  return (
    <div
      data-chamfer=""
      data-card-field=""
      style={{ ...chamferVars({ cut: chamferCutFor(24), fill: CARD_FILL, edge: CARD_CONTROL_EDGE, border: 1 }), borderColor: CARD_CONTROL_EDGE }}
      className={`box-border w-full min-w-0 border focus-within:!border-[var(--thread-card-accent,var(--color-accent))] ${CARD_CONTROL_RADIUS} ${className ?? ''}`}
    >
      <input
        data-testid={testId}
        {...input}
        className={`block w-full min-w-0 box-border bg-transparent border-0 px-2.5 py-1 text-[11px] outline-none ${CARD_PLACEHOLDER_CLASS}`}
        style={{ color: CARD_TEXT, ...input.style }}
      />
    </div>
  )
}

/** A card button, side by side. `primary` is the accent's fill with its own
 *  label colour, as the Settings accent button; `danger` the strong danger
 *  fill with its own label, as the confirm dialog's destructive button; any
 *  other is outlined, as the Settings quiet button. */
export function CardButton({
  label,
  testId,
  onClick,
  primary = false,
  danger = false,
  disabled = false,
  type = 'button',
}: {
  label: string
  testId: string
  onClick?: () => void
  primary?: boolean
  danger?: boolean
  disabled?: boolean
  type?: 'button' | 'submit'
}): JSX.Element {
  const edge = primary ? CARD_ACCENT_FILL : danger ? CARD_DANGER_FILL : CARD_CONTROL_EDGE
  // The square Style draws the fill on a layer over the edge, so every fill
  // here is opaque: an outlined button takes the card's own fill.
  const fill = primary ? CARD_ACCENT_FILL : danger ? CARD_DANGER_FILL : CARD_FILL
  return (
    <button
      type={type}
      data-testid={testId}
      data-label={label}
      data-chamfer=""
      data-card-primary={primary ? '' : undefined}
      data-card-danger={danger && !primary ? '' : undefined}
      disabled={disabled}
      onClick={onClick}
      style={{ ...chamferVars({ cut: chamferCutFor(24), fill, edge, border: 1 }), borderColor: edge, background: fill, color: primary ? CARD_ON_ACCENT : danger ? CARD_ON_DANGER : CARD_TEXT2 }}
      className={`${CARD_CONTROL_RADIUS} border px-2.5 py-1 text-[11px] font-medium whitespace-nowrap transition-colors cursor-pointer disabled:cursor-not-allowed disabled:opacity-50 ${
        primary
          ? 'hover:enabled:!bg-[var(--thread-card-accent-fill-hover,var(--thread-card-accent-fill,var(--thread-card-accent,var(--color-accent))))]'
          : danger
            ? ''
            : 'hover:text-[var(--thread-card-text,var(--color-text-primary))]'
      }`}
    >
      {label}
    </button>
  )
}
