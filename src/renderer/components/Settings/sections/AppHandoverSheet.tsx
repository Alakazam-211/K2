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
import { Button, Callout, Checkbox, Input } from '@/components/ui'
import { daemonErrorText } from '@/lib/daemon-error'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { fetchHandoverPlan, requestHandover, type HandoverOptions, type HandoverPlan } from './app-access-api'
import { APP_APPROVE_CONFIRM_ACTION, approveAppRequest } from './app-approvals-api'
import { vaultApi } from './vault-api'
import { SettingDropdown } from '../controls/SettingControls'
import { AccessSheet, DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED, Field } from './AccessDialogs'
import { PRODUCT_NAME } from '@/lib/brand'

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
      setError('there is no way to confirm it’s you on this server yet. Set a vault password on the Vault page; the request waits in App Approvals')
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
  const verb = parked ? 'Unpark' : 'Hand over'
  const working = step.kind === 'confirming'
  return (
    <AccessSheet
      title={`${verb} ${appId}`}
      intro={
        parked
          ? `Pick the agent that keeps it up from now on. ${PRODUCT_NAME} shows what moves before anything does.`
          : `Pick the agent that takes it over. ${PRODUCT_NAME} shows what moves before anything does, and you can undo it for 72 hours.`
      }
      onClose={onClose}
      busy={working}
      testId={`apps-handover-${appId}`}
      footer={
        <>
          <Button variant="ghost" size="md" onClick={onClose} disabled={working}>
            Cancel
          </Button>
          {step.kind === 'password' ? (
            <Button variant="accent" size="md" onClick={() => void submitPassword(step.request, step.method)}>
              Confirm
            </Button>
          ) : (
            <Button variant="accent" size="md" disabled={!plan || !plan.ready || working} onClick={() => void handOver()}>
              {working ? 'Confirming…' : verb}
            </Button>
          )}
        </>
      }
    >
      <Field label="Agent" htmlFor={`handover-to-${appId}`}>
        <SettingDropdown
          fullWidth
          menuAlign="left"
          ariaLabel={`Agent for ${appId}`}
          placeholder="Pick an agent"
          value={to}
          options={choices.map((p) => ({ value: p.id, label: p.name }))}
          onChange={(v) => {
            // A re-pick of the same agent keeps the plan up (no reload).
            if (v === to) return
            setTo(v)
            void loadPlan(v, opts)
          }}
        />
      </Field>
      {step.kind === 'planning' ? <p className={DIALOG_MUTED}>Working out what moves…</p> : null}
      {plan ? (
        <div className="space-y-3" data-testid={`apps-handover-${appId}-plan`}>
          <div>
            <p className={DIALOG_LABEL}>What moves</p>
            <ul className="space-y-1 type-hint text-[var(--color-text-secondary)] list-disc pl-5 leading-relaxed">
              {plan.lines.map((l, i) => (
                <li key={i}>{l}</li>
              ))}
            </ul>
          </div>
          {plan.blockers.map((b) => (
            <Callout key={b.code} tone="error" className="type-hint" data-testid={`apps-handover-${appId}-blocker`}>
              {b.hint}
            </Callout>
          ))}
          {plan.databaseCase === 'shared' ? (
            <label className="flex items-start gap-2 type-hint text-[var(--color-text-primary)] cursor-pointer">
              <Checkbox checked={opts.db === 'share'} onChange={(e) => setOpt({ ...opts, db: e.target.checked ? 'share' : undefined })} />
              Share the database (the old agent keeps access to this App’s data)
            </label>
          ) : null}
          {plan.collisions ? (
            <label className="flex items-start gap-2 type-hint text-[var(--color-text-primary)] cursor-pointer">
              <Checkbox
                checked={opts.collisions === 'use_b'}
                onChange={(e) => setOpt({ ...opts, collisions: e.target.checked ? 'use_b' : undefined })}
              />
              Use the new agent’s own values for secrets it already has (else the App keeps its own, renamed)
            </label>
          ) : null}
          {plan.sharedGuests > 0 ? (
            <label className="flex items-start gap-2 type-hint text-[var(--color-text-primary)] cursor-pointer">
              <Checkbox checked={opts.resetShared === true} onChange={(e) => setOpt({ ...opts, resetShared: e.target.checked || undefined })} />
              Take over the guests who also use the old agent’s other Apps (they keep this App and set a new password)
            </label>
          ) : null}
        </div>
      ) : null}
      {step.kind === 'confirming' ? <p className={DIALOG_MUTED}>Confirm it’s you…</p> : null}
      {step.kind === 'password' ? (
        <Field label="Your password" htmlFor={`handover-pw-${appId}`} hint={`${PRODUCT_NAME} asks for it to confirm it’s you.`}>
          <Input id={`handover-pw-${appId}`} ref={pwRef} type="password" aria-label="Password to confirm" className={DIALOG_FIELD} />
        </Field>
      ) : null}
      {error ? (
        <Callout tone="error" role="alert" className="type-hint" data-testid={`apps-handover-${appId}-error`}>
          Couldn’t {verb.toLowerCase()} {appId}. Nothing moved. The server said: {error.trim().replace(/[.\s]+$/, '')}.
        </Callout>
      ) : null}
    </AccessSheet>
  )
}
