// Settings → Language Models → Subs / Tokens: the sheet that adds a token
// ("Add a token", or a provider's own Add) and signs one in again. (A
// wallet login internally; routes keep those names.)
//
// One short flow in the shared dialog frame (the same frame, width and
// place as Admin Access's dialogs): choose a provider and how to add it →
// sign in in the browser, or paste an API token → done (what is now saved,
// where it applies) or failed (what failed, the server's reason quietly,
// what to do next: try again, or paste an API token instead).
//
// Sign-in: the daemon runs the tool's OWN login command in a daemon-owned
// terminal on the server. This sheet turns that into buttons a phone can
// use: Open sign-in page (in the viewer's own browser), Copy link, Copy code
// (device code), Paste code (sent to the login terminal). The raw terminal
// text stays under "Show terminal" as the fallback.
//
// The sign-in link is never cut (0.46.3 S1): Open and Copy link always use
// the whole stored `login.url`, never text read back from the page, and the
// full link can be shown wrapped (no ellipsis, no max length).
//
// Signed in: when the daemon offers K5 (`offer: "meet_k5"`, prd-setup-checklist-v1
// §10.1), the success state leads to it (`K5Offer`).
//
// An API token is a password field with Show; it is sent once and cleared
// from state right after. Afterwards the sheet shows at most its last four
// characters.

import React from 'react'
import { useCallback, useEffect, useId, useRef, useState } from 'react'
import { openUrl } from '@tauri-apps/plugin-opener'
import type { ServerScope } from '@/kessel/server-scope'
import AgentIcon from '@/components/AgentIcon/AgentIcon'
import { Button, Callout, Input, Radio, Segmented, Spinner, Surface } from '@/components/ui'
import { SETTING_ROW_LABEL } from '../controls/SettingControls'
import { AccessSheet, DIALOG_FIELD, DIALOG_LABEL, DIALOG_MUTED, FailedNote, Field } from '../sections/AccessDialogs'
import {
  addApiKey,
  addLogin,
  cancelLogin,
  errorText,
  loginAgain,
  loginStatus,
  sendLoginInput,
  serverInSentence,
  switchLogin,
  WALLET_TOOLS,
  LOGIN_POLL_MS,
  LOGIN_STATE_LABELS,
  type LlmLogin,
  type LoginMode,
  useLlmAccountsStore,
} from '@/stores/llm-accounts'
import { K5Offer } from '@/components/K5Sheet/K5Offer'

export type LoginSheetStart =
  /** Add a token: choose the provider here (or start on `tool`) and how. */
  | { kind: 'new'; tool?: string; method?: AddMethod }
  /** Add a subscription for `tool` (the provider and sign-in chosen). */
  | { kind: 'add'; tool: string; toolLabel: string }
  /** Sign a saved subscription in again. */
  | { kind: 'relogin'; tool: string; toolLabel: string; accountId: string; label: string }

export type AddMethod = 'signin' | 'key'

/** What the sheet added, for the page's saved note. */
export interface LoginSheetResult {
  tool: string
  label: string
  method: AddMethod | 'relogin'
}

const FINAL = new Set(['signed_in', 'failed', 'cancelled', 'timed_out'])

/** The host to show for a sign-in URL: the daemon's `urlHost`, else parsed here. */
export function signInHost(login: Pick<LlmLogin, 'url' | 'urlHost'>): string | null {
  if (login.urlHost) return login.urlHost
  if (!login.url) return null
  try {
    return new URL(login.url).host || null
  } catch {
    return null
  }
}

/** At most the last four characters of a key, masked; nothing for a key
 *  too short to show any of it safely. */
export function maskedTail(key: string): string | null {
  return key.length >= 12 ? `•••• ${key.slice(-4)}` : null
}

/** A provider the sheet can add a token for. */
interface Provider {
  tool: string
  display: string
  subscription: boolean
  apiKeys: boolean
}

