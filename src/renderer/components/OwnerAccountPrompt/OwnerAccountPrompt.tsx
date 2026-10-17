// "Your account" setup prompt (0.46.0, Rosson 2026-10-17). An owner with
// no linked account now sends as `[from owner]` (it used to be the old
// display name), so K2 asks once which Admin Access login is theirs.
//
// The daemon owns the truth (`GET /cli/owner/account-prompt`): it answers
// `show: true` only for the owner token while no account is linked.
//   - `mode: "pick"` (enabled Owner/Admin logins exist, and "Not now" was
//     not chosen for this version): pick one, or "Create a new account"
//     (Back returns to the list; Not now stays). The daemon picks the
//     obvious pre-selection (the only admin, or the one whose first/last
//     name matches the old "Your name").
//   - `mode: "create"` (no Owner/Admin login at all): the create form is
//     required; no Not now, and Esc or a click outside don't close it.
// Both cases share one form (`CreateAdminForm`): `users/add`, `users/set-role`
// admin, then the link. No localStorage gating: every window and client
// reads the same answer.
//
// This client only shows it in the main window of a local connection (the
// owner on the server's own computer). "Use this account" writes the
// existing link (`POST /cli/owner/accounts`, the same route as Settings →
// General → Your account); "Not now" stores the dismissed version
// (`POST /cli/owner/account-prompt/dismiss`), so it asks again only after
// the version changes. Both hide it everywhere: other windows re-read on
// focus.

import { useCallback, useEffect, useRef, useState } from 'react'
import { Button, DialogFrame, DialogScrim } from '@/components/ui'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { getWindowLabel } from '@/lib/window-chrome'
import { setOwnerAccounts } from '@/components/Settings/sections/notify-api'
import { CHAMFER_CLIP, resolvePersonColor } from '@/components/Presence/PresenceChip'
import { PERSON_NAME_MAX, realName } from '@/lib/person-name'
import { PersonColorPicker } from '@/components/Presence/PersonColorPicker'

export const OWNER_ACCOUNT_PROMPT_PATH = 'owner/account-prompt'
export const OWNER_ACCOUNT_PROMPT_DISMISS_PATH = 'owner/account-prompt/dismiss'

export const PROMPT_TITLE = 'Which account is yours?'
export const PROMPT_WHY = 'Agents see your messages as from this user.'

export interface PromptCandidate {
  username: string
  role: string
  firstName: string | null
  lastName: string | null
  displayName: string
  color: string | null
}

export interface PromptView {
  show: boolean
  /** `pick` an existing Owner/Admin login (Not now allowed), or `create`
   *  one: there are none, and the prompt can't be dismissed until one is
   *  made (Rosson 2026-10-17). */
  mode: 'pick' | 'create'
  candidates: PromptCandidate[]
  preselect: string | null
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() !== '' ? v : null
}

export function parsePromptView(raw: unknown): PromptView {
  const o = raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
  const candidates: PromptCandidate[] = []
  for (const c of Array.isArray(o.candidates) ? o.candidates : []) {
    if (!c || typeof c !== 'object') continue
    const r = c as Record<string, unknown>
    const username = str(r.username)
    if (!username) continue
    candidates.push({
      username,
      role: str(r.role) ?? 'admin',
      firstName: str(r.firstName),
      lastName: str(r.lastName),
      displayName: str(r.displayName) ?? username,
      color: str(r.color),
    })
  }
  const preselect = str(o.preselect)
  const mode = o.mode === 'create' ? 'create' : 'pick'
  return {
    show: o.show === true && (mode === 'create' || candidates.length > 0),
    mode,
    candidates,
    preselect: preselect && candidates.some((c) => c.username === preselect) ? preselect : null,
  }
}

/** The local owner in the main window: the only place it may show. */
export function promptEligible(): boolean {
  return getWindowLabel() === 'main' && !primaryScope().isRemote
}

/** The daemon's answer, or null when this server can't say (an older
 *  daemon without the route, or a login that isn't the owner token). */
export async function fetchPromptView(): Promise<PromptView | null> {
  try {
    return parsePromptView(await daemonCliGet<unknown>(primaryScope(), OWNER_ACCOUNT_PROMPT_PATH))
  } catch (e) {
    console.debug('[owner-account-prompt] not shown:', e)
    return null
  }
}

export const CREATE_TITLE = 'Create your admin user'

/** Admin Access's username rule: trimmed and lower-cased (the daemon
 *  validates the rest). */
export function normalizeNewUsername(raw: string): string {
  return raw.trim().toLowerCase()
}

