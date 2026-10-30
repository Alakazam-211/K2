// Small parts local to Settings → Chat Hosting (Sew S32 K27): the status
// chip, a type-back confirm, the shown-once secret box, and the person
// confirm (vault) for the person-only steps. Kept local on purpose: the mail
// page has its own copies and S32 does not refactor it.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import { Button, SquareCheckbox } from '@/components/ui'
import { finishSewConfirm, sewStateChip, startSewConfirm, type SewConfirmAction } from './sew-api'

export const muted = 'text-[var(--color-text-muted)]'
export const secondary = 'text-[var(--color-text-secondary)]'

export function SewStatusChip({ state }: { state: string }): React.JSX.Element {
  const { label, color } = sewStateChip(state)
  return (
    <span
      data-testid="sew-state-chip"
      className="text-[10px] font-semibold px-1.5 py-0.5 uppercase tracking-wide"
      style={{ color, backgroundColor: `color-mix(in srgb, ${color} 14%, transparent)` }}
    >
      {label}
    </span>
  )
}

/** The button stays disabled until the typed text equals `expect` exactly
 *  (trimmed, case-sensitive). */
export function TypeToConfirm({
  expect,
  value,
  onChange,
  disabled,
  label,
}: {
  expect: string
  value: string
  onChange: (v: string) => void
  disabled?: boolean
  label: string
}): React.JSX.Element {
  return (
    <label className={`block text-[11px] ${muted}`}>
      {label}
      <input
        aria-label={label}
        value={value}
        disabled={disabled}
        placeholder={expect}
        onChange={(e) => onChange(e.target.value)}
        className="mt-1 w-full font-mono border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs text-[var(--color-text-primary)]"
      />
    </label>
  )
}

export function typedMatches(typed: string, expect: string): boolean {
  return expect !== '' && typed.trim() === expect
}

const ONCE_SECRET_MS = 5 * 60 * 1000

/** Shown-once secret: the value lives in the PARENT's component state only
 *  and the parent drops it on `onDone`. Done needs the checkbox unless
 *  `needsAck` is false (a secret K2 already keeps, like Sew's recovery key
 *  in the vault). The box clears itself after 5 minutes. Never logged,
 *  toasted or stored. */
export function OnceSecret({
  heading,
  secret,
  warning,
  onDone,
  needsAck = true,
}: {
  heading: string
  secret: string
  warning: string
  onDone: () => void
  needsAck?: boolean
}): React.JSX.Element {
  const [stored, setStored] = useState(!needsAck)
  const [copied, setCopied] = useState(false)
  const doneRef = useRef(onDone)
  doneRef.current = onDone
  useEffect(() => {
    const t = window.setTimeout(() => doneRef.current(), ONCE_SECRET_MS)
    return () => window.clearTimeout(t)
  }, [])
  return (
    <div role="dialog" aria-label={heading} className="border border-[var(--color-border)] p-3 space-y-2" data-testid="sew-once-secret">
      <p className="text-xs font-medium text-[var(--color-text-primary)]">{heading}</p>
      <p className={`text-[11px] ${secondary}`}>{warning}</p>
      <div className="flex items-center gap-2">
        <code data-testid="sew-once-secret-value" className="select-all font-mono text-xs break-all text-[var(--color-text-primary)]">
          {secret}
        </code>
        <Button
          onClick={() => {
            void navigator.clipboard?.writeText(secret).then(
              () => setCopied(true),
              () => setCopied(false),
            )
          }}
        >
          {copied ? 'Copied' : 'Copy'}
        </Button>
      </div>
      {needsAck ? (
        <label className={`flex items-center gap-2 text-[11px] ${secondary}`}>
          <SquareCheckbox checked={stored} onChange={(e) => setStored(e.target.checked)} aria-label="I stored it off this machine" />
          I stored it off this machine
        </label>
      ) : null}
      <Button variant="accent" disabled={!stored} onClick={onDone}>
        Done
      </Button>
    </div>
  )
}

