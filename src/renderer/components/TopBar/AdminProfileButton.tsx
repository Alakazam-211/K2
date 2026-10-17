// Top bar "Admin profile" (Rosson 2026-10-16, 0.46.0 UI experiment): a
// person icon right of the Zen toggle. A click opens a modal with who this
// window is signed in as on its server: name, role, how (the owner key on
// this computer, or a Connect login), colour, account dates and recent
// sign-ins. Values come from daemon routes (`admin-profile-api.ts`); the
// full controls live in Settings → User Access.
//
// The owner key on this computer shows the owner's ACCOUNT (Rosson
// 2026-10-17): the login picked in Settings → General → Your account, with
// that login's username, role and created date, the owner's display name,
// and a Change password that sets that login's password with the owner's
// permission (`POST /cli/users/set-password`). With no account picked it
// shows the owner profile and says where to pick one.
//
// Colour (0.46.0): the person swatches (`PersonColorPicker`) for whoever is
// signed in; the owner and owner-linked logins set the owner's colour.
//
// A Connect login can change its own password here (Rosson 2026-10-16):
// the SAME step as the first-password flow (`PasswordRotationStep`,
// `POST /cli/auth/change-password`, then a re-login with the new password
// because the daemon revokes every session of that user). Hidden for the
// owner key on this computer, which has no password of its own.

import { useCallback, useEffect, useRef, useState } from 'react'
import { DialogScrim, Surface } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { windowTopBarTarget } from './top-bar-scope'
import { useSettingsStore } from '@/stores/settings'
import type { AuthAuditEvent } from '@/components/Settings/sections/access-audit-api'
import { primaryOwnerAccount } from '@/components/Settings/sections/YourAccountRow'
import { PersonColorPicker } from '@/components/Presence/PersonColorPicker'
import { daemonCliPost } from '@/lib/daemon-cli'
import { PasswordRotationStep } from '@/components/PasswordRotationStep'
import { CHANGE_PASSWORD_COPY } from '@/lib/password-rotation'
import { saveRememberedPassword, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useToastStore } from '@/stores/toast'
import { AdminIcon } from '@/components/icons/K2Icons'
import { loadAdminProfile, type AdminProfile, type ProfileAccount, type ProfileRole, type Read } from './admin-profile-api'

const noDrag = { WebkitAppRegion: 'no-drag' } as React.CSSProperties


const ROLE_LABEL: Record<ProfileRole, string> = { owner: 'Owner', admin: 'Admin', member: 'Member' }

export function roleLabel(role: ProfileRole | null): string {
  return role ? ROLE_LABEL[role] : 'Unknown'
}

/** How this window is signed in to its server, in plain words. */
export function signedInHow(profile: Pick<AdminProfile, 'ownerToken'>, isRemote: boolean): string {
  if (profile.ownerToken) {
    return isRemote ? 'With the owner key of that server' : 'As the owner, on this computer'
  }
  return isRemote ? 'With a Connect login, from this device' : 'With a Connect login'
}

function formatDate(iso: string): string {
  const d = new Date(iso)
  if (!iso || Number.isNaN(d.getTime())) return '—'
  return d.toLocaleDateString(undefined, { year: 'numeric', month: 'short', day: 'numeric' })
}

function formatTime(iso: string): string {
  const d = new Date(iso)
  if (!iso || Number.isNaN(d.getTime())) return iso || '—'
  return d.toLocaleString(undefined, { month: 'short', day: 'numeric', hour: 'numeric', minute: '2-digit' })
}

function unavailable(r: Exclude<Read<unknown>, { kind: 'ok' }>): string {
  return r.kind === 'role' ? `Only ${r.needs} can see this.` : `Couldn’t load: ${r.message}`
}

function Row({ label, children, testId }: { label: string; children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <div className="flex items-baseline gap-3 py-1" data-testid={testId}>
      <div className="w-28 flex-shrink-0 text-[11px] text-[var(--color-text-muted)]">{label}</div>
      <div className="flex-1 min-w-0 text-[12.5px] text-[var(--color-text-primary)] break-words">{children}</div>
    </div>
  )
}

function Section({ title, children, testId }: { title: string; children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <section className="mb-4 last:mb-0" data-testid={testId}>
      <h3 className="text-[11px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)] m-0 mb-1">
        {title}
      </h3>
      {children}
    </section>
  )
}

