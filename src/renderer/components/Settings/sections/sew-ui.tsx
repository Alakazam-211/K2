// Small parts local to Settings → Chat / Voice (Sew S32 K27): the status
// dot and word, a type-back field, the shown-once recovery key, the person
// confirm (vault) for the person-only steps, and a confirm-first question.
// Every question and step opens in the Settings dialog frame
// (AccessDialogs), so this page asks the way the rest of Settings does.

import React, { useCallback, useEffect, useId, useRef, useState } from 'react'
import { Button, Callout, Input, Surface } from '@/components/ui'
import { AccessSheet, ConfirmSheet, CopyLine, DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED } from './AccessDialogs'
import { finishSewConfirm, sewStateChip, startSewConfirm, type SewConfirmAction, type SewFailure } from './sew-api'
import { confirmPasswordLabel, LoginUsernameField, type ConfirmIdentity } from './confirm-password-ui'

export const muted = 'text-[var(--color-text-muted)]'
export const secondary = 'text-[var(--color-text-secondary)]'

/** Copy a quiet technical value (an id, a command), with its own result. */
export function CopyButton({ value, label }: { value: string; label: string }): React.JSX.Element {
  const [state, setState] = useState<'idle' | 'done' | 'failed'>('idle')
  return (
    <Button
      variant="quiet"
      aria-label={label}
      onClick={() => {
        const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
        if (!clip) return setState('failed')
        clip.writeText(value).then(
          () => setState('done'),
          () => setState('failed'),
        )
      }}
    >
      {state === 'done' ? 'Copied' : state === 'failed' ? 'Select it to copy' : 'Copy'}
    </Button>
  )
}

/** What to ask a server admin to do: the plain ask as a sentence, then the
 *  command quiet in a small value box, with Copy. Never the primary line. */
export function CommandLine({ command, ask = 'Ask an admin to run this on the server.' }: { command: string; ask?: string }): React.JSX.Element {
  return (
    <span className="block mt-1" data-testid="sew-command">
      <span className="block">{ask}</span>
      <span className="mt-1 flex flex-wrap items-center gap-2 text-[var(--color-text-muted)]">
        <code data-k2-chamfer="chip" className="font-mono break-all px-1.5 py-0.5 rounded-[var(--radius-field)] border border-[var(--color-border)] bg-[var(--color-bg)]">
          {command}
        </code>
        <CopyButton value={command} label={`Copy the command ${command}`} />
      </span>
    </span>
  )
}

/** A failed action: what went wrong in plain words (the daemon's code
 *  mapped by `sewFailure`, else the caller's `what`), what to do now, an
 *  optional next-step button, then the server's own words, quieter. */
export function SewFailed({
  what,
  failure,
  next,
  action,
}: {
  what: string
  failure: SewFailure | string
  next: string
  action?: React.ReactNode
}): React.JSX.Element {
  const f: SewFailure = typeof failure === 'string' ? { line: null, next: null, detail: failure, code: null } : failure
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    ref.current?.scrollIntoView?.({ block: 'nearest' })
  }, [f.detail])
  const detail = f.detail.trim().replace(/[.\s]+$/, '')
  return (
    <div ref={ref}>
      <Callout tone="error" role="alert" className="type-hint leading-relaxed" data-testid="dialog-error">
        <div className="flex items-start justify-between gap-4">
          <div className="min-w-0 space-y-0.5">
            <p className="font-medium">{f.line ?? what}</p>
            <p>{f.next ?? next}</p>
            {detail && detail !== f.line?.replace(/[.\s]+$/, '') ? <p className="text-[var(--color-text-secondary)]">The server said: {detail}.</p> : null}
          </div>
          {action ? <div className="flex-shrink-0">{action}</div> : null}
        </div>
      </Callout>
    </div>
  )
}

/** A round status dot (dots and badges are the only round marks in Terminal). */
export function StatusDot({ color, className = '' }: { color: string; className?: string }): React.JSX.Element {
  return <span aria-hidden="true" className={`inline-block w-2 h-2 rounded-[var(--radius-badge)] flex-shrink-0 ${className}`} style={{ backgroundColor: color }} />
}

/** The server's state as a dot and a plain word. */
export function SewStatusChip({ state }: { state: string }): React.JSX.Element {
  const { label, color } = sewStateChip(state)
  return (
    <span className="inline-flex items-center gap-2 type-row text-[var(--color-text-primary)]">
      <StatusDot color={color} />
      <span data-testid="sew-state-chip">{label}</span>
    </span>
  )
}

/** The confirm stays off until the typed text equals `expect` exactly
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
  const id = useId()
  return (
    <div>
      <label htmlFor={id} className={DIALOG_LABEL}>
        {label}
      </label>
      <Input id={id} aria-label={label} value={value} disabled={disabled} placeholder={expect} autoComplete="off" spellCheck={false} onChange={(e) => onChange(e.target.value)} className={DIALOG_FIELD} />
    </div>
  )
}

export function typedMatches(typed: string, expect: string): boolean {
  return expect !== '' && typed.trim() === expect
}

const ONCE_SECRET_MS = 5 * 60 * 1000

/** The recovery key, shown once in a dialog: the value lives in the
 *  PARENT's component state only and the parent drops it on `onDone`.
 *  The dialog closes itself after 5 minutes. Never logged, toasted or
 *  stored. */
