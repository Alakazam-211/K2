// Settings → User Access (0.46.6): the App users list's per-person password
// actions and the Add user sheet. Thin client: each action is one daemon
// route, and the daemon decides who may and says why not.
//
// - Reset password → POST skin/users/reset-link (the owner's reset): `code`
//   for a one-time link shown here, or `to` (+ `from`) to mail it. Using it
//   ends the person's sessions and drops an agent's tie.
// - Set password → POST skin/users/password: typed, or `generate: true`
//   (the daemon makes it and returns it once). Ends their sessions.
// - Unlock → POST skin/users/unlock (PeopleSection calls it from the row).
// - Add user → POST skin/users (typed or `generate: true`) + full name, or
//   POST skin/users/invite (a link to set their own; needs an email and an
//   App).
//
// A one-time link or password lives only in the open sheet's state: closing
// the sheet unmounts it and drops it. Never logged, never sent to a Thread.

import React, { useEffect, useId, useState } from 'react'
import { daemonErrorText } from '@/lib/daemon-error'
import { Button, DialogFrame, DialogScrim, Input, Radio } from '@/components/ui'
import { SettingDropdown } from '../controls/SettingControls'
import { settingMenuOpen } from '../settings-esc'
import type { PersonRow } from './user-access-api'

/** POST one `/cli/*` route on the window's server (PeopleSection passes it). */
export type DaemonPost = (route: string, body: Record<string, unknown>) => Promise<unknown>

const FIELD_CLS = 'w-full type-control'
const NOTE = 'type-hint leading-relaxed text-[var(--color-text-secondary)]'
const MUTED = 'type-hint leading-relaxed text-[var(--color-text-secondary)]'

function rec(raw: unknown): Record<string, unknown> {
  return raw && typeof raw === 'object' && !Array.isArray(raw) ? (raw as Record<string, unknown>) : {}
}

function str(v: unknown): string | null {
  return typeof v === 'string' && v.trim() ? v.trim() : null
}

/** "in 24 hours" / "on Oct 30, 14:05" for an epoch-seconds expiry. */
export function expiryWords(expiresAt: number | null, ttlSecs: number | null): string | null {
  if (ttlSecs && ttlSecs > 0) {
    const hours = Math.round(ttlSecs / 3600)
    if (hours >= 1) return `It expires in ${hours} hour${hours === 1 ? '' : 's'}.`
    return `It expires in ${Math.max(1, Math.round(ttlSecs / 60))} minutes.`
  }
  if (expiresAt) return `It expires ${new Date(expiresAt * 1000).toLocaleString()}.`
  return null
}

function Sheet({
  title,
  onClose,
  children,
  testId,
}: {
  title: string
  onClose: () => void
  children: React.ReactNode
  testId: string
}): React.JSX.Element {
  const titleId = useId()
  useEffect(() => {
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        // An open K2 menu (App, Role) closes first; the dialog stays.
        if (settingMenuOpen()) return
        e.preventDefault()
        e.stopPropagation()
        onClose()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])
  return (
    <>
      <DialogScrim onMouseDown={onClose} />
      <DialogFrame
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        data-testid={testId}
        className="w-[min(30rem,calc(100vw-2.5rem))] max-h-[calc(100vh-3rem)] flex flex-col"
      >
        {/* The frame stays still (its chamfer); the content scrolls inside. */}
        <div className="min-h-0 overflow-y-auto p-4 space-y-3">
          <h3 id={titleId} className="type-heading text-[var(--color-text-primary)]">
            {title}
          </h3>
          {children}
        </div>
      </DialogFrame>
    </>
  )
}

/**
 * A password someone else set is temporary (0.46.6 must-change): said
 * wherever Settings shows or confirms one, when the daemon says so
 * (`passwordMustChange` on the reply).
 */
export const MUST_CHANGE_NOTE = 'It’s temporary: the next time they sign in, K2 asks them to choose their own.'