function Muted({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <p className="m-0 py-1 text-[12px] text-[var(--color-text-secondary)]">{children}</p>
}

function SignInRow({ ev, showUser }: { ev: AuthAuditEvent; showUser: boolean }): React.JSX.Element {
  const ok = ev.outcome === 'ok'
  return (
    <li className="flex items-baseline gap-2 py-0.5 text-[12px]" data-testid="admin-profile-sign-in">
      <span className="w-32 flex-shrink-0 text-[var(--color-text-secondary)]">{formatTime(ev.ts)}</span>
      <span className={ok ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-status-error-soft)]'}>
        {ok ? 'Signed in' : `Failed (${ev.outcome})`}
      </span>
      {showUser ? <span className="text-[var(--color-text-secondary)]">as {ev.user}</span> : null}
      <span className="text-[var(--color-text-muted)] truncate">
        {ev.ip && ev.ip !== '-' ? `from ${ev.ip}` : ''}
        {ev.ingress ? ` · ${ev.ingress}` : ''}
      </span>
    </li>
  )
}

/** The saved server this window signed in to with `profile`'s Connect
 *  login, ready for the password step; null for the owner key (or no
 *  saved server), where there is no self-service password. */
export function passwordChangeHost(profile: AdminProfile, scope: ServerScope): ConnectHost | null {
  if (profile.ownerToken || !profile.username) return null
  const host = scope.connectHost()
  return host ? { ...host, username: profile.username } : null
}

/** The hint for the owner key when no account is picked yet. */
export const PICK_ACCOUNT_HINT = 'Pick your account in Settings → General'

/** The owner's account: the login picked in Settings → General → Your
 *  account (the first linked login), with its record. Null for a Connect
 *  login, before anything is picked, or when the reads failed. */
export function ownerAccountOf(profile: AdminProfile): ProfileAccount | null {
  if (!profile.ownerToken || profile.linked.kind !== 'ok' || profile.accounts.kind !== 'ok') return null
  const name = primaryOwnerAccount(profile.linked.value)
  if (!name) return null
  return profile.accounts.value.find((a) => a.username.trim().toLowerCase() === name) ?? null
}

export function ownerPasswordChangedNotice(username: string): string {
  return `Password changed for ${username}. Devices signed in with it must sign in again.`
}

/** The owner key sets the linked login's password (Owner floor,
 *  `POST /cli/users/set-password`): no current password, no re-login
 *  here — the daemon signs that login out everywhere. */
function OwnerSetPasswordStep({
  scope,
  username,
  onCancel,
  onDone,
}: {
  scope: ServerScope
  username: string
  onCancel: () => void
  onDone: () => void
}): React.JSX.Element {
  const [pw, setPw] = useState('')
  const [confirm, setConfirm] = useState('')
  const [busy, setBusy] = useState(false)
  const [err, setErr] = useState<string | null>(null)
  const mismatch = confirm !== '' && pw !== confirm
  const input =
    'w-full px-2 py-1 text-xs bg-[var(--color-bg-surface)] border border-[var(--color-border)] text-[var(--color-text-primary)] outline-none focus:border-[var(--color-accent)]'
  return (
    <form
      data-testid="admin-profile-owner-password"
      onSubmit={(e) => {
        e.preventDefault()
        if (busy || !pw || pw !== confirm) return
        setBusy(true)
        setErr(null)
        daemonCliPost(scope, 'users/set-password', { username, password: pw }).then(
          () => onDone(),
          (e2: unknown) => {
            setErr(e2 instanceof Error ? e2.message : String(e2))
            setBusy(false)
          },
        )
      }}
      className="flex flex-col gap-2"
    >
      <p className="m-0 text-[12px] text-[var(--color-text-secondary)]">
        Set a new password for <span className="text-[var(--color-text-primary)]">{username}</span>. Devices signed in
        with it must sign in again.
      </p>
      <label className="flex flex-col gap-1 text-[11px] text-[var(--color-text-muted)]">
        New password
        <input type="password" autoComplete="new-password" value={pw} onChange={(e) => setPw(e.target.value)} className={input} />
      </label>
      <label className="flex flex-col gap-1 text-[11px] text-[var(--color-text-muted)]">
        Confirm new password
        <input type="password" autoComplete="new-password" value={confirm} onChange={(e) => setConfirm(e.target.value)} className={input} />
      </label>
      {mismatch ? <p className="m-0 text-[11px] text-[var(--color-status-error-soft)]">The passwords don’t match.</p> : null}
      {err ? (
        <p role="alert" className="m-0 text-[11px] text-[var(--color-status-error-soft)]">
          {err}
        </p>
      ) : null}
      <div className="flex gap-2">
        <button
          type="submit"
          disabled={busy || !pw || pw !== confirm}
          className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer disabled:opacity-50 disabled:cursor-default"
        >
          {busy ? 'Saving…' : 'Set password'}
        </button>
        <button
          type="button"
          onClick={onCancel}
          className="px-3 py-1 text-[11px] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] cursor-pointer"
        >
          Cancel
        </button>
      </div>
    </form>
  )
}

