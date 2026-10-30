// K2's own PIN form (PV-7, PV-12). It is never a text field in a site list, and
// it posts the PIN straight to the route as this person. Nothing here keeps it,
// and it never goes through a card, the Thread or an agent.

import { useState } from 'react'
import { setSiteVisibility, type SiteVisibility } from '@/lib/published-sites'
import type { ServerScope } from '@/kessel/server-scope'

const PIN_MIN = 6
const PIN_MAX = 64
const PIN_WARN_BELOW = 8

/** What K2 says about a dev server when it is shared (DM-3). */
export const DEV_SHARE_WARNING =
  'This is a dev server. The person with the PIN can see source maps, error pages with file paths, and anything the dev server evaluates.'

const fieldCls =
  'w-full px-2 py-1 text-[11px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag'
const buttonCls =
  'px-2 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-50 no-drag'

/** Why these two entries can't be a PIN yet, or null. */
export function pinProblem(pin: string, again: string): string | null {
  if (pin.length < PIN_MIN) return `A PIN needs at least ${PIN_MIN} characters.`
  if (pin.length > PIN_MAX) return `A PIN can have at most ${PIN_MAX} characters.`
  if (pin !== again) return "The two PINs don't match."
  return null
}

export function SitePinForm({
  scope,
  label,
  dev = false,
  onDone,
  onCancel,
}: {
  scope: ServerScope
  label: string
  dev?: boolean
  onDone: (v: SiteVisibility) => void
  onCancel: () => void
}): React.JSX.Element {
  const [pin, setPin] = useState('')
  const [again, setAgain] = useState('')
  const [needsYes, setNeedsYes] = useState<string | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const problem = pinProblem(pin, again)

  const submit = async (yes: boolean): Promise<void> => {
    if (problem) {
      setError(problem)
      return
    }
    setBusy(true)
    setError(null)
    try {
      const v = await setSiteVisibility(scope, label, { mode: 'pin', pin, ...(yes ? { yes: true } : {}) })
      setPin('')
      setAgain('')
      onDone(v)
    } catch (e) {
      const text = e instanceof Error ? e.message : String(e)
      if (text.includes('--yes')) {
        setNeedsYes(text.replace(/ Pass --yes to go ahead\.?/, ''))
      } else {
        setError(text)
      }
    } finally {
      setBusy(false)
    }
  }

  return (
    <form
      data-testid="site-pin-form"
      className="flex flex-col gap-1.5"
      onSubmit={(e) => {
        e.preventDefault()
        void submit(needsYes !== null)
      }}
    >
      <div className="text-[11px] text-[var(--color-text-primary)]">Set a PIN for {label}</div>
      {dev ? (
        <div data-testid="site-pin-dev-warning" className="text-[10px] text-[var(--color-status-warn-text)]">
          {DEV_SHARE_WARNING}
        </div>
      ) : null}
      <input
        data-testid="site-pin-input"
        aria-label="PIN"
        type="password"
        autoComplete="new-password"
        value={pin}
        maxLength={PIN_MAX}
        onChange={(e) => setPin(e.target.value)}
        placeholder={`PIN (${PIN_MIN} to ${PIN_MAX} characters)`}
        className={fieldCls}
      />
      <input
        data-testid="site-pin-again"
        aria-label="The same PIN again"
        type="password"
        autoComplete="new-password"
        value={again}
        maxLength={PIN_MAX}
        onChange={(e) => setAgain(e.target.value)}
        placeholder="The same PIN again"
        className={fieldCls}
      />
      {pin.length >= PIN_MIN && pin.length < PIN_WARN_BELOW ? (
        <div data-testid="site-pin-short" className="text-[10px] text-[var(--color-status-warn-text)]">
          Under {PIN_WARN_BELOW} characters is easier to guess.
        </div>
      ) : null}
      {needsYes ? (
        <div data-testid="site-pin-needs-yes" className="text-[10px] text-[var(--color-status-warn-text)]">
          {needsYes}
        </div>
      ) : null}
      {error ? (
        <div data-testid="site-pin-error" className="text-[10px] text-[var(--color-status-error)]">
          {error}
        </div>
      ) : null}
      <div className="text-[10px] text-[var(--color-text-muted)]">Send the address and the PIN separately.</div>
      <div className="flex gap-1.5">
        <button type="submit" data-testid="site-pin-save" disabled={busy || !!problem} className={buttonCls}>
          {needsYes ? 'Go ahead' : 'Set PIN'}
        </button>
        <button type="button" data-testid="site-pin-cancel" disabled={busy} onClick={onCancel} className={buttonCls}>
          Cancel
        </button>
      </div>
    </form>
  )
}
