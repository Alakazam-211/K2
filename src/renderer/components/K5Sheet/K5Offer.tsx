// The K5 offer in the LLM sign-in sheet's success state (prd-setup-checklist-v1
// §10.1): once a tool is signed in, the daemon may say `offer: "meet_k5"`
// (the K5 switch is on, there's no K5 yet, the caller may make one, and this
// person hasn't said Not now). Meet K5 opens the K5 sheet; Not now is kept
// per person by the daemon. Skippable, never blocking.
//
// Look: a tile inside the sheet (the sheet's own tile material, cut corners
// in Terminal), the agents mark beside its title (set in the dialogs'
// heading role, so it leads the tile), the line under it in the dialogs'
// muted words, and its two answers, the main one first.
//
// The window's server only, like the checklist and the K5 sheet.

import { useEffect, useId, useState } from 'react'
import { Button, Surface } from '@/components/ui'
import { AgentsIcon } from '@/components/icons/K2Icons'
import { DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'
import type { ServerScope } from '@/kessel/server-scope'
import { useFirstRunStore, watchFirstRun } from '@/stores/first-run'
import { openK5Sheet } from '@/stores/k5-sheet'

/** The contract's copy (purpose only). */
export const K5_OFFER_COPY = {
  title: 'Meet K5, K2’s own agent',
  line: 'K5 helps you set up K2, change its settings and look after your agents. First, it helps you hire your Chief of Staff, the lead agent for your own work.',
  meet: 'Meet K5',
  notNow: 'Not now',
} as const

/** `onMeet`: the sign-in sheet closes as the K5 sheet opens. */
export function K5Offer({ scope, onMeet }: { scope: ServerScope; onMeet?: () => void }): React.JSX.Element | null {
  const primary = scope.isPrimary
  const titleId = useId()
  const lineId = useId()
  const [dismissing, setDismissing] = useState(false)
  // The sign-in just landed: read the status now and on its events (the
  // daemon's `llm_accounts_changed` / `first_run_changed`).
  useEffect(() => (primary ? watchFirstRun(scope) : undefined), [primary, scope])
  const offer = useFirstRunStore((s) => (primary ? s.entries[scope.id]?.doc?.offer ?? null : null))
  if (offer !== 'meet_k5') return null
  return (
    <Surface
      chamfer
      material="tile"
      role="group"
      aria-labelledby={titleId}
      aria-describedby={lineId}
      className="px-4 py-3 space-y-3"
      data-testid="k5-offer"
    >
      <div className="flex items-start gap-2.5">
        <AgentsIcon size={16} className="mt-px flex-shrink-0 text-[var(--color-accent-text)]" />
        <div className="min-w-0">
          <p id={titleId} className="type-heading text-[var(--color-text-primary)]">
            {K5_OFFER_COPY.title}
          </p>
          <p id={lineId} className={`${DIALOG_MUTED} mt-1`}>
            {K5_OFFER_COPY.line}
          </p>
        </div>
      </div>
      <div className="flex gap-2 pl-[26px]">
        <Button
          variant="accent"
          size="md"
          data-testid="k5-offer-meet"
          disabled={dismissing}
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
          disabled={dismissing}
          onClick={() => {
            setDismissing(true)
            void useFirstRunStore
              .getState()
              .dismissOffer(scope)
              .finally(() => setDismissing(false))
          }}
        >
          {K5_OFFER_COPY.notNow}
        </Button>
      </div>
    </Surface>
  )
}