/** A secret shown once: the value, Copy, and how long it lasts. */
export function OneTimeSecret({
  label,
  value,
  expiry,
  copyLabel,
}: {
  label: string
  value: string
  expiry?: string | null
  copyLabel: string
}): React.JSX.Element {
  const [copy, setCopy] = useState<'idle' | 'done' | 'failed'>('idle')
  const doCopy = (): void => {
    const clip = typeof navigator !== 'undefined' ? navigator.clipboard : undefined
    if (!clip) {
      setCopy('failed')
      return
    }
    clip.writeText(value).then(
      () => setCopy('done'),
      () => setCopy('failed'),
    )
  }
  return (
    <div data-k2-chamfer="surface" className="space-y-1.5 border border-[var(--color-border)] rounded-[var(--radius-box)] p-3">
      <p className={NOTE}>{label}</p>
      <p data-testid="one-time-secret" className="type-row font-mono break-all select-all text-[var(--color-text-primary)]">
        {value}
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <Button aria-label={copyLabel} onClick={doCopy}>
          {copy === 'done' ? 'Copied' : 'Copy'}
        </Button>
        {copy === 'failed' ? <span className={MUTED}>Couldn’t copy. Select the text and copy it yourself.</span> : null}
      </div>
      {expiry ? <p className={MUTED}>{expiry}</p> : null}
      <p className={MUTED}>K2 won’t show this again. Once you close this window, it’s gone.</p>
    </div>
  )
}

function ErrorLine({ text }: { text: string | null }): React.JSX.Element | null {
  if (!text) return null
  return (
    <p role="alert" className="type-hint leading-relaxed text-[var(--color-status-error-soft)]">
      {text}
    </p>
  )
}

function tieLine(person: PersonRow): string | null {
  if (!person.tiedTo) return null
  return person.pendingReset
    ? `The ${person.tiedTo} agent set ${person.username} up, so the extra access you gave starts once they choose their own password, here or in the App.`
    : `The ${person.tiedTo} agent set ${person.username} up, so they reach only ${person.tiedTo} until they choose their own password, here or in the App.`
}

type ResetResult =
  | { kind: 'link'; value: string; expiry: string | null }
  | { kind: 'code'; value: string; expiry: string | null; hint: string | null }
  | { kind: 'sent'; to: string; from: string | null; expiry: string | null }
  | { kind: 'held'; hint: string }

