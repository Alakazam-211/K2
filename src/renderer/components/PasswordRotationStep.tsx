// PasswordRotationStep — the ONE "Set a new password" step (PRD
// connect-login-edge-only §4.2 W1–W4), shared by every first-time sign-in
// entry point:
//   - RemoteSignIn (full-screen re-auth + the switcher's pickHost path, and
//     the W2 data-plane `password_change_required` route-back)
//   - Settings → Connections "Add server" (inline, `variant="inline"`)
//
// Given a host whose `token` is the RESTRICTED session the daemon minted for
// a temporary password, it: reads the active policy, validates new+confirm
// client-side, POSTs change-password, then RE-LOGS-IN with the new password
// (the daemon revokes every session of the user on change — including this
// one) and hands the caller the fresh token via `onDone`. The host is not
// "signed in" until `onDone` fires; callers finish their own commit
// (remember the NEW password, selectHost / close) there.
//
// Works identically in desktop and hosted-web builds: the daemon calls go
// through lib/password-rotation.ts (`?token=` + web cookie/CSRF header via
// withDaemonFetch); the re-login is the same `loginToHost` every entry uses.

import React, { useEffect, useRef, useState } from 'react'
import { loginToHost, type ConnectHost } from '@/stores/connect-host'
import {
  DEFAULT_PASSWORD_POLICY,
  ROTATION_COPY,
  changeHostPassword,
  fetchPasswordPolicy,
  policyHint,
  validateNewPassword,
  type PasswordPolicy,
} from '@/lib/password-rotation'

export interface PasswordRotationStepProps {
  /** The host to rotate on. `token` = the restricted session; `username`
   *  = the account being rotated. */
  host: ConnectHost
  /** Prefill for the temporary-password field (what the user just typed,
   *  or the keychain-remembered password). Editable — the W2 route-back may
   *  not know it. */
  initialCurrentPassword?: string
  /** Fired ONLY after change-password succeeded AND the re-login with the
   *  new password minted a fresh, unrestricted session (already committed
   *  to the store by loginToHost). */
  onDone: (result: { token: string; newPassword: string }) => void | Promise<void>
  /** Back out (RemoteSignIn: back to the password form / close; add-server:
   *  discard the provisional host). */
  onCancel?: () => void
  /** 'overlay' = the full-screen sign-in look (13px); 'inline' = the
   *  Settings add-server form look (xs). */
  variant?: 'overlay' | 'inline'
}

const CLS = {
  overlay: {
    input:
      'w-full px-2.5 py-1.5 text-[13px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] disabled:opacity-60',
    label: 'flex flex-col gap-1.5 text-[12px] text-[var(--color-text-secondary)]',
    lead: 'text-[12px] text-[var(--color-text-secondary)] leading-relaxed',
    hint: 'text-[11px] text-[var(--color-text-muted)]',
    error: 'text-[12px] text-[var(--color-status-error-soft)]',
    primary:
      'flex-1 px-4 py-1.5 text-[13px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 cursor-pointer disabled:opacity-60 disabled:cursor-progress',
    secondary:
      'px-4 py-1.5 text-[13px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-60',
    gap: 'flex flex-col gap-3.5',
  },
  inline: {
    input:
      'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] no-drag disabled:opacity-60',
    label: 'flex flex-col gap-1 text-[11px] text-[var(--color-text-secondary)]',
    lead: 'text-[11px] text-[var(--color-text-secondary)] leading-relaxed',
    hint: 'text-[10px] text-[var(--color-text-muted)]',
    error: 'text-[10px] text-[var(--color-status-error-soft)]',
    primary:
      'px-3 py-1 text-[11px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer disabled:opacity-60',
    secondary:
      'px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer disabled:opacity-60',
    gap: 'flex flex-col gap-2',
  },
} as const

