// The vault confirm a person gives before a secret is bound to a source
// (contract-sources-v1 §4.3, §8) [GV43]. Owner: B2.
//
// `vault/confirm/start` names the methods this caller has: on a Mac desktop
// the Mac's own sheet (`local-auth`); over a Connect login the login's
// password; otherwise the vault password. The token is bound to the action
// (`source-secret` / `source-batch`) and to the id (the source id, or the
// batch hash), works once, within 60 seconds. The password lives in the
// field until submit, then it's gone.

import React, { useEffect, useRef, useState } from 'react'
import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { btn, input, muted, danger, sourcesErrorText } from './data-sources-ui'

type Step = { step: 'starting' } | { step: 'mac' } | { step: 'password'; method: string } | { step: 'failed'; text: string }

export interface ConfirmStepProps {
  scope: ServerScope
  action: 'source-secret' | 'source-batch'
  /** The source id, or the batch hash. */
  secretId: string
  /** Shown in the Mac's sheet ("source secret deploys"). */
  label: string
  onToken: (token: string) => void
  onCancel: () => void
}

/** Starts on mount; calls `onToken` with a fresh confirm token. */
export function ConfirmStep({ scope, action, secretId, label, onToken, onCancel }: ConfirmStepProps): React.JSX.Element {
  const [state, setState] = useState<Step>({ step: 'starting' })
  const [hasPw, setHasPw] = useState(false)
  const pwRef = useRef<HTMLInputElement>(null)
  const started = useRef(false)

  const finish = async (method: string, password?: string): Promise<void> => {
    const r = await daemonCliPost<{ token: string }>(scope, 'vault/confirm/finish', {
      action,
      secretId,
      method,
      label,
      ...(password !== undefined ? { password } : {}),
    })
    onToken(r.token)
  }

  useEffect(() => {
    if (started.current) return
    started.current = true
    void (async () => {
      try {
        const { methods } = await daemonCliPost<{ methods: string[] }>(scope, 'vault/confirm/start', { action, secretId })
        if (methods.includes('local-auth')) {
          setState({ step: 'mac' })
          await finish('local-auth')
          return
        }
        const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
        if (pw) {
          setState({ step: 'password', method: pw })
          return
        }
        setState({
          step: 'failed',
          text: 'No way to confirm here. Bind from Settings over your Connect login, or set a vault password.',
        })
      } catch (e) {
        setState({ step: 'failed', text: sourcesErrorText(e) })
      }
    })()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  const submit = async (method: string): Promise<void> => {
    const el = pwRef.current
    if (!el) return
    const pw = el.value
    el.value = ''
    setHasPw(false)
    try {
      await finish(method, pw)
    } catch (e) {
      setState({ step: 'failed', text: sourcesErrorText(e) })
    }
  }

  return (
    <div className="space-y-1.5" data-testid="sources-confirm">
      {state.step === 'starting' && <p className={`text-[11px] ${muted}`}>Asking the vault…</p>}
      {state.step === 'mac' && <p className={`text-[11px] ${muted}`}>Confirm on your Mac.</p>}
      {state.step === 'password' && (
        <form
          className="flex flex-col gap-1.5 max-w-sm"
          onSubmit={(e) => {
            e.preventDefault()
            void submit(state.method)
          }}
        >
          <label className={`text-[11px] ${muted}`}>
            {state.method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
            <input
              ref={pwRef}
              type="password"
              autoComplete="current-password"
              data-testid="sources-confirm-password"
              onChange={(e) => setHasPw(e.currentTarget.value !== '')}
              className={`${input} mt-1`}
            />
          </label>
          <div className="flex gap-1.5">
            <button type="submit" className={btn} disabled={!hasPw}>
              Confirm
            </button>
            <button type="button" className={btn} onClick={onCancel}>
              Cancel
            </button>
          </div>
        </form>
      )}
      {state.step === 'failed' && (
        <div className="space-y-1">
          <p className={`text-[11px] ${danger}`} role="alert">
            {state.text}
          </p>
          <button type="button" className={btn} onClick={onCancel}>
            Back
          </button>
        </div>
      )}
    </div>
  )
}