/** Reset password: a one-time link for you to pass on, or the link by email. */
export function ResetPasswordDialog({
  person,
  onClose,
  post,
}: {
  person: PersonRow
  onClose: () => void
  post: DaemonPost
}): React.JSX.Element {
  // The stored email is offered only when nothing could have chosen it
  // (no agent tie, and the daemon doesn't refuse it).
  const storedOk = !person.tiedTo && !person.storedEmailRefused && person.email ? person.email : ''
  const [mode, setMode] = useState<'link' | 'email'>('link')
  const [to, setTo] = useState(storedOk)
  const [from, setFrom] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [result, setResult] = useState<ResetResult | null>(null)

  const run = async (body: Record<string, unknown>): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const r = rec(await post('skin/users/reset-link', { username: person.username, ...body }))
      const expiresAt = typeof r.expiresAt === 'number' ? r.expiresAt : null
      const ttl = typeof r.ttlSecs === 'number' ? r.ttlSecs : null
      const expiry = expiryWords(expiresAt, ttl)
      if (r.delivered === 'returned' && str(r.link)) {
        setResult({ kind: 'link', value: str(r.link) as string, expiry })
      } else if (r.delivered === 'returned' && str(r.code)) {
        setResult({ kind: 'code', value: str(r.code) as string, expiry, hint: str(r.hint) })
      } else if (r.delivered === 'held') {
        setResult({ kind: 'held', hint: str(r.hint) ?? 'The email is waiting for approval.' })
      } else {
        setResult({ kind: 'sent', to: str(r.sentTo) ?? to.trim(), from: str(r.from), expiry })
      }
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  const tie = tieLine(person)
  return (
    <Sheet title={`Reset password for ${person.username}`} onClose={onClose} testId="reset-password-dialog">
      {result ? (
        <>
          {result.kind === 'link' ? (
            <OneTimeSecret
              label={`Give ${person.username} this link. It works once: they open it and choose a new password on K2’s own page.`}
              value={result.value}
              expiry={result.expiry}
              copyLabel="Copy reset link"
            />
          ) : result.kind === 'code' ? (
            <OneTimeSecret
              label={`This server has no public web address yet, so there’s no link: give ${person.username} this one-time code instead.`}
              value={result.value}
              expiry={result.expiry}
              copyLabel="Copy reset code"
            />
          ) : result.kind === 'sent' ? (
            <p className={NOTE}>
              Reset link sent to {result.to}
              {result.from ? ` from ${result.from}` : ''}. It works once. {result.expiry ?? ''}
            </p>
          ) : (
            <p className={NOTE}>{result.hint}</p>
          )}
          <div className="flex justify-end">
            <Button variant="accent" onClick={onClose}>
              Done
            </Button>
          </div>
        </>
      ) : (
        <>
          <p className={NOTE}>
            When {person.username} uses the link, they choose a new password on a K2 page (not inside the App).
            Then they’re signed out of every App and device, and any lock from wrong passwords clears.
          </p>
          {tie ? <p className={NOTE}>{tie}</p> : null}
          <div role="radiogroup" aria-label="How to reset" className="space-y-1.5">
            <label className="flex items-start gap-2 type-control text-[var(--color-text-primary)] cursor-pointer">
              <Radio name="reset-mode" checked={mode === 'link'} onChange={() => setMode('link')} />
              <span>
                Get a one-time link
                <span className={`block ${MUTED}`}>You pass it on yourself, so no email is involved. K2 shows it once.</span>
              </span>
            </label>
            <label className="flex items-start gap-2 type-control text-[var(--color-text-primary)] cursor-pointer">
              <Radio name="reset-mode" checked={mode === 'email'} onChange={() => setMode('email')} />
              <span>
                Email them the link
                <span className={`block ${MUTED}`}>K2 sends it to an address you give.</span>
              </span>
            </label>
          </div>
          {mode === 'email' ? (
            <div className="space-y-2">
              {person.email && !storedOk ? (
                <p className={MUTED}>
                  The email on file ({person.email}) may have been entered by an agent, so K2 won’t send a reset
                  there. Enter an address you know is theirs.
                </p>
              ) : null}
              <label className="block space-y-1">
                <span className={MUTED}>Their email</span>
                <Input
                  type="email"
                  className={FIELD_CLS}
                  aria-label={`Email for ${person.username}`}
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  value={to}
                  onChange={(e) => setTo(e.target.value)}
                />
              </label>
              <label className="block space-y-1">
                <span className={MUTED}>
                  Send from (optional): the address the email comes from. Pick one no agent can read, so no agent
                  sees the link. Leave it blank to use K2’s own sender, if this server has one.
                </span>
                <Input
                  type="email"
                  className={FIELD_CLS}
                  aria-label="Send the reset from"
                  autoCapitalize="none"
                  autoCorrect="off"
                  spellCheck={false}
                  value={from}
                  onChange={(e) => setFrom(e.target.value)}
                />
              </label>
            </div>
          ) : null}
          <ErrorLine text={error} />
          <div className="flex justify-end gap-2">
            <Button variant="quiet" onClick={onClose}>
              Cancel
            </Button>
            {mode === 'link' ? (
              <Button variant="accent" disabled={busy} onClick={() => void run({ code: true })}>
                Get link
              </Button>
            ) : (
              <Button
                variant="accent"
                disabled={busy || !to.trim()}
                onClick={() => void run({ to: to.trim(), ...(from.trim() ? { from: from.trim() } : {}) })}
              >
                Send link
              </Button>
            )}
          </div>
        </>
      )}
    </Sheet>
  )
}