export function PasswordRotationStep({
  host,
  initialCurrentPassword = '',
  onDone,
  onCancel,
  variant = 'overlay',
}: PasswordRotationStepProps): React.JSX.Element {
  const c = CLS[variant]
  const [current, setCurrent] = useState(initialCurrentPassword)
  const [next, setNext] = useState('')
  const [confirm, setConfirm] = useState('')
  const [policy, setPolicy] = useState<PasswordPolicy>(DEFAULT_PASSWORD_POLICY)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const nextRef = useRef<HTMLInputElement | null>(null)
  const currentRef = useRef<HTMLInputElement | null>(null)

  // Policy from the host (restricted sessions may read it). Keyed on the
  // host + token so a re-minted session re-reads. Falls back to the default
  // inside fetchPasswordPolicy; the daemon re-validates on submit anyway.
  useEffect(() => {
    let cancelled = false
    void fetchPasswordPolicy(host).then((p) => {
      if (!cancelled) setPolicy(p)
    })
    return () => {
      cancelled = true
    }
  }, [host.id, host.token]) // eslint-disable-line react-hooks/exhaustive-deps

  // The prefill may arrive AFTER mount (RemoteSignIn resolves the keychain-
  // remembered temporary password asynchronously on the W2 / pickHost
  // route-back). Adopt it only while the field is still empty — never
  // clobber something the user has started typing.
  useEffect(() => {
    if (initialCurrentPassword) {
      setCurrent((c) => (c ? c : initialCurrentPassword))
      nextRef.current?.focus()
    } else {
      currentRef.current?.focus()
    }
  }, [initialCurrentPassword])

  const submit = async (): Promise<void> => {
    if (!current) {
      setError(ROTATION_COPY.missingCurrent)
      currentRef.current?.focus()
      return
    }
    const weak = validateNewPassword(next, policy)
    if (weak) {
      setError(weak)
      nextRef.current?.focus()
      return
    }
    if (next !== confirm) {
      setError(ROTATION_COPY.mismatch)
      return
    }
    setBusy(true)
    setError(null)
    const changed = await changeHostPassword(host, current, next)
    if (!changed.ok) {
      setError(changed.reason)
      setBusy(false)
      return
    }
    // Every session of this user is now revoked (the daemon's rule) — mint
    // a fresh, unrestricted one with the NEW password. Same loginToHost
    // (same edge login URL rule) as every other sign-in.
    const login = await loginToHost(host, next)
    if (!login.ok) {
      setError(`Password changed. Sign in again with your new password. (${login.reason})`)
      setBusy(false)
      return
    }
    if (login.mustChangePassword) {
      setError('The server still requires a password change. Contact your server admin.')
      setBusy(false)
      return
    }
    await onDone({ token: login.token, newPassword: next })
    // The parent switches / closes on success; if it stays mounted (add-
    // server inline), release the busy state.
    setBusy(false)
  }

  return (
    <form
      className={c.gap}
      data-testid="password-rotation-step"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      <div className="flex flex-col gap-1">
        <div
          className={
            variant === 'overlay'
              ? 'text-[15px] font-semibold text-[var(--color-text-primary)]'
              : 'text-xs font-medium text-[var(--color-text-primary)]'
          }
        >
          {ROTATION_COPY.title}
        </div>
        <p className={c.lead}>{ROTATION_COPY.lead}</p>
      </div>

      <label className={c.label}>
        {ROTATION_COPY.currentLabel}
        <input
          ref={currentRef}
          type="password"
          value={current}
          disabled={busy}
          onChange={(e) => setCurrent(e.target.value)}
          autoComplete="current-password"
          className={c.input}
        />
      </label>

      <div className="flex flex-col gap-1">
        <label className={c.label}>
          {ROTATION_COPY.newLabel}
          <input
            ref={nextRef}
            type="password"
            value={next}
            disabled={busy}
            onChange={(e) => setNext(e.target.value)}
            autoComplete="new-password"
            aria-describedby="k2-password-rotation-policy"
            className={c.input}
          />
        </label>
        <span id="k2-password-rotation-policy" className={c.hint}>
          {policyHint(policy)}
        </span>
      </div>

      <label className={c.label}>
        {ROTATION_COPY.confirmLabel}
        <input
          type="password"
          value={confirm}
          disabled={busy}
          onChange={(e) => setConfirm(e.target.value)}
          autoComplete="new-password"
          className={c.input}
        />
      </label>

      {error && (
        <div className={c.error} role="alert">
          {error}
        </div>
      )}

      <div className="flex gap-2 mt-1">
        <button type="submit" disabled={busy} className={c.primary}>
          {busy ? 'Setting password…' : ROTATION_COPY.submit}
        </button>
        {onCancel && (
          <button type="button" onClick={onCancel} disabled={busy} className={c.secondary}>
            Cancel
          </button>
        )}
      </div>
    </form>
  )
}
