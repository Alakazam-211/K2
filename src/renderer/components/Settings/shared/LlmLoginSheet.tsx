// Settings → LLMs: the sign-in sheet for a subscription ("Add
// subscription" / "Sign in again"; a wallet login internally).
//
// The daemon runs the tool's OWN login command in a daemon-owned terminal
// on the server. This sheet turns that into three buttons a phone can use:
// Open sign-in page (in the viewer's own browser), Copy code (device code),
// Paste code (sent to the login terminal). The raw terminal text stays
// under "Show terminal" as the fallback.

import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import type { ServerScope } from '@/kessel/server-scope'
import {
  addLogin,
  cancelLogin,
  errorText,
  loginAgain,
  loginStatus,
  sendLoginInput,
  switchLogin,
  LOGIN_POLL_MS,
  LOGIN_STATE_LABELS,
  type LlmLogin,
  type LoginMode,
} from '@/stores/llm-accounts'

export type LoginSheetStart =
  | { kind: 'add'; tool: string; toolLabel: string }
  | { kind: 'relogin'; tool: string; toolLabel: string; accountId: string; label: string }

const FINAL = new Set(['signed_in', 'failed', 'cancelled', 'timed_out'])

export function LlmLoginSheet({
  scope,
  start,
  onClose,
}: {
  scope: ServerScope
  start: LoginSheetStart
  onClose: () => void
}): React.JSX.Element {
  const [label, setLabel] = useState(start.kind === 'relogin' ? start.label : '')
  const [mode, setMode] = useState<LoginMode>(scope.isRemote ? 'other_device' : 'this_computer')
  const [login, setLogin] = useState<LlmLogin | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [paste, setPaste] = useState('')
  const [showTerminal, setShowTerminal] = useState(false)
  const [copied, setCopied] = useState(false)
  const [madeActive, setMadeActive] = useState<null | 'active' | 'kept'>(null)
  const loginRef = useRef<LlmLogin | null>(null)
  loginRef.current = login

  const done = login ? login.done || FINAL.has(login.state) : false

  // Poll the login while it runs (no terminal needed on a phone).
  useEffect(() => {
    if (!login || done) return
    let stopped = false
    const id = login.loginId
    const timer = setInterval(() => {
      loginStatus(scope, id)
        .then((next) => {
          if (!stopped) setLogin(next)
        })
        .catch((e) => {
          if (!stopped) setError(errorText(e))
        })
    }, LOGIN_POLL_MS)
    return () => {
      stopped = true
      clearInterval(timer)
    }
  }, [scope, login?.loginId, done]) // eslint-disable-line react-hooks/exhaustive-deps

  const begin = useCallback(async () => {
    setBusy(true)
    setError(null)
    try {
      const out =
        start.kind === 'add'
          ? await addLogin(scope, start.tool, label.trim(), mode)
          : await loginAgain(scope, start.accountId, mode)
      setLogin(out.login)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [scope, start, label, mode])

  const cancel = useCallback(async () => {
    const current = loginRef.current
    if (current && !(current.done || FINAL.has(current.state))) {
      try {
        const out = await cancelLogin(scope, current.loginId)
        setLogin(out.login)
      } catch (e) {
        setError(errorText(e))
        return
      }
    }
    onClose()
  }, [scope, onClose])

  const openSignIn = useCallback(() => {
    if (!login?.url) return
    void openUrl(login.url).catch((e: unknown) => setError(errorText(e)))
  }, [login?.url])

  const copyCode = useCallback(() => {
    if (!login?.code) return
    void navigator.clipboard
      ?.writeText(login.code)
      .then(() => setCopied(true))
      .catch((e: unknown) => setError(errorText(e)))
  }, [login?.code])

  const sendPaste = useCallback(async () => {
    if (!login || !paste.trim()) return
    try {
      await sendLoginInput(scope, login.loginId, paste.trim())
      setPaste('')
    } catch (e) {
      setError(errorText(e))
    }
  }, [scope, login, paste])

  const makeActive = useCallback(async () => {
    if (!login) return
    try {
      await switchLogin(scope, login.accountId)
      setMadeActive('active')
    } catch (e) {
      setError(errorText(e))
    }
  }, [scope, login])

  const title =
    start.kind === 'add' ? `Add a ${start.toolLabel} subscription` : `Sign ${start.label} in again`

  return (
    <div
      className="fixed inset-0 z-[500] flex items-center justify-center bg-black/50 p-4"
      role="dialog"
      aria-modal="true"
      aria-label={title}
      data-testid="llm-login-sheet"
    >
      <div className="w-full max-w-lg max-h-full overflow-y-auto bg-[var(--color-bg)] border border-[var(--color-border)] p-5 space-y-4">
        <div className="text-sm font-medium text-[var(--color-text-primary)]">{title}</div>

        {!login && (
          <div className="space-y-3">
            {start.kind === 'add' && (
              <label className="block">
                <span className="text-[10px] text-[var(--color-text-muted)]">Label</span>
                <input
                  autoFocus
                  value={label}
                  onChange={(e) => setLabel(e.target.value)}
                  placeholder="work, personal, team…"
                  data-testid="llm-login-label"
                  className="mt-1 w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] px-2 py-1.5 text-xs text-[var(--color-text-primary)] outline-none"
                />
              </label>
            )}
            <div>
              <div className="text-[10px] text-[var(--color-text-muted)] mb-1">Sign in from</div>
              <div className="flex gap-1">
                {(
                  [
                    ['this_computer', 'This computer'],
                    ['other_device', 'Another device'],
                  ] as const
                ).map(([value, text]) => (
                  <button
                    key={value}
                    type="button"
                    onClick={() => setMode(value)}
                    aria-pressed={mode === value}
                    className={`px-2 py-1 text-[11px] border ${
                      mode === value
                        ? 'border-[var(--color-accent)] text-[var(--color-text-primary)]'
                        : 'border-[var(--color-border)] text-[var(--color-text-muted)]'
                    }`}
                  >
                    {text}
                  </button>
                ))}
              </div>
              <p className="text-[10px] text-[var(--color-text-muted)] mt-1 leading-relaxed">
                {mode === 'this_computer'
                  ? 'The browser opens on the computer that runs this K2 server.'
                  : 'Open the sign-in page here (phone or laptop), then paste the code back or enter the device code.'}
              </p>
            </div>
            <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
              The tool runs its own sign-in on the server. K2 never sees your password; the subscription
              is kept on this server and isn’t the server default until you choose Use this token.
            </p>
            <div className="flex justify-end gap-2">
              <button type="button" onClick={onClose} className="px-3 py-1 text-xs text-[var(--color-text-muted)]">
                Cancel
              </button>
              <button
                type="button"
                disabled={busy || (start.kind === 'add' && !label.trim())}
                onClick={() => void begin()}
                data-testid="llm-login-start"
                className="px-3 py-1 text-xs border border-[var(--color-accent)] text-[var(--color-text-primary)] disabled:opacity-40"
              >
                {busy ? 'Starting…' : 'Start sign-in'}
              </button>
            </div>
          </div>
        )}

        {login && (
          <div className="space-y-3">
            {login.banner && (
              <div
                className="text-[11px] px-2 py-1.5 border border-yellow-500/60 text-yellow-300"
                data-testid="llm-login-banner"
              >
                {login.banner}
              </div>
            )}
            <div className="text-xs text-[var(--color-text-secondary)]" data-testid="llm-login-state">
              {LOGIN_STATE_LABELS[login.state] ?? login.state}
            </div>

            {!done && (
              <div className="space-y-3">
                <button
                  type="button"
                  disabled={!login.url}
                  onClick={openSignIn}
                  className="w-full py-2.5 text-sm font-medium border border-[var(--color-accent)] text-[var(--color-text-primary)] disabled:opacity-40"
                >
                  {login.url ? 'Open sign-in page' : 'Waiting for the sign-in page…'}
                </button>
                {login.code && (
                  <div className="flex items-center justify-between gap-3 border border-[var(--color-border)] px-3 py-2">
                    <span className="font-mono text-lg tracking-widest text-[var(--color-text-primary)]" data-testid="llm-login-code">
                      {login.code}
                    </span>
                    <button type="button" onClick={copyCode} className="text-[11px] text-[var(--color-accent)]">
                      {copied ? 'Copied' : 'Copy code'}
                    </button>
                  </div>
                )}
                {(login.state === 'waiting_for_code' || showTerminal) && (
                  <div className="flex gap-2">
                    <input
                      value={paste}
                      onChange={(e) => setPaste(e.target.value)}
                      placeholder="Paste code"
                      aria-label="Paste code"
                      autoComplete="off"
                      spellCheck={false}
                      className="flex-1 bg-[var(--color-bg-elevated)] border border-[var(--color-border)] px-2 py-1.5 text-xs font-mono text-[var(--color-text-primary)] outline-none"
                    />
                    <button
                      type="button"
                      disabled={!paste.trim()}
                      onClick={() => void sendPaste()}
                      className="px-3 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-primary)] disabled:opacity-40"
                    >
                      Send
                    </button>
                  </div>
                )}
              </div>
            )}

            {login.state === 'signed_in' && login.offerMakeActive && madeActive === null && (
              <div className="border border-[var(--color-border)] px-3 py-2 space-y-2">
                <div className="text-xs text-[var(--color-text-secondary)]">Make the new subscription the server default?</div>
                <div className="text-[10px] text-[var(--color-text-muted)]">
                  Changing it affects every chat that uses Server default on this server.
                </div>
                <div className="flex gap-2">
                  <button
                    type="button"
                    onClick={() => void makeActive()}
                    className="px-3 py-1 text-xs border border-[var(--color-accent)] text-[var(--color-text-primary)]"
                  >
                    Use this token
                  </button>
                  <button
                    type="button"
                    onClick={() => setMadeActive('kept')}
                    className="px-3 py-1 text-xs text-[var(--color-text-muted)]"
                  >
                    Keep current
                  </button>
                </div>
              </div>
            )}
            {madeActive === 'active' && (
              <div className="text-[11px] text-[var(--color-text-secondary)]">The new subscription is now the server default.</div>
            )}
            {login.error && <div className="text-[11px] text-red-400">{login.error}</div>}

            <div>
              <button
                type="button"
                onClick={() => setShowTerminal((v) => !v)}
                className="text-[10px] text-[var(--color-text-muted)]"
              >
                {showTerminal ? 'Hide terminal' : 'Show terminal'}
              </button>
              {showTerminal && (
                <pre
                  className="mt-1 max-h-48 overflow-auto bg-[var(--color-bg-elevated)] border border-[var(--color-border)] p-2 text-[10px] font-mono text-[var(--color-text-secondary)] whitespace-pre-wrap break-all"
                  data-testid="llm-login-terminal"
                >
                  {login.screen.join('\n')}
                </pre>
              )}
            </div>

            <div className="flex justify-end">
              <button
                type="button"
                onClick={() => (done ? onClose() : void cancel())}
                className="px-3 py-1 text-xs border border-[var(--color-border)] text-[var(--color-text-muted)]"
              >
                {done ? 'Close' : 'Cancel'}
              </button>
            </div>
          </div>
        )}

        {error && <div className="text-[11px] text-red-400" data-testid="llm-login-error">{error}</div>}
      </div>
    </div>
  )
}