export const PASSWORD_CHANGED_NOTICE =
  'Password changed. This device is signed in with the new one; other devices signed in with this login must sign in again.'

function ProfileBody({
  profile,
  scope,
  onChangePassword,
}: {
  profile: AdminProfile
  scope: ServerScope
  onChangePassword: (() => void) | null
}): React.JSX.Element {
  const ownerDisplayName = useSettingsStore((s) => s.ownerDisplayName)
  const host = scope.connectHost()
  const ownerName = ownerDisplayName.trim() || 'owner'
  const account = ownerAccountOf(profile)
  const name = profile.ownerToken ? (account?.username ?? ownerName) : (profile.username ?? 'unknown')
  const me = profile.username?.trim().toLowerCase() ?? null
  const ownerNoAccount = profile.ownerToken && profile.linked.kind === 'ok' && profile.linked.value.length === 0

  return (
    <>
      <Section title="You" testId="admin-profile-you">
        {ownerNoAccount ? (
          <p data-testid="admin-profile-pick-account" className="m-0 mb-1 text-[12px] text-[var(--color-text-secondary)]">
            {PICK_ACCOUNT_HINT}.
          </p>
        ) : null}
        <Row label="Name" testId="admin-profile-name">
          {name}
          {profile.ownerToken && account ? (
            <span className="text-[var(--color-text-muted)]"> (your Connect login, Settings → General → Your account)</span>
          ) : profile.ownerToken ? (
            <span className="text-[var(--color-text-muted)]"> (Settings → General → Your name)</span>
          ) : (
            <span className="text-[var(--color-text-muted)]"> (your Connect username)</span>
          )}
        </Row>
        {profile.ownerToken && account ? (
          <Row label="Display name" testId="admin-profile-display-name">
            {ownerName}
            <span className="text-[var(--color-text-muted)]"> (Settings → General → Your name)</span>
          </Row>
        ) : null}
        <Row label="Role" testId="admin-profile-role">{roleLabel(account ? account.role : profile.role)}</Row>
        <Row label="Signed in" testId="admin-profile-how">{signedInHow(profile, scope.isRemote)}</Row>
        <Row label="Server" testId="admin-profile-server">
          {scope.label}
          {host && host.hostname !== scope.label ? <span className="text-[var(--color-text-secondary)]"> · {host.hostname}</span> : null}
          {profile.publicUrl ? <span className="text-[var(--color-text-secondary)]"> · {profile.publicUrl}</span> : null}
        </Row>
        <Row label="Colour" testId="admin-profile-color">
          <PersonColorPicker scope={scope} />
        </Row>
      </Section>

      <Section title="Owner’s account" testId="admin-profile-linked">
        {profile.linked.kind !== 'ok' ? (
          <Muted>{unavailable(profile.linked)}</Muted>
        ) : profile.ownerToken ? (
          profile.linked.value.length === 0 ? (
            <Muted>No Connect login is yours yet. {PICK_ACCOUNT_HINT}.</Muted>
          ) : (
            <Row label="Your logins">{profile.linked.value.join(', ')}</Row>
          )
        ) : me && profile.linked.value.includes(me) ? (
          <Muted>This login is the owner’s account.</Muted>
        ) : (
          <Muted>
            This login isn’t the owner’s account.
            {profile.linked.value.length > 0 ? ` The owner’s logins: ${profile.linked.value.join(', ')}.` : ''}
          </Muted>
        )}
      </Section>

      <Section title="Account" testId="admin-profile-account">
        {profile.accounts.kind !== 'ok' ? (
          <Muted>{unavailable(profile.accounts)}</Muted>
        ) : profile.ownerToken && account ? (
          <Row label="Created" testId="admin-profile-created">
            {formatDate(account.createdAt)}
            {account.disabled ? ' · disabled' : ''}
          </Row>
        ) : profile.ownerToken ? (
          <>
            <Muted>The owner key isn’t an account, so it has no created date.</Muted>
            {profile.accounts.value.map((a) => (
              <Row key={a.username} label={a.username}>
                Created {formatDate(a.createdAt)} · {roleLabel(a.role)}
                {a.disabled ? ' · disabled' : ''}
              </Row>
            ))}
          </>
        ) : profile.accounts.value.length === 0 ? (
          <Muted>No account record found for this login.</Muted>
        ) : (
          profile.accounts.value.map((a) => (
            <Row key={a.username} label="Created" testId="admin-profile-created">
              {formatDate(a.createdAt)}
              {a.disabled ? ' · disabled' : ''}
            </Row>
          ))
        )}
        {onChangePassword ? (
          <div className="mt-1.5">
            <button
              type="button"
              data-testid="admin-profile-change-password"
              onClick={onChangePassword}
              className="px-3 py-1 text-[11px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer"
            >
              Change password
            </button>
          </div>
        ) : null}
      </Section>

      <Section title="Recent sign-ins" testId="admin-profile-sign-ins">
        {profile.signIns.kind !== 'ok' ? (
          <Muted>{unavailable(profile.signIns)}</Muted>
        ) : profile.signIns.value.length === 0 ? (
          <Muted>
            {profile.ownerToken
              ? 'No Connect sign-ins for your logins. The app on this computer doesn’t sign in.'
              : 'No sign-ins on record.'}
          </Muted>
        ) : (
          <ul className="m-0 p-0 list-none">
            {profile.signIns.value.map((ev, i) => (
              <SignInRow key={`${ev.ts}-${i}`} ev={ev} showUser={profile.ownerToken} />
            ))}
          </ul>
        )}
      </Section>
    </>
  )
}

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), [tabindex]:not([tabindex="-1"])'