export function OnceSecret({ title, note, secret, warning, onDone }: { title: string; note: string; secret: string; warning: string; onDone: () => void }): React.JSX.Element {
  const doneRef = useRef(onDone)
  doneRef.current = onDone
  useEffect(() => {
    const t = window.setTimeout(() => doneRef.current(), ONCE_SECRET_MS)
    return () => window.clearTimeout(t)
  }, [])
  return (
    <AccessSheet
      title={title}
      intro={note}
      onClose={onDone}
      testId="sew-once-secret"
      footer={
        <Button variant="accent" size="md" onClick={onDone} autoFocus>
          Done
        </Button>
      }
    >
      <Surface chamfer material="tile">
        <CopyLine label="Recovery key" value={secret} testId="sew-once-secret-value" />
      </Surface>
      <p className={DIALOG_MUTED}>{warning}</p>
      <p className={DIALOG_MUTED}>Closing this hides the key again. You can show it again here at any time.</p>
    </AccessSheet>
  )
}

type ConfirmStep = { step: 'idle' } | { step: 'starting' } | { step: 'mac' } | { step: 'password'; method: string; who: ConfirmIdentity }

interface Pending {
  action: SewConfirmAction
  secretId: string | undefined
  label: string
  resolve: (token: string | null) => void
  reject: (e: unknown) => void
}

/** A person confirm for one step (vault `confirm/start` + `finish`):
 *  Touch ID / the Mac login sheet, else the Connect or vault password, in
 *  a "Confirm it's you" dialog. `confirm()` resolves the one-use 60 s
 *  token, or null when cancelled. */
export function usePersonConfirm(): {
  confirm: (action: SewConfirmAction, secretId: string | undefined, label: string) => Promise<string | null>
  ui: React.JSX.Element | null
} {
  const [state, setState] = useState<ConfirmStep>({ step: 'idle' })
  const [hasPw, setHasPw] = useState(false)
  const pending = useRef<Pending | null>(null)
  const pwRef = useRef<HTMLInputElement>(null)
  const formId = useId()
  const pwId = useId()

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
            const { methods, who } = await startSewConfirm(action, secretId)
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
              setState({ step: 'password', method: pw, who })
              return
            }
            throw new Error('This server has no way to confirm it’s you yet. Set a vault password from a terminal outside K2 first')
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
  if (state.step === 'mac') {
    ui = (
      <AccessSheet title="Confirm it’s you" onClose={cancel} testId="sew-person-confirm-mac" footer={<Button variant="ghost" size="md" onClick={cancel}>Cancel</Button>}>
        <p className={DIALOG_MUTED}>Confirm on your Mac with Touch ID or your login password.</p>
      </AccessSheet>
    )
  }
  if (state.step === 'password') {
    const method = state.method
    const what = confirmPasswordLabel(method, state.who)
    ui = (
      <AccessSheet
        title="Confirm it’s you"
        intro={`K2 asks for ${method === 'connect-password' ? 'your sign-in password for this server' : 'your vault password'} before this step.`}
        onClose={cancel}
        testId="sew-person-confirm"
        footer={
          <>
            <Button variant="ghost" size="md" onClick={cancel}>
              Cancel
            </Button>
            <Button type="submit" form={formId} variant="accent" size="md" disabled={!hasPw}>
              Confirm
            </Button>
          </>
        }
      >
        <form
          id={formId}
          onSubmit={(e) => {
            e.preventDefault()
            void submit(method)
          }}
        >
          <LoginUsernameField method={method} who={state.who} testId="sew-confirm-username" />
          <label htmlFor={pwId} className={DIALOG_LABEL}>
            {what}
          </label>
          <Input
            id={pwId}
            ref={pwRef}
            type="password"
            autoFocus
            autoComplete="current-password"
            aria-label="Confirm password"
            onChange={(e) => setHasPw(e.currentTarget.value !== '')}
            className={DIALOG_FIELD}
          />
        </form>
      </AccessSheet>
    )
  }
  return { confirm, ui }
}

export interface AskOptions {
  title: string
  message: React.ReactNode
  confirmLabel: string
  destructive?: boolean
}

/** A confirm-first question in the Settings dialog frame: `ask()` resolves
 *  true on the confirm button, false on Cancel / Esc. Cancel has the focus. */
export function useAsk(): { ask: (o: AskOptions) => Promise<boolean>; sheet: React.JSX.Element | null } {
  const [open, setOpen] = useState<(AskOptions & { resolve: (ok: boolean) => void }) | null>(null)
  const ask = useCallback((o: AskOptions) => new Promise<boolean>((resolve) => setOpen({ ...o, resolve })), [])
  const end = (ok: boolean): void => {
    open?.resolve(ok)
    setOpen(null)
  }
  const sheet = open ? (
    <ConfirmSheet
      title={open.title}
      message={open.message}
      confirmLabel={open.confirmLabel}
      destructive={open.destructive}
      onConfirm={() => end(true)}
      onCancel={() => end(false)}
      testId="sew-ask"
    />
  ) : null
  return { ask, sheet }
}
