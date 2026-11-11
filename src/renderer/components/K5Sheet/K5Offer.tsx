// The K5 offer in the LLM sign-in sheet's success state (prd-setup-checklist-v1
// §10.1): once a tool is signed in, the daemon may say `offer: "meet_k5"`
// (the K5 switch is on, there's no K5 yet, the caller may make one, and this
// person hasn't said Not now). Meet K5 opens the K5 sheet; Not now is kept
// per person by the daemon. Skippable, never blocking.
//
// The window's server only, like the checklist and the K5 sheet.

import { useEffect } from 'react'
import { Button } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { useFirstRunStore, watchFirstRun } from '@/stores/first-run'
import { openK5Sheet } from '@/stores/k5-sheet'

/** The contract's copy (purpose only). */
export const K5_OFFER_COPY = {
  title: "Meet K5, K2's own agent",
  line: 'K5 runs K2 for you: setup, settings and your agents. It will help you hire your own Chief of Staff.',
  meet: 'Meet K5',
  notNow: 'Not now',
} as const

/** `onMeet`: the sign-in sheet closes as the K5 sheet opens. */
export function K5Offer({ scope, onMeet }: { scope: ServerScope; onMeet?: () => void }): React.JSX.Element | null {
  const primary = scope.isPrimary
  // The sign-in just landed: read the status now and on its events (the
  // daemon's `llm_accounts_changed` / `first_run_changed`).
  useEffect(() => (primary ? watchFirstRun(scope) : undefined), [primary, scope])
  const offer = useFirstRunStore((s) => (primary ? s.entries[scope.id]?.doc?.offer ?? null : null))
  if (offer !== 'meet_k5') return null
  return (
    <div className="border border-[var(--color-border)] px-3 py-2 space-y-2" data-testid="k5-offer">
      <div className="type-row text-[var(--color-text-primary)]">{K5_OFFER_COPY.title}</div>
      <div className="type-hint text-[var(--color-text-muted)] leading-relaxed">{K5_OFFER_COPY.line}</div>
      <div className="flex gap-2">
        <Button
          variant="accent"
          size="md"
          data-testid="k5-offer-meet"
          onClick={() => {
            openK5Sheet()
            onMeet?.()
          }}
        >
          {K5_OFFER_COPY.meet}
        </Button>
        <Button
          variant="ghost"
          size="md"
          data-testid="k5-offer-dismiss"
          onClick={() => void useFirstRunStore.getState().dismissOffer(scope)}
        >
          {K5_OFFER_COPY.notNow}
        </Button>
      </div>
    </div>
  )
}
