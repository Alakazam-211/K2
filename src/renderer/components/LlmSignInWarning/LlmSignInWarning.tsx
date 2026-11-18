// Before a Claude sign-in reaches its time limit (7 days, 1 day and 2
// hours before, and once it has ended), the server's owners see one
// pop-up per token and step: Sign in now starts the sign-in for that
// token, Later puts this step away for this person. The daemon owns which
// warnings are due and who put what away (`llm/accounts/warnings`, owner
// logins only); this only shows them, on start and when the daemon says
// the tokens changed.

import React, { useCallback, useEffect, useState } from 'react'
import { primaryScope } from '@/kessel/server-scope'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { onLlmAccountsChanged } from '@/stores/session-events'
import { Button } from '@/components/ui'
import { AccessSheet, DIALOG_MUTED } from '@/components/Settings/sections/AccessDialogs'
import { LlmLoginSheet, type LoginSheetStart } from '@/components/Settings/shared/LlmLoginSheet'

export interface SignInWarning {
  accountId: string
  tool: string
  label: string
  /** `7d`, `1d`, `2h` or `ended`. */
  step: string
  /** Unix seconds, when known. */
  endsAt: number | null
  epoch: number
}

/** `Nov 20, 2026, 2:03 PM` in this computer's time. */
export function whenText(secs: number): string {
  return new Date(secs * 1000).toLocaleString(undefined, { dateStyle: 'medium', timeStyle: 'short' })
}

/** What the pop-up says. */
export function warningText(w: SignInWarning, nowMs: number = Date.now()): { title: string; message: string } {
  const name = `Claude login ${w.label}`
  if (w.step === 'ended') {
    const when = w.endsAt !== null && w.endsAt * 1000 <= nowMs ? ` on ${whenText(w.endsAt)}` : ''
    return {
      title: `${name} has ended`,
      message: `${name} has ended${when}. Agents using it stopped. Sign in again to start them.`,
    }
  }
  const by = w.endsAt !== null ? whenText(w.endsAt) : 'soon'
  return {
    title: `${name} needs a fresh sign-in`,
    message: `${name} needs a fresh sign-in by ${by}. After that, agents using it stop.`,
  }
}

function parse(raw: unknown): SignInWarning[] {
  const o = (raw && typeof raw === 'object' ? raw : {}) as { owner?: unknown; warnings?: unknown }
  if (o.owner !== true || !Array.isArray(o.warnings)) return []
  return (o.warnings as Array<Record<string, unknown>>)
    .filter((w) => typeof w.accountId === 'string' && typeof w.step === 'string' && typeof w.epoch === 'number')
    .map((w) => ({
      accountId: w.accountId as string,
      tool: typeof w.tool === 'string' ? w.tool : 'claude',
      label: typeof w.label === 'string' ? w.label : '',
      step: w.step as string,
      endsAt: typeof w.endsAt === 'number' ? w.endsAt : null,
      epoch: w.epoch as number,
    }))
}

export function LlmSignInWarning(): React.JSX.Element | null {
  const scope = primaryScope()
  const [warnings, setWarnings] = useState<SignInWarning[]>([])
  const [signIn, setSignIn] = useState<LoginSheetStart | null>(null)

  const read = useCallback(() => {
    daemonCliGet(scope, 'llm/accounts/warnings')
      .then((raw) => setWarnings(parse(raw)))
      .catch((e: unknown) => console.debug('[llm-sign-in-warning] read:', e))
  }, [scope])

  useEffect(() => {
    read()
    return onLlmAccountsChanged(scope, read)
  }, [scope, read])

  const putAway = useCallback(
    (w: SignInWarning) => {
      setWarnings((ws) => ws.filter((x) => !(x.accountId === w.accountId && x.step === w.step && x.epoch === w.epoch)))
      daemonCliPost(scope, 'llm/accounts/warnings/dismiss', { id: w.accountId, step: w.step, epoch: w.epoch }).catch((e: unknown) =>
        console.debug('[llm-sign-in-warning] dismiss:', e),
      )
    },
    [scope],
  )

  if (signIn) {
    return <LlmLoginSheet scope={scope} start={signIn} onClose={() => setSignIn(null)} />
  }
  const w = warnings[0]
  if (!w) return null
  const { title, message } = warningText(w)
  return (
    <AccessSheet
      title={title}
      onClose={() => putAway(w)}
      testId="llm-sign-in-warning"
      footer={
        <>
          <Button variant="ghost" size="md" onClick={() => putAway(w)}>
            Later
          </Button>
          <Button
            variant="accent"
            size="md"
            autoFocus
            onClick={() => {
              putAway(w)
              setSignIn({ kind: 'relogin', tool: w.tool, toolLabel: 'Claude', accountId: w.accountId, label: w.label })
            }}
          >
            Sign in now
          </Button>
        </>
      }
    >
      <p className={DIALOG_MUTED}>{message}</p>
    </AccessSheet>
  )
}

export default LlmSignInWarning