/** Set password: typed (show/hide) or generated by K2, shown once. */
export function SetPasswordDialog({
  person,
  onClose,
  onReset,
  post,
}: {
  person: PersonRow
  onClose: () => void
  post: DaemonPost
  /** Switch to Reset password (for a person an agent set up). */
  onReset: () => void
}): React.JSX.Element {
  const [password, setPassword] = useState('')
  const [shown, setShown] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [done, setDone] = useState<{ generated: string | null; mustChange: boolean } | null>(null)

  const submit = async (generate: boolean): Promise<void> => {
    setBusy(true)
    setError(null)
    try {
      const body = generate ? { username: person.username, generate: true } : { username: person.username, password }
      const r = rec(await post('skin/users/password', body))
      setPassword('')
      setDone({ generated: generate ? str(r.generatedPassword) : null, mustChange: r.passwordMustChange === true })
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Sheet title={`Set password for ${person.username}`} onClose={onClose} testId="set-password-dialog">
      {done ? (
        <>
          {done.generated ? (
            <OneTimeSecret
              label={`New password for ${person.username}. Give it to them yourself. They’re signed out of every App and device.${done.mustChange ? ` ${MUST_CHANGE_NOTE}` : ''}`}
              value={done.generated}
              copyLabel="Copy password"
            />
          ) : (
            <p className={NOTE}>
              Password set. Give it to {person.username} yourself. They’re signed out of every App and device.
              {done.mustChange ? ` ${MUST_CHANGE_NOTE}` : ''}
            </p>
          )}
          <div className="flex justify-end">
            <Button variant="accent" onClick={onClose}>
              Done
            </Button>
          </div>
        </>
      ) : (
        <>
          <p className={NOTE}>
            Setting a password signs {person.username} out everywhere: every App and every device. It doesn’t
            clear a lock from wrong passwords; use Unlock for that.
          </p>
          {person.tiedTo ? (
            <p className={NOTE}>
              The {person.tiedTo} agent set {person.username} up, so they reach only {person.tiedTo}. Setting a password
              here doesn’t lift that limit; only a reset link they use does.
            </p>
          ) : null}
          {person.tiedTo ? (
            <div>
              <Button onClick={onReset}>Reset password instead…</Button>
            </div>
          ) : null}
          <form
            className="space-y-2"
            onSubmit={(e) => {
              e.preventDefault()
              if (password.trim()) void submit(false)
            }}
          >
            <div className="flex items-center gap-2">
              <Input
                type={shown ? 'text' : 'password'}
                className={FIELD_CLS}
                aria-label={`New password for ${person.username}`}
                autoComplete="new-password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
              />
              <Button variant="quiet" className="flex-shrink-0" onClick={() => setShown((s) => !s)}>
                {shown ? 'Hide' : 'Show'}
              </Button>
            </div>
            <ErrorLine text={error} />
            <div className="flex flex-wrap justify-end gap-2">
              <Button variant="quiet" onClick={onClose}>
                Cancel
              </Button>
              <Button disabled={busy} onClick={() => void submit(true)}>
                Generate a password and set it
              </Button>
              <Button type="submit" variant="accent" disabled={busy || !password.trim()}>
                Set password and sign them out
              </Button>
            </div>
          </form>
        </>
      )}
    </Sheet>
  )
}

export type AppChoice = { value: string; label: string }
export type RoleChoice = { value: string; label: string; appId: string | null }

/** What Add user did: the new username, and a secret to show once. */
export type AddedUser = {
  username: string
  secret: { kind: 'password' | 'link'; value: string; expiry: string | null } | null
  note: string | null
}

/** Add user: username, full name, email, and how they get a password. */
export function AddUserDialog({
  onClose,
  onAdded,
  apps,
  roles,
  loadApps,
  post,
}: {
  post: DaemonPost
  onClose: () => void
  onAdded: (added: AddedUser) => void
  apps: AppChoice[]
  roles: RoleChoice[]
  /** Read the box's Apps (only when the invite option is picked). */
  loadApps: () => Promise<void>
}): React.JSX.Element {
  const [username, setUsername] = useState('')
  const [fullName, setFullName] = useState('')
  const [email, setEmail] = useState('')
  const [mode, setMode] = useState<'type' | 'generate' | 'invite'>('type')
  const [password, setPassword] = useState('')
  const [shown, setShown] = useState(false)
  const [app, setApp] = useState('')
  const [role, setRole] = useState('')
  const [from, setFrom] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const hasEmail = email.trim().length > 0
  // "Send a link" needs an email: drop back when it is cleared.
  useEffect(() => {
    if (!hasEmail && mode === 'invite') setMode('type')
  }, [hasEmail, mode])
  useEffect(() => {
    if (mode !== 'invite') return
    loadApps().catch((e) => setError(daemonErrorText(e)))
  }, [mode, loadApps])

  const appRoles = roles.filter((r) => !r.appId || r.appId === app)
  const ready =
    username.trim().length > 0 &&
    (mode === 'generate' || (mode === 'type' && password.trim().length > 0) || (mode === 'invite' && hasEmail && app !== ''))

  const submit = async (): Promise<void> => {
    const name = username.trim().toLowerCase()
    const mail = email.trim()
    const full = fullName.trim()
    setBusy(true)
    setError(null)
    try {
      let added: AddedUser
      if (mode === 'invite') {
        const r = rec(
          await post('skin/users/invite', {
            username: name,
            app,
            email: mail,
            ...(role ? { role } : {}),
            ...(from.trim() ? { from: from.trim() } : {}),
          }),
        )
        const expiry = expiryWords(typeof r.expiresAt === 'number' ? r.expiresAt : null, typeof r.ttlSecs === 'number' ? r.ttlSecs : null)
        if (r.delivered === 'returned' && str(r.link)) {
          added = { username: name, secret: { kind: 'link', value: str(r.link) as string, expiry }, note: null }
        } else if (r.delivered === 'held') {
          added = { username: name, secret: null, note: str(r.hint) ?? 'The invite is waiting for approval.' }
        } else {
          added = { username: name, secret: null, note: `Invite sent to ${str(r.sentTo) ?? mail}. The link works once. ${expiry ?? ''}`.trim() }
        }
      } else {
        const body: Record<string, unknown> = { username: name }
        if (mode === 'generate') body.generate = true
        else body.password = password
        if (mail) body.email = mail
        const r = rec(await post('skin/users', body))
        const generated = mode === 'generate' ? str(r.generatedPassword) : null
        const mustChange = r.passwordMustChange === true ? MUST_CHANGE_NOTE : null
        added = { username: name, secret: generated ? { kind: 'password', value: generated, expiry: null } : null, note: mustChange }
      }
      if (full) {
        try {
          await post('skin/users/full-name', { username: name, fullName: full })
        } catch (e) {
          // The person exists; say the name didn't save rather than hide it.
          added = { ...added, note: `${added.note ? `${added.note} ` : ''}The full name didn’t save: ${daemonErrorText(e)}` }
        }
      }
      setPassword('')
      onAdded(added)
    } catch (e) {
      setError(daemonErrorText(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Sheet title="Add person" onClose={onClose} testId="add-user-dialog">
      <form
        className="space-y-2.5"
        onSubmit={(e) => {
          e.preventDefault()
          if (ready && !busy) void submit()
        }}
      >
        <label className="block space-y-1">
          <span className={MUTED}>Username. Lowercase; it can’t be changed later.</span>
          <Input
            className={FIELD_CLS}
            aria-label="New user’s username"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
        </label>
        <label className="block space-y-1">
          <span className={MUTED}>Full name, if you like</span>
          <Input className={FIELD_CLS} aria-label="New user’s full name" value={fullName} onChange={(e) => setFullName(e.target.value)} />
        </label>
        <label className="block space-y-1">
          <span className={MUTED}>Email, for invite and reset links</span>
          <Input
            type="email"
            className={FIELD_CLS}
            aria-label="New user’s email"
            autoCapitalize="none"
            autoCorrect="off"
            spellCheck={false}
            value={email}
            onChange={(e) => setEmail(e.target.value)}
          />
        </label>
        <div role="radiogroup" aria-label="Password" className="space-y-1.5">
          <span className={MUTED}>Password</span>
          <label className="flex items-center gap-2 type-control text-[var(--color-text-primary)] cursor-pointer">
            <Radio name="add-mode" checked={mode === 'type'} onChange={() => setMode('type')} />
            Type one
          </label>
          <label className="flex items-start gap-2 type-control text-[var(--color-text-primary)] cursor-pointer">
            <Radio name="add-mode" checked={mode === 'generate'} onChange={() => setMode('generate')} />
            <span>
              Let K2 generate one
              <span className={`block ${MUTED}`}>Shown once, after you add them.</span>
            </span>
          </label>
          <label
            className={`flex items-start gap-2 type-control cursor-pointer ${hasEmail ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-text-muted)]'}`}
          >
            <Radio
              name="add-mode"
              checked={mode === 'invite'}
              disabled={!hasEmail}
              onChange={() => setMode('invite')}
            />
            <span>
              Send them a link to set their own
              {!hasEmail ? <span className={`block ${MUTED}`}>Needs an email.</span> : null}
            </span>
          </label>
        </div>
        {mode === 'type' ? (
          <div className="flex items-center gap-2">
            <Input
              type={shown ? 'text' : 'password'}
              className={FIELD_CLS}
              aria-label="New user’s password"
              autoComplete="new-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
            />
            <Button variant="quiet" className="flex-shrink-0" onClick={() => setShown((s) => !s)}>
              {shown ? 'Hide' : 'Show'}
            </Button>
          </div>
        ) : null}
        {mode === 'invite' ? (
          <div className="space-y-2">
            <p className={MUTED}>An invite is for one App. They choose their password on that App’s page.</p>
            <div className="block space-y-1">
              <span className={MUTED}>App</span>
              <SettingDropdown
                fullWidth
                menuAlign="left"
                ariaLabel="Invite to App"
                placeholder="Choose an App"
                value={app}
                options={apps}
                onChange={(v) => {
                  // A re-pick of the same App keeps the role.
                  if (v === app) return
                  setApp(v)
                  setRole('')
                }}
              />
            </div>
            <div className="block space-y-1">
              <span className={MUTED}>Role in this App (optional)</span>
              <SettingDropdown
                fullWidth
                menuAlign="left"
                ariaLabel="Role in the App"
                value={role}
                options={[{ value: '', label: 'No role' }, ...appRoles.map((r) => ({ value: r.value, label: r.label }))]}
                onChange={setRole}
              />
            </div>
            <label className="block space-y-1">
              <span className={MUTED}>
                Send from (optional): a sender address set up for this App. Leave it blank to use the App’s default.
              </span>
              <Input
                type="email"
                className={FIELD_CLS}
                aria-label="Send the invite from"
                autoCapitalize="none"
                autoCorrect="off"
                spellCheck={false}
                value={from}
                onChange={(e) => setFrom(e.target.value)}
              />
            </label>
          </div>
        ) : null}
        <ErrorLine text={error} />
        <div className="flex justify-end gap-2">
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button type="submit" variant="accent" disabled={!ready || busy}>
            {mode === 'invite' ? 'Add and send link' : 'Add person'}
          </Button>
        </div>
      </form>
    </Sheet>
  )
}

/** After Add user: the generated password or invite link, once. */
export function AddedUserDialog({ added, onClose }: { added: AddedUser; onClose: () => void }): React.JSX.Element {
  return (
    <Sheet title={`${added.username} added`} onClose={onClose} testId="added-user-dialog">
      {added.secret ? (
        <OneTimeSecret
          label={
            added.secret.kind === 'password'
              ? `${added.username}’s password. Give it to them yourself.`
              : `K2 couldn’t email this link, so give it to ${added.username} yourself. It works once: they open it and choose their password.`
          }
          value={added.secret.value}
          expiry={added.secret.expiry}
          copyLabel={added.secret.kind === 'password' ? 'Copy password' : 'Copy invite link'}
        />
      ) : null}
      {added.note ? <p className={NOTE}>{added.note}</p> : null}
      <div className="flex justify-end">
        <Button variant="accent" onClick={onClose}>
          Done
        </Button>
      </div>
    </Sheet>
  )
}