function providerHint(p: Provider): string {
  if (p.subscription && p.apiKeys) return 'Sign in with a subscription, or paste an API token.'
  if (p.subscription) return 'Sign in with a subscription.'
  return 'API tokens only.'
}

const ORDER: readonly string[] = WALLET_TOOLS

export function LlmLoginSheet({
  scope,
  start,
  onClose,
  onDone,
}: {
  scope: ServerScope
  start: LoginSheetStart
  onClose: () => void
  /** A token was added or signed in again (for the page's saved note). */
  onDone?: (r: LoginSheetResult) => void
}): React.JSX.Element {
  const doc = useLlmAccountsStore((s) => s.entries[scope.id]?.doc ?? null)
  // Only Admin and above may make it the server default (the daemon says
  // so in the list doc); a Member never sees the offer.
  const canChangeDefault = doc?.canChangeDefault !== false
  const airgap = doc?.airgap === true
  const relogin = start.kind === 'relogin'

  const providers: Provider[] = (() => {
    const fromDoc = (doc?.tools ?? [])
      .filter((t) => t.supported && (t.subscription || t.apiKeys))
      .map((t) => ({ tool: t.tool, display: t.display, subscription: t.subscription, apiKeys: t.apiKeys }))
      .sort((a, b) => rank(a.tool) - rank(b.tool))
    if (start.kind === 'new') return fromDoc
    // Started on one provider: that one, as the list describes it (or as
    // the caller named it before the list loads).
    const known = fromDoc.find((p) => p.tool === start.tool)
    return [known ?? { tool: start.tool, display: start.toolLabel, subscription: true, apiKeys: false }]
  })()

  const [tool, setTool] = useState<string | null>(start.kind === 'new' ? (start.tool ?? null) : start.tool)
  const provider = providers.find((p) => p.tool === tool) ?? (providers.length === 1 ? providers[0] : null)
  const [method, setMethod] = useState<AddMethod>(start.kind === 'new' && start.method ? start.method : 'signin')
  // A provider without one of the two ways uses the other.
  const how: AddMethod = relogin
    ? 'signin'
    : provider && !provider.subscription
      ? 'key'
      : provider && !provider.apiKeys
        ? 'signin'
        : method

  const [label, setLabel] = useState(start.kind === 'relogin' ? start.label : '')
  const [mode, setMode] = useState<LoginMode>(scope.isRemote ? 'other_device' : 'this_computer')
  const [key, setKey] = useState('')
  const [keyShown, setKeyShown] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [login, setLogin] = useState<LlmLogin | null>(null)
  const [pollError, setPollError] = useState<string | null>(null)
  const [keyDone, setKeyDone] = useState<{ label: string; tail: string | null } | null>(null)
  const [paste, setPaste] = useState('')
  const [showTerminal, setShowTerminal] = useState(false)
  const [copied, setCopied] = useState(false)
  const [showLink, setShowLink] = useState(false)
  const [linkCopied, setLinkCopied] = useState(false)
  const [madeActive, setMadeActive] = useState<null | 'active' | 'kept'>(null)
  const [actionError, setActionError] = useState<string | null>(null)
  const loginRef = useRef<LlmLogin | null>(null)
  loginRef.current = login
  const reported = useRef(false)
  const ids = { label: useId(), key: useId(), paste: useId() }

  const done = login ? login.done || FINAL.has(login.state) : false
  const signedIn = login?.state === 'signed_in'
  const display = provider?.display ?? (start.kind === 'new' ? 'the provider' : start.toolLabel)
  const server = serverInSentence(scope)

  // Poll the login while it runs (no terminal needed on a phone).
  useEffect(() => {
    if (!login || done) return
    let stopped = false
    const id = login.loginId
    const timer = setInterval(() => {
      loginStatus(scope, id)
        .then((next) => {
          if (stopped) return
          setPollError(null)
          setLogin(next)
        })
        .catch((e) => {
          if (!stopped) setPollError(errorText(e))
        })
    }, LOGIN_POLL_MS)
    return () => {
      stopped = true
      clearInterval(timer)
    }
  }, [scope, login?.loginId, done]) // eslint-disable-line react-hooks/exhaustive-deps

  // Tell the page once, when a sign-in lands.
  useEffect(() => {
    if (!signedIn || reported.current || !provider) return
    reported.current = true
    onDone?.({ tool: provider.tool, label: login?.label || label.trim(), method: relogin ? 'relogin' : 'signin' })
  }, [signedIn]) // eslint-disable-line react-hooks/exhaustive-deps

  const begin = useCallback(async () => {
    if (!provider) return
    setBusy(true)
    setError(null)
    setPollError(null)
    setMadeActive(null)
    reported.current = false
    try {
      const out =
        start.kind === 'relogin'
          ? await loginAgain(scope, start.accountId, mode)
          : await addLogin(scope, provider.tool, label.trim(), mode)
      setLogin(out.login)
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [scope, start, provider, label, mode])

  const addKey = useCallback(async () => {
    if (!provider) return
    const sentKey = key.trim()
    const name = label.trim()
    setKey('')
    setKeyShown(false)
    setBusy(true)
    setError(null)
    try {
      await addApiKey(scope, provider.tool, name, sentKey)
      setKeyDone({ label: name, tail: maskedTail(sentKey) })
      onDone?.({ tool: provider.tool, label: name, method: 'key' })
    } catch (e) {
      setError(errorText(e))
    } finally {
      setBusy(false)
    }
  }, [scope, provider, key, label, onDone])

  const cancel = useCallback(async () => {
    const current = loginRef.current
    if (current && !(current.done || FINAL.has(current.state))) {
      try {
        const out = await cancelLogin(scope, current.loginId)
        setLogin(out.login)
      } catch (e) {
        setActionError(errorText(e))
        return
      }
    }
    onClose()
  }, [scope, onClose])

  const openSignIn = useCallback(() => {
    if (!login?.url) return
    void openUrl(login.url).catch((e: unknown) => setActionError(errorText(e)))
  }, [login?.url])

  // Copies the whole stored URL, never what the page shows.
  const copyLink = useCallback(() => {
    if (!login?.url) return
    const clip = navigator.clipboard
    if (!clip) {
      setActionError('Copy isn’t available here. Choose Show full link and select it.')
      return
    }
    void clip
      .writeText(login.url)
      .then(() => setLinkCopied(true))
      .catch((e: unknown) => setActionError(errorText(e)))
  }, [login?.url])

  // A new link (the tool issued another one) has not been copied yet.
  useEffect(() => {
    setLinkCopied(false)
  }, [login?.url])

  const copyCode = useCallback(() => {
    if (!login?.code) return
    void navigator.clipboard
      ?.writeText(login.code)
      .then(() => setCopied(true))
      .catch((e: unknown) => setActionError(errorText(e)))
  }, [login?.code])

  const sendPaste = useCallback(async () => {
    if (!login || !paste.trim()) return
    try {
      await sendLoginInput(scope, login.loginId, paste.trim())
      setPaste('')
    } catch (e) {
      setActionError(errorText(e))
    }
  }, [scope, login, paste])

  const makeActive = useCallback(async () => {
    if (!login) return
    setActionError(null)
    try {
      await switchLogin(scope, login.accountId)
      setMadeActive('active')
    } catch (e) {
      setActionError(errorText(e))
    }
  }, [scope, login])

  /** Back to the form, the key way chosen (a failed sign-in). */
  const pasteInstead = (): void => {
    setLogin(null)
    setError(null)
    setActionError(null)
    setMethod('key')
  }

  const name = (login?.label || label.trim() || (start.kind === 'relogin' ? start.label : '')).trim()

  // ── Done: an API token was added ────────────────────────────────────
  if (keyDone) {
    return (
      <AccessSheet title="API token added" onClose={onClose} testId="llm-login-sheet">
        <Callout tone="ok" role="status" className="type-hint space-y-0.5" data-testid="llm-login-done">
          <p className="font-medium">
            {display} API token “{keyDone.label}” is saved on {server}.
          </p>
          {keyDone.tail ? <p data-testid="llm-key-tail">Key ending {keyDone.tail}. K2 never shows the whole key again.</p> : null}
        </Callout>
        <p className={DIALOG_MUTED}>
          {display} bills every chat that uses it per token. To use it, make it the server default on Subs / Tokens,
          or pick it for one workspace or chat from its Token menu.
        </p>
        <div className="flex justify-end">
          <Button variant="accent" size="md" onClick={onClose} autoFocus>
            Done
          </Button>
        </div>
      </AccessSheet>
    )
  }

  // ── A sign-in is running or finished ────────────────────────────────
  if (login) {
    const failed = done && !signedIn
    const title = signedIn ? `Signed in to ${display}` : failed ? 'Sign-in didn’t finish' : `Sign in to ${display}`
    const stateWords = LOGIN_STATE_LABELS[login.state] ?? login.state
    // The ×, Esc and a press outside are this step's Cancel (a sign-in under
    // way is cancelled on the server; nothing is saved). Held while starting.
    // Once a sign-in is under way a press outside does nothing: it goes on in
    // the browser, and the click that brings K2 back must not cancel it.
    return (
      <AccessSheet
        title={title}
        onClose={() => (done ? onClose() : void cancel())}
        busy={busy}
        pressOutside={done}
        testId="llm-login-sheet"
      >
        {login.banner && !done ? (
          <Callout tone="warn" className="type-hint" data-testid="llm-login-banner">
            {login.banner}
          </Callout>
        ) : null}

        {!done ? (
          <>
            <div className="flex items-center gap-2" role="status" data-testid="llm-login-state">
              <Spinner />
              <span className={SETTING_ROW_LABEL}>{stateWords}</span>
            </div>
            <div className="space-y-2">
              <Button variant="accent" size="md" className="w-full" disabled={!login.url} onClick={openSignIn} autoFocus={!!login.url}>
                {login.url ? 'Open sign-in page' : 'Waiting for the sign-in page…'}
              </Button>
              {login.url ? (
                <div className="space-y-1.5" data-testid="llm-login-link">
                  <div className="flex items-center justify-between gap-3">
                    <span className={`${DIALOG_MUTED} min-w-0`}>
                      Sign in at{' '}
                      <span className="font-mono text-[var(--color-text-primary)]" data-testid="llm-login-host">
                        {signInHost(login) ?? 'the sign-in page'}
                      </span>
                    </span>
                    <span className="flex shrink-0 gap-2">
                      <Button variant="quiet" onClick={() => setShowLink((v) => !v)} aria-expanded={showLink}>
                        {showLink ? 'Hide full link' : 'Show full link'}
                      </Button>
                      <Button variant="quiet" onClick={copyLink}>
                        {linkCopied ? 'Link copied' : 'Copy link'}
                      </Button>
                    </span>
                  </div>
                  {showLink ? (
                    <Surface
                      chamfer
                      material="tile"
                      className="font-mono type-hint leading-snug text-[var(--color-text-secondary)] px-3 py-2 select-all"
                      style={{ overflowWrap: 'anywhere', wordBreak: 'break-all', userSelect: 'all', whiteSpace: 'normal' }}
                      data-testid="llm-login-url"
                    >
                      {login.url}
                    </Surface>
                  ) : null}
                </div>
              ) : null}
            </div>

            {login.code ? (
              <Surface chamfer material="tile" className="flex items-center justify-between gap-3 px-4 py-2.5">
                <div className="min-w-0">
                  <p className="type-hint text-[var(--color-text-secondary)]">Code to enter on the sign-in page</p>
                  <p className="type-title font-mono tracking-widest text-[var(--color-text-primary)] select-all" data-testid="llm-login-code">
                    {login.code}
                  </p>
                </div>
                <Button className="flex-shrink-0" onClick={copyCode}>
                  {copied ? 'Copied' : 'Copy code'}
                </Button>
              </Surface>
            ) : null}

            {login.state === 'waiting_for_code' || showTerminal ? (
              <div>
                <label htmlFor={ids.paste} className={DIALOG_LABEL}>
                  Code from the sign-in page
                </label>
                <div className="flex items-center gap-2">
                  <Input
                    id={ids.paste}
                    className={`${DIALOG_FIELD} font-mono`}
                    value={paste}
                    onChange={(e) => setPaste(e.target.value)}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter') void sendPaste()
                    }}
                    placeholder="Paste code"
                    aria-label="Paste code"
                    autoComplete="off"
                    spellCheck={false}
                  />
                  <Button className="flex-shrink-0" disabled={!paste.trim()} onClick={() => void sendPaste()}>
                    Send
                  </Button>
                </div>
                <p className={`${DIALOG_MUTED} mt-1`}>After you sign in, the page shows a code. Paste it here.</p>
              </div>
            ) : null}

            {pollError ? (
              <p className="type-hint text-[var(--color-status-warn-amber-soft)]" role="status" data-testid="llm-login-poll-error">
                Lost touch with the server; still trying. The server said: {pollError.replace(/[.\s]+$/, '')}.
              </p>
            ) : null}
          </>
        ) : null}

        {signedIn ? (
          <>
            <Callout tone="ok" role="status" className="type-hint space-y-0.5" data-testid="llm-login-done">
              <p className="font-medium">
                {display} “{name}” is signed in and saved on {server}.
              </p>
              <p>Workspaces and chats can pick it from their Token menu.</p>
            </Callout>
            {/* K5 at first run: the daemon says when to offer it. */}
            <K5Offer scope={scope} onMeet={onClose} />
            {login.offerMakeActive && canChangeDefault && madeActive === null ? (
              <div data-testid="llm-login-offer">
                <p className={SETTING_ROW_LABEL}>Make it the {display} server default?</p>
                <p className={DIALOG_MUTED}>
                  Every chat set to Server default on this server moves to it, and running chats pick it up.
                </p>
              </div>
            ) : null}
            {madeActive === 'active' ? (
              <p className={DIALOG_MUTED} role="status" data-testid="llm-login-made-default">
                It’s now the {display} server default.
              </p>
            ) : null}
          </>
        ) : null}

        {failed ? (
          <FailedNote
            what={login.state === 'timed_out' ? 'The sign-in timed out.' : login.state === 'cancelled' ? 'The sign-in was cancelled.' : `K2 couldn’t sign in to ${display}.`}
            next={
              provider?.apiKeys && !relogin
                ? 'Nothing was saved. Try again, or paste an API token instead.'
                : 'Nothing was saved. Try again, or open Show terminal to see what the sign-in said.'
            }
            detail={login.error || stateWords}
          />
        ) : null}

        {actionError ? (
          <FailedNote what="That didn’t work." next="Try again in a moment." detail={actionError} />
        ) : null}

        {signedIn ? null : (
          <div>
            <Button variant="quiet" onClick={() => setShowTerminal((v) => !v)} aria-expanded={showTerminal}>
              {showTerminal ? 'Hide terminal' : 'Show terminal'}
            </Button>
            {showTerminal ? (
              <pre
                className="mt-2 max-h-48 overflow-auto bg-[var(--color-bg-elevated)] border border-[var(--color-border)] p-2 type-hint font-mono text-[var(--color-text-secondary)] whitespace-pre-wrap break-all"
                data-testid="llm-login-terminal"
              >
                {login.screen.join('\n')}
              </pre>
            ) : null}
          </div>
        )}

        <div className="flex justify-end gap-2 pt-1">
          {!done ? (
            <Button variant="ghost" size="md" onClick={() => void cancel()}>
              Cancel sign-in
            </Button>
          ) : signedIn ? (
            login.offerMakeActive && canChangeDefault && madeActive === null ? (
              <>
                <Button variant="ghost" size="md" onClick={() => setMadeActive('kept')}>
                  Not now
                </Button>
                <Button variant="accent" size="md" onClick={() => void makeActive()} autoFocus>
                  Make it the default
                </Button>
              </>
            ) : (
              <Button variant="accent" size="md" onClick={onClose} autoFocus>
                Done
              </Button>
            )
          ) : (
            <>
              <Button variant="ghost" size="md" onClick={onClose}>
                Close
              </Button>
              {provider?.apiKeys && !relogin ? (
                <Button size="md" onClick={pasteInstead}>
                  Paste an API token instead
                </Button>
              ) : null}
              <Button variant="accent" size="md" onClick={() => void begin()} disabled={busy} autoFocus>
                {busy ? 'Starting…' : 'Try again'}
              </Button>
            </>
          )}
        </div>
      </AccessSheet>
    )
  }

  // ── The form: provider, how, name, then the sign-in or the key ──────
  const ready = !!provider && !busy && (relogin || !!label.trim()) && (how === 'signin' ? !(airgap && !relogin) : !!key.trim())
  const submit = (): void => {
    if (!ready) return
    if (how === 'key') void addKey()
    else void begin()
  }
  const title = start.kind === 'relogin' ? `Sign “${start.label}” in again` : 'Add a token'
  const intro =
    start.kind === 'relogin'
      ? `${display} runs its own sign-in on the server. K2 never sees your password.`
      : `A subscription or API token your agents can use on ${server}. Adding one doesn’t change the server default.`

  return (
    <AccessSheet title={title} intro={intro} onClose={onClose} busy={busy} testId="llm-login-sheet">
      <form
        className="space-y-4"
        onSubmit={(e) => {
          e.preventDefault()
          submit()
        }}
      >
        {!relogin && providers.length > 1 ? (
          <fieldset className="space-y-2">
            <legend className={DIALOG_LABEL}>Provider</legend>
            <div role="radiogroup" aria-label="Provider" className="space-y-2">
              {providers.map((p) => (
                <label key={p.tool} className="flex items-start gap-2 cursor-pointer" data-testid={`llm-provider-${p.tool}`}>
                  <Radio
                    name="llm-provider"
                    className="mt-0.5"
                    checked={provider?.tool === p.tool}
                    aria-label={`${p.display}: ${providerHint(p)}`}
                    onChange={() => setTool(p.tool)}
                  />
                  <span>
                    <span className={`${SETTING_ROW_LABEL} inline-flex items-center gap-1.5`}>
                      <AgentIcon agent={p.display} size={14} />
                      {p.display}
                    </span>
                    <span className={`block ${DIALOG_MUTED}`}>{providerHint(p)}</span>
                  </span>
                </label>
              ))}
            </div>
          </fieldset>
        ) : null}
        {!relogin && providers.length === 0 ? (
          <p className={DIALOG_MUTED} data-testid="llm-no-providers">
            This server has no provider K2 can save a token for yet.
          </p>
        ) : null}

        {provider && !relogin && provider.subscription && provider.apiKeys ? (
          <fieldset className="space-y-2">
            <legend className={DIALOG_LABEL}>How</legend>
            <div role="radiogroup" aria-label="How to add it" className="space-y-2">
              <label className="flex items-start gap-2 cursor-pointer">
                <Radio
                  name="llm-method"
                  className="mt-0.5"
                  checked={how === 'signin'}
                  disabled={airgap}
                  aria-label="Sign in with a subscription"
                  onChange={() => setMethod('signin')}
                />
                <span>
                  <span className={SETTING_ROW_LABEL}>Sign in with a subscription</span>
                  <span className={`block ${DIALOG_MUTED}`}>
                    {airgap ? 'Off while air-gap mode is on.' : `Your ${provider.display} plan and its usage limits. Opens ${provider.display}’s sign-in page.`}
                  </span>
                </span>
              </label>
              <label className="flex items-start gap-2 cursor-pointer">
                <Radio name="llm-method" className="mt-0.5" checked={how === 'key'} aria-label="Paste an API token" onChange={() => setMethod('key')} />
                <span>
                  <span className={SETTING_ROW_LABEL}>Paste an API token</span>
                  <span className={`block ${DIALOG_MUTED}`}>{provider.display} bills it per token.</span>
                </span>
              </label>
            </div>
          </fieldset>
        ) : null}

        {provider && !relogin ? (
          <Field label="Name" htmlFor={ids.label} hint="So you can tell your tokens apart, like work or personal.">
            <Input
              id={ids.label}
              autoFocus={start.kind !== 'new'}
              className={DIALOG_FIELD}
              value={label}
              onChange={(e) => setLabel(e.target.value)}
              data-testid="llm-login-label"
              autoComplete="off"
            />
          </Field>
        ) : null}

        {provider && how === 'signin' ? (
          <div>
            <p className={DIALOG_LABEL}>Sign in from</p>
            <Segmented<LoginMode>
              aria-label="Sign in from"
              value={mode}
              onChange={setMode}
              options={[
                { value: 'this_computer', label: 'This computer' },
                { value: 'other_device', label: 'Another device' },
              ]}
            />
            <p className={`${DIALOG_MUTED} mt-1`}>
              {mode === 'this_computer'
                ? 'The sign-in page opens in a browser on the computer that runs this server.'
                : 'Open the sign-in page on this device, phone or laptop, then paste back the code it shows.'}
            </p>
            {!relogin ? (
              <p className={`${DIALOG_MUTED} mt-1`}>{provider.display} runs its own sign-in on the server. K2 never sees your password.</p>
            ) : null}
          </div>
        ) : null}

        {provider && how === 'key' ? (
          <Field label="API token" htmlFor={ids.key} hint={`From your ${provider.display} account. K2 keeps it on this server and never shows it again.`}>
            <div className="flex items-center gap-2">
              <Input
                id={ids.key}
                type={keyShown ? 'text' : 'password'}
                autoComplete="off"
                spellCheck={false}
                className={`${DIALOG_FIELD} font-mono`}
                value={key}
                onChange={(e) => setKey(e.target.value)}
                aria-invalid={error ? true : undefined}
                data-testid="llm-key-input"
              />
              <Button variant="quiet" className="flex-shrink-0" onClick={() => setKeyShown((s) => !s)} aria-pressed={keyShown}>
                {keyShown ? 'Hide' : 'Show'}
              </Button>
            </div>
          </Field>
        ) : null}

        {error ? (
          how === 'key' ? (
            <FailedNote what="K2 couldn’t add the API token." next="Nothing was saved. Check that you copied the whole token, paste it again, then try again." detail={error} />
          ) : (
            <FailedNote what="The sign-in didn’t start." next="Nothing was saved. Try again in a moment." detail={error} />
          )
        ) : null}

        <div className="flex justify-end gap-2 pt-1">
          <Button variant="ghost" size="md" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          {how === 'key' ? (
            <Button type="submit" variant="accent" size="md" disabled={!ready} data-testid="llm-key-add">
              {busy ? 'Adding…' : 'Add API token'}
            </Button>
          ) : (
            <Button type="submit" variant="accent" size="md" disabled={!ready} data-testid="llm-login-start">
              {busy ? 'Starting…' : 'Start sign-in'}
            </Button>
          )}
        </div>
      </form>
    </AccessSheet>
  )
}

function rank(tool: string): number {
  const i = ORDER.indexOf(tool)
  return i < 0 ? ORDER.length : i
}
