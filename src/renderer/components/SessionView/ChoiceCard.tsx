// The Thread's choice card (ask/choice): lettered options, a time frame,
// who answered. Shared by the desktop Thread, the Zen Thread widget and the
// Grok Bot chat view.

import { useState, type JSX } from 'react'
import { cardExpiryLabel } from './overlayThread'
import { CHAMFER_CUT, chamferCutFor, chamferVars } from '@/components/Thread/chamfer'

const CARD_EDGE = 'var(--thread-card-border,var(--color-border))'
const CARD_ACCENT = 'var(--thread-card-accent,var(--color-accent))'

/** A, B, … Z, AA — UI letter only; the posted answer is still the option label. */
export function choiceLetter(index: number): string {
  let n = index
  let s = ''
  do {
    s = String.fromCharCode(65 + (n % 26)) + s
    n = Math.floor(n / 26) - 1
  } while (n >= 0)
  return s
}

export function ChoiceCard({
  options,
  allowCustom,
  status,
  answer,
  answeredBy,
  expiresAt,
  nowSec,
  onPick,
}: {
  options: { label: string }[]
  allowCustom: boolean
  status: string
  answer?: string | null
  /** `cardAnswererLabel` of the card's `answeredBy` ("You", a login). */
  answeredBy?: string | null
  /** The card's time frame (unix seconds); the daemon expires it. */
  expiresAt?: number | null
  nowSec?: number
  onPick: (label: string) => void
}): JSX.Element {
  const pending = status === 'pending'
  const expiry =
    status === 'expired'
      ? 'expired'
      : pending
        ? cardExpiryLabel(expiresAt, nowSec ?? Math.floor(Date.now() / 1000))
        : null
  const [custom, setCustom] = useState('')
  return (
    <div
      data-testid="thread-choice-card"
      // K2's square Style (desktop Thread): chamfered, card and options.
      data-chamfer=""
      style={chamferVars({ cut: CHAMFER_CUT.surface, fill: 'var(--thread-card-bg,var(--color-bg))', edge: CARD_EDGE, border: 1 })}
      className="mt-1 w-full box-border flex flex-col gap-1.5 rounded-[var(--thread-card-radius,0px)] border border-[var(--thread-card-border,var(--color-border))] bg-[var(--thread-card-bg,var(--color-bg))] px-2 py-1.5"
    >
      {options.map((opt, i) => {
          const selected = answer === opt.label
          const letter = choiceLetter(i)
          return (
            <button
              key={`${opt.label}-${i}`}
              type="button"
              data-testid="thread-choice-chip"
              data-chamfer=""
              style={chamferVars({
                cut: chamferCutFor(26),
                fill: selected ? `color-mix(in srgb, ${CARD_ACCENT} 15%, transparent)` : 'transparent',
                edge: selected ? CARD_ACCENT : CARD_EDGE,
                border: 1,
              })}
              data-label={opt.label}
              data-letter={letter}
              data-primary={i === 0 ? 'true' : 'false'}
              disabled={!pending}
              onClick={() => onPick(opt.label)}
              className={`flex w-full items-center gap-2 box-border rounded-[var(--thread-card-radius,0px)] px-2 py-1.5 text-left text-[11px] font-medium border transition-colors ${
                selected
                  ? 'border-[var(--thread-card-accent,var(--color-accent))] bg-[var(--thread-card-accent,var(--color-accent))]/15 text-[var(--thread-card-text,var(--color-text-primary))]'
                  : pending
                    ? 'border-[var(--thread-card-border,var(--color-border))] text-[var(--thread-card-text-secondary,var(--color-text-secondary))] hover:border-[var(--thread-card-accent,var(--color-accent))] hover:text-[var(--thread-card-text,var(--color-text-primary))] cursor-pointer'
                    : 'border-[var(--thread-card-border,var(--color-border))] text-[var(--thread-card-muted,var(--color-text-muted))] opacity-50'
              } disabled:cursor-not-allowed`}
            >
              <span
                className={`w-4 shrink-0 text-[10px] font-semibold ${
                  selected
                    ? 'text-[var(--thread-card-accent,var(--color-accent))]'
                    : 'text-[var(--thread-card-muted,var(--color-text-muted))]'
                }`}
              >
                {letter}
              </span>
              <span className="min-w-0 flex-1">{opt.label}</span>
            </button>
          )
      })}
      {expiry && (
        <div
          data-testid="thread-choice-expiry"
          className="text-[10px] text-[var(--thread-card-muted,var(--color-text-muted))]"
        >
          {expiry}
        </div>
      )}
      {status === 'answered' && answeredBy && (
        <div
          data-testid="thread-choice-answered-by"
          className="text-[10px] text-[var(--thread-card-muted,var(--color-text-muted))]"
        >
          answered by {answeredBy}
        </div>
      )}
      {allowCustom && pending && (
        <div className="mt-0.5 flex gap-2">
          <input
            data-testid="thread-choice-custom"
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            placeholder="Custom…"
            className="flex-1 bg-transparent text-[12px] text-[var(--thread-card-text,var(--color-text-primary))] outline-none border border-[var(--thread-card-border,var(--color-border))] px-2 py-1"
          />
          <button
            type="button"
            data-testid="thread-choice-custom-submit"
            disabled={!custom.trim()}
            onClick={() => onPick(custom.trim())}
            className="px-2 py-1 text-[11px] border border-[var(--thread-card-border,var(--color-border))]"
          >
            Send
          </button>
        </div>
      )}
    </div>
  )
}
