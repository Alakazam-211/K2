// Settings → Apps → This server → an App row → "Hand over…" (prd-app-
// handoff-v1 §4.13, `k2 app handover`, 0.46.5). The owner picks the agent
// that takes the App, sees exactly what moves (the daemon's plan, as the
// card's lines), picks the options the plan asks for, and hands it over:
// that files the handover card and approves it in the same flow with a
// fresh vault confirm (`app-approve`, bound to the card). Nothing moves
// until the confirm.
//
// Thin client: the daemon computes the plan, checks who may, refuses what
// it can't move and applies everything; this sheet shows its truth and
// sends the owner's gestures.

import React, { useRef, useState } from 'react'
import { Button, Callout } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { fetchHandoverPlan, requestHandover, type HandoverOptions, type HandoverPlan } from './app-access-api'
import { APP_APPROVE_CONFIRM_ACTION, approveAppRequest } from './app-approvals-api'
import { vaultApi } from './vault-api'

const muted = 'text-[var(--color-text-muted)]'

type Step =
  | { kind: 'pick' }
  | { kind: 'planning' }
  | { kind: 'plan'; plan: HandoverPlan }
  | { kind: 'confirming'; request: string }
  | { kind: 'password'; request: string; method: string }

/** File a card's confirm and approve it: Touch ID / login on a Mac, else
 *  the password the daemon names. `password` answers a `password` step. */
export async function approveWithConfirm(request: string, label: string, method: string, password?: string): Promise<void> {
  const { token } = await vaultApi.confirmFinish({
    action: APP_APPROVE_CONFIRM_ACTION,
    secretId: request,
    method,
    label: label.slice(0, 64),
    ...(password !== undefined ? { password } : {}),
  })
  const out = await approveAppRequest(request, token)
  if (out.error) {
    throw new Error(JSON.stringify({ error: { code: out.error.code, hint: out.error.hint || out.error.code } }))
  }
}