export function AdminProfileModal({ onClose }: { onClose: () => void }): React.JSX.Element {
  // The window's own server (like the timer): a focused remote room
  // doesn't change who you are signed in as here.
  const scope = windowTopBarTarget().scope
  const openSettings = useSettingsStore((s) => s.openSettings)
  const [state, setState] = useState<{ kind: 'loading' } | { kind: 'ok'; profile: AdminProfile } | { kind: 'error'; message: string }>({
    kind: 'loading',
  })
  const [changing, setChanging] = useState<ConnectHost | null>(null)
  // The owner key changing its linked login's password (Owner permission).
  const [ownerChanging, setOwnerChanging] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const dialogRef = useRef<HTMLDivElement>(null)
  const closeRef = useRef<HTMLButtonElement>(null)

  useEffect(() => {
    let cancelled = false
    loadAdminProfile(scope).then(
      (profile) => {
        if (!cancelled) setState({ kind: 'ok', profile })
      },
      (e: unknown) => {
        if (!cancelled) setState({ kind: 'error', message: e instanceof Error ? e.message : String(e) })
      },
    )
    return () => {
      cancelled = true
    }
  }, [scope])

  useEffect(() => {
    closeRef.current?.focus()
    const onKey = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        onClose()
        return
      }
      if (e.key !== 'Tab') return
      const dialog = dialogRef.current
      if (!dialog) return
      const items = Array.from(dialog.querySelectorAll<HTMLElement>(FOCUSABLE))
      if (items.length === 0) return
      const first = items[0]
      const last = items[items.length - 1]
      const active = document.activeElement
      const inside = active instanceof Node && dialog.contains(active)
      if (e.shiftKey && (active === first || !inside)) {
        e.preventDefault()
        last.focus()
      } else if (!e.shiftKey && (active === last || !inside)) {
        e.preventDefault()
        first.focus()
      }
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [onClose])

  return (
    <>
      <DialogScrim
        data-testid="admin-profile-scrim"
        onMouseDown={(e) => {
          e.stopPropagation()
          onClose()
        }}
      />
      <Surface
        ref={dialogRef}
        role2="surface"
        elevation={5}
        className="no-drag"
        role="dialog"
        aria-modal="true"
        aria-labelledby="admin-profile-title"
        data-testid="admin-profile-dialog"
        style={{
          position: 'fixed',
          top: '50%',
          left: '50%',
          transform: 'translate(-50%, -50%)',
          zIndex: 99999,
          width: 'min(520px, 90vw)',
          maxHeight: '78vh',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        <div
          className="flex items-center justify-between gap-3 flex-shrink-0"
          style={{ padding: '12px 18px', borderBottom: '1px solid var(--color-border)' }}
        >
          <h2 id="admin-profile-title" className="text-[14px] font-semibold text-[var(--color-text-primary)] m-0">
            Admin profile
          </h2>
          <button
            ref={closeRef}
            type="button"
            onClick={onClose}
            aria-label="Close"
            title="Close (Esc)"
            data-testid="admin-profile-close"
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
          >
            ×
          </button>
        </div>
        <div className="overflow-y-auto min-h-0" style={{ padding: '14px 18px 16px' }}>
          {state.kind === 'loading' ? (
            <Muted>Loading…</Muted>
          ) : state.kind === 'error' ? (
            <p role="alert" className="m-0 text-[12px] text-[var(--color-status-error-soft)]">
              Couldn’t read who you are on {scope.label}: {state.message}
            </p>
          ) : ownerChanging ? (
            <OwnerSetPasswordStep
              scope={scope}
              username={ownerChanging}
              onCancel={() => setOwnerChanging(null)}
              onDone={() => {
                setNotice(ownerPasswordChangedNotice(ownerChanging))
                setOwnerChanging(null)
              }}
            />
          ) : changing ? (
            <PasswordRotationStep
              host={changing}
              variant="inline"
              copy={CHANGE_PASSWORD_COPY}
              onCancel={() => setChanging(null)}
              onDone={async ({ newPassword }) => {
                // The step already re-logged in and stored the new session
                // token, so this window keeps working. A remembered password
                // must follow, or the next silent sign-in would use the old one.
                const saved = useConnectHostStore.getState().hosts.find((h) => h.id === changing.id)
                if (saved?.remember ?? changing.remember) {
                  const failure = await saveRememberedPassword(changing.id, newPassword)
                  if (failure) useToastStore.getState().addToast(failure, 'error', 12000)
                }
                setChanging(null)
                setNotice(PASSWORD_CHANGED_NOTICE)
              }}
            />
          ) : (
            <>
              {notice ? (
                <p role="status" data-testid="admin-profile-notice" className="m-0 mb-3 text-[12px] text-[var(--color-text-primary)]">
                  {notice}
                </p>
              ) : null}
              <ProfileBody
                profile={state.profile}
                scope={scope}
                onChangePassword={(() => {
                  const host = passwordChangeHost(state.profile, scope)
                  if (host) {
                    return () => {
                      setNotice(null)
                      setChanging(host)
                    }
                  }
                  // The owner key: the linked login, with the owner's permission.
                  const account = ownerAccountOf(state.profile)
                  return account
                    ? () => {
                        setNotice(null)
                        setOwnerChanging(account.username)
                      }
                    : null
                })()}
              />
            </>
          )}
        </div>
        <div
          className="flex items-center justify-end gap-2 flex-shrink-0"
          style={{ padding: '10px 18px', borderTop: '1px solid var(--color-border)' }}
        >
          <button
            type="button"
            data-testid="admin-profile-open-settings"
            onClick={() => {
              onClose()
              openSettings('people')
            }}
            className="px-3 py-1.5 text-xs text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)] cursor-pointer"
          >
            Open Settings → User Access
          </button>
        </div>
      </Surface>
    </>
  )
}

export default function AdminProfileButton(): React.JSX.Element {
  const [open, setOpen] = useState(false)
  const buttonRef = useRef<HTMLButtonElement>(null)
  const close = useCallback(() => {
    setOpen(false)
    buttonRef.current?.focus()
  }, [])

  return (
    <>
      <button
        ref={buttonRef}
        type="button"
        data-admin-profile=""
        aria-label="Admin profile"
        aria-haspopup="dialog"
        aria-expanded={open}
        title="Admin profile"
        onClick={() => setOpen(true)}
        className="no-drag flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors cursor-pointer"
        style={noDrag}
      >
        {/* Icon style A3: Lucide `circle-user`, squared. */}
        <AdminIcon />
      </button>
      {open ? <AdminProfileModal onClose={close} /> : null}
    </>
  )
}
