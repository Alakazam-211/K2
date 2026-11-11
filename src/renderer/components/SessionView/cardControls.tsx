// The Thread cards' shared controls: the `--thread-card-*` tokens, the card
// button (ChoiceCard's chip, side by side) and the one-line field. The
// permission card's Deny box and a card's Dismiss line use these, so every
// inline step on a card has the same field and buttons, sized to the card,
// in both themes (chamfered in K2's square Style, `--thread-card-radius`
// elsewhere).

import type { JSX } from 'react'
import { chamferCutFor, chamferVars } from '@/components/Thread/chamfer'

export const CARD_EDGE = 'var(--thread-card-border,var(--color-border))'
export const CARD_ACCENT = 'var(--thread-card-accent,var(--color-accent))'
export const CARD_FILL = 'var(--thread-card-bg,var(--color-bg))'
export const CARD_TEXT = 'var(--thread-card-text,var(--color-text-primary))'
export const CARD_TEXT2 = 'var(--thread-card-text-secondary,var(--color-text-secondary))'
export const CARD_MUTED = 'var(--thread-card-muted,var(--color-text-muted))'
export const CARD_DANGER = 'var(--zen-danger,var(--color-status-error))'
export const CARD_RADIUS = 'rounded-[var(--thread-card-radius,0px)]'

/** A card's one-line field (give it `style={{ color: CARD_TEXT }}`). */
export const CARD_INPUT_CLASS = `w-full box-border bg-transparent px-2 py-1 text-[11px] outline-none border border-[var(--thread-card-border,var(--color-border))] focus:border-[var(--thread-card-accent,var(--color-accent))] ${CARD_RADIUS}`

/** A card button: ChoiceCard's chip, side by side. `primary` is filled. */
export function CardButton({
  label,
  testId,
  onClick,
  primary = false,
  danger = false,
  disabled = false,
}: {
  label: string
  testId: string
  onClick: () => void
  primary?: boolean
  danger?: boolean
  disabled?: boolean
}): JSX.Element {
  const edge = primary ? CARD_ACCENT : danger ? `color-mix(in srgb, ${CARD_DANGER} 60%, ${CARD_EDGE})` : CARD_EDGE
  const fill = primary ? `color-mix(in srgb, ${CARD_ACCENT} 18%, transparent)` : 'transparent'
  return (
    <button
      type="button"
      data-testid={testId}
      data-label={label}
      data-chamfer=""
      disabled={disabled}
      onClick={onClick}
      style={{ ...chamferVars({ cut: chamferCutFor(24), fill, edge, border: 1 }), borderColor: edge, background: fill, color: primary ? CARD_TEXT : CARD_TEXT2 }}
      className={`${CARD_RADIUS} border px-2.5 py-1 text-[11px] font-medium whitespace-nowrap transition-colors cursor-pointer hover:text-[var(--thread-card-text,var(--color-text-primary))] disabled:cursor-not-allowed disabled:opacity-50`}
    >
      {label}
    </button>
  )
}
