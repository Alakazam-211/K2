// RemoteSignIn — the full-screen sign-in for a saved K2 server with no
// live session token (or a rejected/expired one). connect-users (#617):
//
//   "Password not remembered (or remembered-but-rejected/expired) →
//    full-screen sign-in for that specific server (pre-filled label +
//    address + username, focus the password field), then connect."
//
// Mounted by ConnectionGate when `pendingSignIn` is set on the
// connect-host store. On success it:
//   - exchanges {username, password} for a session token via
//     loginToHost (POST /cli/auth/login) — which commits the session
//     token into the store + caches it to the keychain
//   - if "Remember password" → caches the PASSWORD to the OS keychain
//     (rememberPassword) and persists remember:true; else forgets it
//   - selectHost(host) to commit the switch (gate re-polls + mounts)
//
// If the password is already remembered, we auto-login on mount without
// prompting.

import React, { useEffect, useRef, useState } from 'react'
import {
  useConnectHostStore,
  rememberPassword,
  resolvePassword,
  forgetPassword,
  loginToHost,
  type ConnectHost,
} from '@/stores/connect-host'
import { IconLock } from '@/components/icons/IconLock'
import { isWebClient } from '@/lib/is-web'

export function RemoteSignIn({ host }: { host: ConnectHost }): React.JSX.Element {
  const selectHost = useConnectHostStore((s) => s.selectHost)
  const addHost = useConnectHostStore((s) => s.addHost)
  const cancelSignIn = useConnectHostStore((s) => s.cancelSignIn)
  // false ⇒ tile "Sign in" for management: cache the token but DON'T switch the
  // active daemon (the overlay just closes, staying on the current view).
  const activate = useConnectHostStore((s) => s.signInActivate)

  // Hosted web / first-time same-origin hosts often have no saved username
  // (boot seeds hostname only). Always allow editing; pre-fill when known.
  const [username, setUsername] = useState(host.username ?? '')
  const [password, setPassword] = useState('')
  // Default the remember toggle to the host's saved intent.
  const [remember, setRemember] = useState(host.remember)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const usernameRef = useRef<HTMLInputElement | null>(null)
  const passwordRef = useRef<HTMLInputElement | null>(null)
  const web = isWebClient()
  // Username is locked only when a non-empty value was already configured
  // on a desktop saved host (classic flow). Web / empty always editable.
  const usernameLocked = !web && Boolean(host.username?.trim())

  const address = host.secure && host.port === 443
    ? host.hostname
    : `${host.hostname}:${host.port}`

  // Shared login path used by both the auto-login effect and the submit
  // handler. On success commits + switches; on failure surfaces the
  // reason and (for auto-login) leaves the form for manual entry.
  const doLogin = async (
    pw: string,
    rememberPw: boolean,
    user = username,
  ): Promise<boolean> => {
    const uname = user.trim()
    if (!uname) {
      setError('Enter your username.')
      return false
    }
    const hostWithUser: ConnectHost = { ...host, username: uname }
    const result = await loginToHost(hostWithUser, pw)
    if (!result.ok) {
      setError(result.reason)
      return false
    }
    // loginToHost committed the session token + lastConnectedAt into the
    // store. Persist remember intent + the keychain side.
    const refreshed = useConnectHostStore.getState().hosts.find((h) => h.id === host.id)
    const updated: ConnectHost = {
      ...(refreshed ?? hostWithUser),
      username: uname,
      token: result.token,
      remember: rememberPw,
      lastConnectedAt: Date.now(),
    }
    addHost(updated) // re-persists the (token-less) list with remember + lastConnectedAt
    if (rememberPw) {
      await rememberPassword(host.id, pw)
    } else {
      await forgetPassword(host.id)
    }
    // Switcher path ⇒ switch (the gate re-polls against the new host and
    // mounts on accept). Management path ⇒ just close the overlay; the token is
    // cached so the tile's owner controls work, and we stay on the current view.
    if (activate) {
      selectHost(updated)
    } else {
      cancelSignIn()
    }
    return true
  }

  // Auto-login if the password was remembered AND we have a username;
  // otherwise focus username (if empty) or password for manual entry.
  // (connect-users #617: "If the password was remembered, auto-login
  // without prompting.")
  useEffect(() => {
    let cancelled = false
    void (async () => {
      if (host.remember && host.username?.trim()) {
        const pw = await resolvePassword(host.id)
        if (cancelled) return
        if (pw) {
          setBusy(true)
          const ok = await doLogin(pw, true, host.username)
          if (cancelled) return
          if (ok) return // switched away — overlay will unmount
          // Remembered password was rejected/expired → fall through to
          // manual entry.
          setBusy(false)
        }
      }
      if (!username.trim()) {
        usernameRef.current?.focus()
      } else {
        passwordRef.current?.focus()
      }
    })()
    return () => { cancelled = true }
    // host id is the stable identity; doLogin closes over `host` which is
    // stable for a given pendingSignIn render.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [host.id])

  const submit = async (): Promise<void> => {
    if (!username.trim()) {
      setError('Enter your username.')
      usernameRef.current?.focus()
      return
    }
    if (!password) {
      setError('Enter the server password.')
      return
    }
    setBusy(true)
    setError(null)
    const ok = await doLogin(password, remember)
    if (!ok) setBusy(false)
  }

  const inputCls =
    'w-full px-2.5 py-1.5 text-[13px] bg-[var(--color-bg)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]'

  return (
    <div className="fixed inset-0 z-[10000] flex items-center justify-center bg-[var(--color-bg)] text-[var(--color-text-primary)]">
      <form
        onSubmit={(e) => {
          e.preventDefault()
          void submit()
        }}
        className="w-[360px] max-w-[90vw] flex flex-col gap-3.5 p-7 border border-[var(--color-border)] bg-[var(--color-bg-surface)]"
      >
        <div className="flex flex-col gap-1">
          <div className="text-[15px] font-semibold text-[var(--color-text-primary)]">
            Sign in to {host.label}
          </div>
          <div className="flex items-center gap-1 text-[12px] text-[var(--color-text-muted)] font-mono">
            {host.secure && <IconLock className="w-3 h-3 flex-shrink-0" />}
            {address}
          </div>
        </div>

        <label className="flex flex-col gap-1.5 text-[12px] text-[var(--color-text-secondary)]">
          Username
          <input
            ref={usernameRef}
            type="text"
            value={username}
            readOnly={usernameLocked}
            disabled={busy}
            onChange={(e) => setUsername(e.target.value)}
            placeholder="e.g. z3thon"
            autoComplete="username"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            className={`${inputCls} ${usernameLocked ? 'opacity-70' : ''} disabled:opacity-60`}
          />
        </label>

        <label className="flex flex-col gap-1.5 text-[12px] text-[var(--color-text-secondary)]">
          Password
          <input
            ref={passwordRef}
            type="password"
            value={password}
            disabled={busy}
            onChange={(e) => setPassword(e.target.value)}
            placeholder="Server password"
            autoComplete="current-password"
            className={`${inputCls} disabled:opacity-60`}
          />
        </label>

        <label className="flex items-center gap-2 text-[12px] text-[var(--color-text-secondary)] cursor-pointer select-none">
          <input
            type="checkbox"
            checked={remember}
            disabled={busy}
            onChange={(e) => setRemember(e.target.checked)}
            className="peer sr-only"
          />
          <span
            aria-hidden="true"
            className="w-3.5 h-3.5 flex-shrink-0 flex items-center justify-center border border-[var(--color-border)] bg-[var(--color-bg)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)]"
          >
            {remember && (
              <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="white" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                <path d="M2.5 6.5 L5 9 L9.5 3.5" />
              </svg>
            )}
          </span>
          {web
            ? 'Remember password (this browser)'
            : 'Remember password (stored in your OS keychain)'}
        </label>

        {error && (
          <div className="text-[12px] text-[var(--color-status-error-soft)]" role="alert">
            {error}
          </div>
        )}

        <div className="flex gap-2 mt-1">
          <button
            type="submit"
            disabled={busy}
            className="flex-1 px-4 py-1.5 text-[13px] text-[var(--color-on-accent)] bg-[var(--color-accent)] hover:opacity-90 cursor-pointer disabled:opacity-60 disabled:cursor-progress"
          >
            {busy ? 'Connecting…' : 'Connect'}
          </button>
          <button
            type="button"
            onClick={cancelSignIn}
            disabled={busy}
            className="px-4 py-1.5 text-[13px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-60"
          >
            Cancel
          </button>
        </div>
      </form>
    </div>
  )
}