/** The no-admins form: the existing routes in order. `users/add` (with
 *  the optional names), `users/set-role` admin, then link it as the
 *  owner's account (`owner/accounts`). Colour is the owner's own picker
 *  (`people/color`, saved as you pick; Automatic by default), which the
 *  linked login shows. Every daemon error stays inline. */
function CreateAdminForm({
  onDone,
  onBack,
  notNow,
}: {
  onDone: () => void
  /** Has-admins case: back to the list. */
  onBack?: () => void
  /** Has-admins case: the Not now button (absent when it's required). */
  notNow?: React.ReactNode
}): React.JSX.Element {
  const [username, setUsername] = useState('')
  const [password, setPassword] = useState('')
  const [confirm, setConfirm] = useState('')
  const [firstName, setFirstName] = useState('')
  const [lastName, setLastName] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  // A user made but not yet linked (a later step failed): retry links it.
  const created = useRef<string | null>(null)
  const mismatch = confirm !== '' && password !== confirm
  const name = normalizeNewUsername(username)
  const ready = name !== '' && password !== '' && password === confirm && !busy

  const submit = async (): Promise<void> => {
    if (!ready) return
    setBusy(true)
    setErr(null)
    const scope = primaryScope()
    try {
      if (created.current !== name) {
        const body: Record<string, unknown> = { username: name, password }
        if (firstName.trim()) body.firstName = firstName.trim()
        if (lastName.trim()) body.lastName = lastName.trim()
        await daemonCliPost(scope, 'users/add', body)
        created.current = name
      }
      await daemonCliPost(scope, 'users/set-role', { username: name, role: 'admin' })
      await setOwnerAccounts([name], scope)
      onDone()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const input =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)] disabled:opacity-50'
  const field = (label: string, el: React.ReactNode): React.JSX.Element => (
    <label className="flex items-center gap-3 py-1.5">
      <span className="w-28 flex-shrink-0 text-xs text-[var(--color-text-secondary)]">{label}</span>
      <span className="flex-1 min-w-0">{el}</span>
    </label>
  )

  return (
    <form
      className="mt-3 flex flex-col"
      data-testid="owner-account-create"
      onSubmit={(e) => {
        e.preventDefault()
        void submit()
      }}
    >
      {field(
        'Username',
        <input
          className={input}
          aria-label="Username"
          autoCapitalize="none"
          autoCorrect="off"
          spellCheck={false}
          value={username}
          disabled={busy || created.current !== null}
          onChange={(e) => setUsername(e.target.value)}
        />,
      )}
      {field(
        'Password',
        <input className={input} aria-label="Password" type="password" autoComplete="new-password" value={password} disabled={busy} onChange={(e) => setPassword(e.target.value)} />,
      )}
      {field(
        'Confirm',
        <input className={input} aria-label="Confirm password" type="password" autoComplete="new-password" value={confirm} disabled={busy} onChange={(e) => setConfirm(e.target.value)} />,
      )}
      {mismatch ? <p className="m-0 text-[11px] text-[var(--color-status-error-soft)]">The passwords don’t match.</p> : null}
      {field(
        'First name',
        <input className={input} aria-label="First name" placeholder="Optional" maxLength={PERSON_NAME_MAX} value={firstName} disabled={busy} onChange={(e) => setFirstName(e.target.value)} />,
      )}
      {field(
        'Last name',
        <input className={input} aria-label="Last name" placeholder="Optional" maxLength={PERSON_NAME_MAX} value={lastName} disabled={busy} onChange={(e) => setLastName(e.target.value)} />,
      )}
      <div className="flex items-center gap-3 py-1.5" data-testid="owner-account-create-color">
        <span className="w-28 flex-shrink-0 text-xs text-[var(--color-text-secondary)]">Colour</span>
        <span className="flex-1 min-w-0">
          <PersonColorPicker scope={primaryScope()} />
        </span>
      </div>
      {err ? (
        <p role="alert" className="m-0 mt-2 text-[11px] text-[var(--color-status-error-soft)]">
          {err}
        </p>
      ) : null}
      <div className="mt-4 flex items-center justify-end gap-2">
        {onBack ? (
          <button
            type="button"
            data-testid="owner-account-back"
            onClick={onBack}
            disabled={busy}
            className="mr-auto text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:underline cursor-pointer disabled:opacity-50"
          >
            ← Back
          </button>
        ) : null}
        {notNow}
        <Button type="submit" variant="accent" size="md" disabled={!ready}>
          {busy ? 'Creating…' : 'Create and use this account'}
        </Button>
      </div>
    </form>
  )
}