type ConfirmStep =
  | { step: 'idle' }
  | { step: 'starting' }
  | { step: 'mac' }
  | { step: 'password'; method: string }

interface Pending {
  action: SewConfirmAction
  secretId: string | undefined
  label: string
  resolve: (token: string | null) => void
  reject: (e: unknown) => void
}

/** A person confirm for one step (vault `confirm/start` + `finish`):
 *  Touch ID / the Mac login sheet, else the Connect or vault password.
 *  `confirm()` resolves the one-use 60 s token, or null when cancelled. */
export function usePersonConfirm(): {
  confirm: (action: SewConfirmAction, secretId: string | undefined, label: string) => Promise<string | null>
  ui: React.JSX.Element | null
} {
  const [state, setState] = useState<ConfirmStep>({ step: 'idle' })
  const [hasPw, setHasPw] = useState(false)
  const pending = useRef<Pending | null>(null)
  const pwRef = useRef<HTMLInputElement>(null)

  useEffect(
    () => () => {
      pending.current?.resolve(null)
      pending.current = null
    },
    [],
  )

  const confirm = useCallback(
    (action: SewConfirmAction, secretId: string | undefined, label: string): Promise<string | null> =>
      new Promise<string | null>((resolve, reject) => {
        pending.current?.resolve(null)
        pending.current = { action, secretId, label, resolve, reject }
        setState({ step: 'starting' })
        void (async () => {
          try {
            const { methods } = await startSewConfirm(action, secretId)
            if (methods.includes('local-auth')) {
              setState({ step: 'mac' })
              const { token } = await finishSewConfirm(action, secretId, 'local-auth', label)
              setState({ step: 'idle' })
              pending.current = null
              resolve(token)
              return
            }
            const pw = methods.find((m) => m === 'connect-password' || m === 'vault-password')
            if (pw) {
              setState({ step: 'password', method: pw })
              return
            }
            throw new Error('No way to confirm here yet. Set a vault password from a terminal outside K2: k2 vault confirm-password set.')
          } catch (e) {
            setState({ step: 'idle' })
            pending.current = null
            reject(e)
          }
        })()
      }),
    [],
  )

  const submit = async (method: string): Promise<void> => {
    const p = pending.current
    const el = pwRef.current
    if (!p || !el) return
    const password = el.value
    el.value = ''
    setHasPw(false)
    try {
      const { token } = await finishSewConfirm(p.action, p.secretId, method, p.label, password)
      setState({ step: 'idle' })
      pending.current = null
      p.resolve(token)
    } catch (e) {
      setState({ step: 'idle' })
      pending.current = null
      p.reject(e)
    }
  }

  const cancel = (): void => {
    pending.current?.resolve(null)
    pending.current = null
    setState({ step: 'idle' })
  }

  let ui: React.JSX.Element | null = null
  if (state.step === 'starting') ui = <p className={`text-[11px] ${muted}`}>Asking for your confirm…</p>
  if (state.step === 'mac') ui = <p className={`text-[11px] ${muted}`}>Confirm on your Mac.</p>
  if (state.step === 'password') {
    const method = state.method
    ui = (
      <form
        className="flex flex-col gap-1.5 max-w-sm"
        data-testid="sew-person-confirm"
        onSubmit={(e) => {
          e.preventDefault()
          void submit(method)
        }}
      >
        <label className={`text-[11px] ${muted}`}>
          {method === 'connect-password' ? 'Your K2 Connect password' : 'Your vault password'}
          <input
            ref={pwRef}
            type="password"
            autoComplete="current-password"
            aria-label="Confirm password"
            onChange={(e) => setHasPw(e.currentTarget.value !== '')}
            className="mt-1 w-full border border-[var(--color-border)] bg-transparent px-2 py-1 text-xs"
          />
        </label>
        <div className="flex gap-1.5">
          <Button type="submit" variant="accent" disabled={!hasPw}>
            Confirm
          </Button>
          <Button type="button" variant="ghost" onClick={cancel}>
            Cancel
          </Button>
        </div>
      </form>
    )
  }
  return { confirm, ui }
}