export function AppHandoverSheet({
  appId,
  parked,
  currentWorkspace,
  agents,
  onDone,
  onClose,
}: {
  appId: string
  parked: boolean
  currentWorkspace: string | null
  agents: ProjectWithWorkspaces[]
  onDone: (text: string) => Promise<void>
  onClose: () => void
}): React.JSX.Element {
  const [to, setTo] = useState('')
  const [step, setStep] = useState<Step>({ kind: 'pick' })
  const [opts, setOpts] = useState<HandoverOptions>({})
  const [error, setError] = useState<string | null>(null)
  const pwRef = useRef<HTMLInputElement>(null)
  const choices = agents.filter((p) => p.id !== currentWorkspace)
  const label = `hand over ${appId}`

  /** Ask the daemon what moves. A plan already shown stays up while the
   *  new one loads (an option changed); `keepError` keeps the last error. */
  const loadPlan = async (target: string, o: HandoverOptions, keepError = false): Promise<void> => {
    if (!keepError) setError(null)
    setStep((cur) => (cur.kind === 'plan' ? cur : { kind: 'planning' }))
    try {
      setStep({ kind: 'plan', plan: await fetchHandoverPlan(appId, target, o) })
    } catch (e) {
      setStep({ kind: 'pick' })
      setError(daemonErrorText(e))
    }
  }

  const setOpt = (o: HandoverOptions): void => {
    setOpts(o)
    if (to) void loadPlan(to, o)
  }

  const handOver = async (): Promise<void> => {
    setError(null)
    try {
      const { request } = await requestHandover(appId, to, opts, parked)
      setStep({ kind: 'confirming', request })
      const { methods } = await vaultApi.confirmStart(APP_APPROVE_CONFIRM_ACTION, request)
      if (methods.includes('local-auth')) {
        await approveWithConfirm(request, label, 'local-auth')
        await onDone(`${appId} is handed over. You can undo it for 72 hours.`)
        return
      }
      const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
      if (pw) {
        setStep({ kind: 'password', request, method: pw })
        return
      }
      setError('No way to confirm on this server: set a vault password (k2 vault password set). The card waits in App Approvals.')
    } catch (e) {
      setError(daemonErrorText(e))
      if (to) await loadPlan(to, opts, true)
    }
  }

  const submitPassword = async (request: string, method: string): Promise<void> => {
    const pw = pwRef.current?.value ?? ''
    if (pwRef.current) pwRef.current.value = ''
    setError(null)
    try {
      await approveWithConfirm(request, label, method, pw)
      await onDone(`${appId} is handed over. You can undo it for 72 hours.`)
    } catch (e) {
      setError(daemonErrorText(e))
    }
  }

  const plan = step.kind === 'plan' ? step.plan : null
  return (
    <div className="pl-9 space-y-2" data-testid={`apps-handover-${appId}`}>
      <div className="flex items-center gap-2 flex-wrap">
        <select
          aria-label={`Agent for ${appId}`}
          className="px-2 py-1 text-[11px] bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] no-drag"
          value={to}
          onChange={(e) => {
            setTo(e.target.value)
            if (e.target.value) void loadPlan(e.target.value, opts)
            else setStep({ kind: 'pick' })
          }}
        >
          <option value="">Hand it to…</option>
          {choices.map((p) => (
            <option key={p.id} value={p.id}>
              {p.name}
            </option>
          ))}
        </select>
        <Button variant="ghost" onClick={onClose}>
          Close
        </Button>
      </div>
      {step.kind === 'planning' ? <p className={`text-[11px] ${muted}`}>Working out what moves…</p> : null}
      {plan ? (
        <div className="space-y-2" data-testid={`apps-handover-${appId}-plan`}>
          <ul className="space-y-1 text-[11px] text-[var(--color-text-secondary)] list-disc pl-4">
            {plan.lines.map((l, i) => (
              <li key={i}>{l}</li>
            ))}
          </ul>
          {plan.blockers.map((b) => (
            <Callout key={b.code} tone="error" data-testid={`apps-handover-${appId}-blocker`}>
              {b.hint}
            </Callout>
          ))}
          {plan.databaseCase === 'shared' ? (
            <label className="flex items-center gap-2 text-[11px]">
              <input type="checkbox" checked={opts.db === 'share'} onChange={(e) => setOpt({ ...opts, db: e.target.checked ? 'share' : undefined })} />
              Share the database (the old agent keeps access to this App’s data)
            </label>
          ) : null}
          {plan.collisions ? (
            <label className="flex items-center gap-2 text-[11px]">
              <input
                type="checkbox"
                checked={opts.collisions === 'use_b'}
                onChange={(e) => setOpt({ ...opts, collisions: e.target.checked ? 'use_b' : undefined })}
              />
              Use the new agent’s own values for secrets it already has (else the App keeps its own, renamed)
            </label>
          ) : null}
          {plan.sharedGuests > 0 ? (
            <label className="flex items-center gap-2 text-[11px]">
              <input type="checkbox" checked={opts.resetShared === true} onChange={(e) => setOpt({ ...opts, resetShared: e.target.checked || undefined })} />
              Take over the guests who also use the old agent’s other Apps (they keep this App and set a new password)
            </label>
          ) : null}
          <Button disabled={!plan.ready} onClick={() => void handOver()}>
            {parked ? 'Unpark' : 'Hand over'}
          </Button>
        </div>
      ) : null}
      {step.kind === 'confirming' ? <p className={`text-[11px] ${muted}`}>Confirm it’s you…</p> : null}
      {step.kind === 'password' ? (
        <div className="flex items-center gap-2">
          <input
            ref={pwRef}
            type="password"
            aria-label="Password to confirm"
            className="px-2 py-1 text-[11px] bg-[var(--color-bg-surface)] border border-[var(--color-border)]"
          />
          <Button onClick={() => void submitPassword(step.request, step.method)}>Confirm</Button>
        </div>
      ) : null}
      {error ? (
        <Callout tone="error" data-testid={`apps-handover-${appId}-error`}>
          {error}
        </Callout>
      ) : null}
    </div>
  )
}