export default function OwnerAccountPrompt(): React.JSX.Element | null {
  const [view, setView] = useState<PromptView | null>(null)
  const [picked, setPicked] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const [creating, setCreating] = useState(false)
  const eligible = useRef(promptEligible())

  const refresh = useCallback(async () => {
    if (!eligible.current) return
    const v = await fetchPromptView()
    setView((prev) => {
      // Keep the person's own pick across re-reads.
      if (!prev?.show && v?.show) setPicked(v.preselect)
      return v
    })
  }, [])

  useEffect(() => {
    void refresh()
    const onVisible = (): void => {
      if (document.visibilityState === 'visible') void refresh()
    }
    window.addEventListener('focus', onVisible)
    document.addEventListener('visibilitychange', onVisible)
    return () => {
      window.removeEventListener('focus', onVisible)
      document.removeEventListener('visibilitychange', onVisible)
    }
  }, [refresh])

  if (!view?.show) return null

  const hide = (): void => setView((v) => (v ? { ...v, show: false } : v))
  // No Owner/Admin login yet: the create form is required (no Not now;
  // the scrim and Esc don't close it, since nothing here listens for them).
  const required = view.mode === 'create'
  const showForm = required || creating

  const useAccount = async (): Promise<void> => {
    if (!picked || busy) return
    setBusy(true)
    setErr(null)
    try {
      await setOwnerAccounts([picked])
      hide()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const notNow = async (): Promise<void> => {
    if (busy) return
    setBusy(true)
    setErr(null)
    try {
      await daemonCliPost(primaryScope(), OWNER_ACCOUNT_PROMPT_DISMISS_PATH, {})
      hide()
    } catch (e) {
      setErr(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  const notNowButton = required ? null : (
    <Button variant="ghost" size="md" disabled={busy} data-testid="owner-account-not-now" onClick={() => void notNow()}>
      Not now
    </Button>
  )

  return (
    <>
      <DialogScrim data-testid="owner-account-prompt-scrim" />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby="owner-account-prompt-title"
        data-testid="owner-account-prompt"
        data-mode={view.mode}
        style={{ width: 'min(440px, calc(100vw - 32px))', padding: '16px 18px' }}
      >
        <h2 id="owner-account-prompt-title" className="m-0 text-[14px] font-semibold text-[var(--color-text-primary)]">
          {showForm ? CREATE_TITLE : PROMPT_TITLE}
        </h2>
        <p className="m-0 mt-1 text-xs text-[var(--color-text-secondary)]">{PROMPT_WHY}</p>
        {showForm ? (
          <>
            <CreateAdminForm onDone={hide} onBack={required ? undefined : () => setCreating(false)} notNow={notNowButton} />
            {err ? (
              <p role="alert" className="m-0 mt-2 text-[11px] text-[var(--color-status-error-soft)]">
                {err}
              </p>
            ) : null}
          </>
        ) : (
          <>
            <div role="radiogroup" aria-label="Your account" className="mt-3 flex flex-col border-t border-[var(--color-border)]">
              {view.candidates.map((c) => {
                const on = picked === c.username
                const real = realName(c)
                return (
                  <label
                    key={c.username}
                    data-testid={`owner-account-option-${c.username}`}
                    className="flex items-center gap-3 py-2 border-b border-[var(--color-border)] cursor-pointer"
                  >
                    <input
                      type="radio"
                      name="owner-account"
                      value={c.username}
                      checked={on}
                      onChange={() => setPicked(c.username)}
                      className="accent-[var(--color-accent)]"
                    />
                    <span
                      aria-hidden
                      data-testid={`owner-account-dot-${c.username}`}
                      className="inline-block h-3 w-3 flex-none"
                      style={{ background: resolvePersonColor(c.username, c.color), clipPath: CHAMFER_CLIP }}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="text-xs text-[var(--color-text-primary)]">{real ?? c.username}</span>
                      {real ? <span className="ml-2 text-[11px] font-mono text-[var(--color-text-muted)]">{c.username}</span> : null}
                    </span>
                    <span className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">{c.role}</span>
                  </label>
                )
              })}
              <button
                type="button"
                data-testid="owner-account-create-option"
                onClick={() => {
                  setErr(null)
                  setCreating(true)
                }}
                className="flex items-center gap-3 py-2 border-b border-[var(--color-border)] text-left text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer"
              >
                <span aria-hidden className="w-[13px] text-center">+</span>
                Create a new account
              </button>
            </div>
            {err ? (
              <p role="alert" className="m-0 mt-2 text-[11px] text-[var(--color-status-error-soft)]">
                {err}
              </p>
            ) : null}
            <div className="mt-4 flex justify-end gap-2">
              {notNowButton}
              <Button variant="accent" size="md" disabled={busy || !picked} onClick={() => void useAccount()}>
                Use this account
              </Button>
            </div>
          </>
        )}
      </DialogFrame>
    </>
  )
}

