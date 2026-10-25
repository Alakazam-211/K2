// Top bar "Admin profile" (Rosson 2026-10-16, 0.46.0): a person icon right
// of the Zen toggle. A click opens a modal with who this window is signed
// in as on its server. Values come from daemon routes
// (`admin-profile-api.ts`).
//
// A place to EDIT, not a list of pointers (Rosson 2026-10-17: "just let
// people update things from there rather than telling them where those
// things exist"), laid out calm and simple:
//
//   - a header: the chip in the person's colour, the name, the username,
//     the role tag (and "Owner's account" on an owner-linked login), with
//     how and where you are signed in as one muted line;
//   - labelled rows (one label column, one control column, the Settings
//     row and type scale): Colour, First name, Last name, Your account
//     (the owner key);
//   - Password, then a Security section (collapsed, like the nav's
//     sections) with recent sign-ins for the owner key.
//
// Names (0.46.0): optional first/last for every person (`/cli/people/name`,
// `usePersonName`, shared with Admin Access). The owner key's names are its
// linked account's (the one picked in Your account); with none linked the
// owner is "Owner" and the name fields wait for a pick. The owner's old
// separate display name is gone.
//
// Every control saves straight away, optimistic, rolled back on a refusal,
// and is the same component its Settings twin uses: `YourAccountSelect` +
// `useOwnerAccounts` (`POST /cli/owner/accounts`, Settings → General),
// `PersonColorPicker` (`POST /cli/people/color`), `PersonNameInput`.
//
// Permissions: Your account is the owner key's alone (the daemon refuses
// it to every login), so a Connect login never sees it; a Connect login
// edits only its own names, colour and password.
// The modal is always about the window's own server, never a followed
// (possibly view-only) room, so nothing here edits a room you only view.
//
// Password, inline: the owner key sets its linked login's password with
// the owner's permission (`POST /cli/users/set-password`); a Connect login
// changes its own with the SAME step as the first-password flow
// (`PasswordRotationStep`, `POST /cli/auth/change-password`, then a
// re-login with the new password because the daemon revokes every session
// of that user).

import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { Button, DialogScrim, Surface } from '@/components/ui'
import type { ServerScope } from '@/kessel/server-scope'
import { windowTopBarTarget } from './top-bar-scope'
import type { AuthAuditEvent } from '@/components/Settings/sections/access-audit-api'
import {
  primaryOwnerAccount,
  YourAccountSelect,
  type AccountChoice,
} from '@/components/Settings/sections/YourAccountRow'
import { OWNER_ACCOUNT_TAG, useOwnerAccounts } from '@/components/Settings/sections/OwnerAccountToggle'
import { PersonNameInput, usePersonName } from '@/components/Presence/PersonNameFields'
import { realName } from '@/lib/person-name'
import { SettingRow } from '@/components/Settings/controls/SettingControls'
import { COLOR_NEEDS_NEWER_SERVER, PersonColorPicker } from '@/components/Presence/PersonColorPicker'
import type { PersonColorView } from '@/components/Presence/person-color-api'
import { PresenceChip, type PresenceRole } from '@/components/Presence/PresenceChip'
import { daemonCliPost } from '@/lib/daemon-cli'
import { PasswordRotationStep } from '@/components/PasswordRotationStep'
import { CHANGE_PASSWORD_COPY } from '@/lib/password-rotation'
import { saveRememberedPassword, useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { useToastStore } from '@/stores/toast'
import { AdminIcon } from '@/components/icons/K2Icons'
import { PhonePairing } from '@/components/Settings/shared/PhonePairing'
import { NAV_SECTION_HOVER_CLASS, NavSectionChevron, NavSectionLabel } from '@/components/Sidebar/NavSection'
import {
  loadAdminProfile,
  signInsFor,
  type AdminProfile,
  type ProfileAccount,
  type ProfileRole,
} from './admin-profile-api'

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

function Muted({ children }: { children: React.ReactNode }): React.JSX.Element {
  return <p className="m-0 py-1 text-xs text-[var(--color-text-secondary)]">{children}</p>
}

/** A small square tag (role, "Owner's account"). */
function Tag({ children, testId }: { children: React.ReactNode; testId?: string }): React.JSX.Element {
  return (
    <span
      data-testid={testId}
      className="inline-flex items-center px-1.5 py-px text-[9px] font-medium uppercase tracking-wider border border-[var(--color-border)] text-[var(--color-text-secondary)] bg-[var(--color-bg-elevated)] select-none rounded-[var(--radius-field)]"
    >
      {children}
    </span>
  )
}

/** How a sign-in reached the server, in plain words. */
export function ingressLabel(ingress: string): string {
  if (ingress === 'loopback') return 'This computer'
  if (ingress === 'lan') return 'Local network'
  if (ingress === 'tunnel') return 'K2 Connect'
  if (ingress.startsWith('edge:')) return 'K2 Connect (edge)'
  return ingress || '—'
}

function clientLabel(client: string | undefined): string {
  if (client === 'web') return 'App / web'
  if (client === 'api') return 'API'
  return client || '—'
}

/** Recent sign-ins as a grid: Time, (Login,) Client, IP, Via, Result.
 *  Every cell wraps (long IPs and clients break, nothing is cut off);
 *  rows align to the top so wrapped cells keep one row height per row. */
function SignInGrid({ events, showUser }: { events: AuthAuditEvent[]; showUser: boolean }): React.JSX.Element {
  const headers = ['Time', ...(showUser ? ['Login'] : []), 'Client', 'IP', 'Via', 'Result']
  const columns = showUser
    ? 'minmax(84px,1.1fr) minmax(64px,1fr) minmax(56px,0.8fr) minmax(72px,1.1fr) minmax(64px,1fr) minmax(64px,1fr)'
    : 'minmax(84px,1.1fr) minmax(56px,0.8fr) minmax(72px,1.2fr) minmax(64px,1fr) minmax(64px,1fr)'
  const cell = 'min-w-0 py-1 pr-2 [overflow-wrap:anywhere] [word-break:break-word] border-b border-[var(--color-border)]'
  return (
    <div role="table" aria-label="Recent sign-ins" data-testid="admin-profile-sign-in-grid" className="text-[11px]">
      <div role="row" className="grid items-start" style={{ gridTemplateColumns: columns }}>
        {headers.map((hd) => (
          <div
            key={hd}
            role="columnheader"
            className={`${cell} text-[10px] font-semibold uppercase tracking-wider text-[var(--color-text-muted)]`}
          >
            {hd}
          </div>
        ))}
      </div>
      {events.map((ev, i) => {
        const ok = ev.outcome === 'ok'
        return (
          <div
            key={`${ev.ts}-${i}`}
            role="row"
            data-testid="admin-profile-sign-in"
            className="grid items-start"
            style={{ gridTemplateColumns: columns }}
          >
            <div role="cell" className={`${cell} text-[var(--color-text-secondary)]`}>{formatTime(ev.ts)}</div>
            {showUser ? (
              <div role="cell" className={`${cell} text-[var(--color-text-primary)]`}>{ev.user}</div>
            ) : null}
            <div role="cell" className={`${cell} text-[var(--color-text-secondary)]`}>{clientLabel(ev.client)}</div>
            <div role="cell" className={`${cell} font-mono text-[var(--color-text-secondary)]`}>
              {ev.ip && ev.ip !== '-' ? ev.ip : '—'}
            </div>
            <div role="cell" className={`${cell} text-[var(--color-text-secondary)]`}>{ingressLabel(ev.ingress)}</div>
            <div
              role="cell"
              className={`${cell} ${ok ? 'text-[var(--color-text-primary)]' : 'text-[var(--color-status-error-soft)]'}`}
            >
              {ok ? 'Signed in' : `Failed (${ev.outcome})`}
            </div>
          </div>
        )
      })}
    </div>
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

/** The owner's account record: the first of `linked` (the login picked in
 *  Your account), looked up in every account row. Null for a Connect
 *  login, before anything is picked, or when the reads failed. */
export function ownerAccountOf(profile: AdminProfile, linked: string[]): ProfileAccount | null {
  if (!profile.ownerToken || profile.allUsers.kind !== 'ok') return null
  const name = primaryOwnerAccount(linked)
  if (!name) return null
  return profile.allUsers.value.find((a) => a.username.trim().toLowerCase() === name) ?? null
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
      <p className="m-0 text-xs text-[var(--color-text-secondary)]">
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
        <Button type="submit" variant="quiet" disabled={busy || !pw || pw !== confirm}>
          {busy ? 'Saving…' : 'Set password'}
        </Button>
        <Button variant="ghost" onClick={onCancel}>
          Cancel
        </Button>
      </div>
    </form>
  )
}

export const PASSWORD_CHANGED_NOTICE =
  'Password changed. This device is signed in with the new one; other devices signed in with this login must sign in again.'

function accountChoicesOf(rows: ProfileAccount[]): AccountChoice[] {
  return rows.filter((r) => !r.disabled).map((r) => ({ username: r.username, role: r.role ?? 'member' }))
}

/** The loaded profile: header, rows, sign-ins and Password, plus the two
 *  inline password steps. Mounted once the profile has loaded, so the
 *  shared owner-accounts hook can start from what was read. */
function LoadedProfile({
  profile,
  scope,
  closeButton,
}: {
  profile: AdminProfile
  scope: ServerScope
  closeButton: React.ReactNode
}): React.JSX.Element {
  const linkedRead = profile.linked
  const ownerEdits = profile.ownerToken && linkedRead.kind === 'ok'
  const owner = useOwnerAccounts(
    ownerEdits,
    ownerEdits && linkedRead.kind === 'ok' ? { accounts: linkedRead.value, aliases: linkedRead.value } : undefined,
  )
  const [colorView, setColorView] = useState<PersonColorView | null | undefined>(undefined)
  const [changing, setChanging] = useState<ConnectHost | null>(null)
  const [ownerChanging, setOwnerChanging] = useState<string | null>(null)
  const [notice, setNotice] = useState<string | null>(null)
  const [securityOpen, setSecurityOpen] = useState(false)

  // The owner's linked logins follow the dropdown at once.
  const linked = ownerEdits ? owner.accounts : linkedRead.kind === 'ok' ? linkedRead.value : []
  const account = ownerAccountOf(profile, linked)
  const me = profile.username?.trim().toLowerCase() ?? null
  const host = scope.connectHost()

  // Names: your own; for the owner key, the linked account's (re-read when
  // a different account is picked).
  const personName = usePersonName(scope, undefined, linked.join(','))
  const nameView = personName.state.kind === 'ok' ? personName.state.view : null
  const real = realName(nameView)
  const name = real ?? (profile.ownerToken ? account?.username ?? 'Owner' : (profile.username ?? 'unknown'))
  // The owner key has names only through a linked account.
  const namesEditable = nameView !== null && (!profile.ownerToken || account !== null)
  const username = profile.ownerToken ? account?.username ?? null : profile.username
  const role: ProfileRole | null = account ? account.role : profile.role
  const created = profile.ownerToken
    ? account?.createdAt ?? null
    : profile.accounts.kind === 'ok'
      ? profile.accounts.value[0]?.createdAt ?? null
      : null
  const isOwnersAccount = !profile.ownerToken && me !== null && linkedRead.kind === 'ok' && linkedRead.value.includes(me)

  const signIns = useMemo((): AuthAuditEvent[] => {
    if (profile.ownerToken) return profile.audit.kind === 'ok' ? signInsFor(profile.audit.value, linked) : []
    return profile.signIns.kind === 'ok' ? profile.signIns.value : []
  }, [profile, linked])
  const showSignIns = profile.ownerToken || profile.role === 'owner'

  const choices = useMemo(
    () => (profile.allUsers.kind === 'ok' ? accountChoicesOf(profile.allUsers.value) : null),
    [profile.allUsers],
  )

  const onChangePassword = ((): (() => void) | null => {
    const h = passwordChangeHost(profile, scope)
    if (h) {
      return () => {
        setNotice(null)
        setChanging(h)
      }
    }
    return account
      ? () => {
          setNotice(null)
          setOwnerChanging(account.username)
        }
      : null
  })()

  const server = [
    scope.label,
    host && host.hostname !== scope.label ? host.hostname : null,
    profile.publicUrl,
  ]
    .filter(Boolean)
    .join(' · ')

  const header = (
    <div
      className="flex items-start gap-3 flex-shrink-0"
      style={{ padding: '14px 18px', borderBottom: '1px solid var(--color-border)' }}
      data-testid="admin-profile-header"
    >
      <PresenceChip
        name={name}
        role={(role ?? 'member') as PresenceRole}
        size={36}
        color={colorView?.color ?? null}
        names={nameView}
        title=""
      />
      <div className="flex-1 min-w-0">
        <div className="flex items-center gap-2 flex-wrap">
          <h2
            id="admin-profile-title"
            data-testid="admin-profile-name"
            className="m-0 text-[14px] font-semibold text-[var(--color-text-primary)] truncate"
          >
            {name}
          </h2>
          <Tag testId="admin-profile-role">{roleLabel(role)}</Tag>
          {isOwnersAccount ? <Tag testId="admin-profile-owners-account">{OWNER_ACCOUNT_TAG}</Tag> : null}
        </div>
        {username && username !== name ? (
          <div className="text-xs text-[var(--color-text-secondary)] truncate" data-testid="admin-profile-username">
            {username}
          </div>
        ) : null}
        <div className="mt-0.5 text-[11px] text-[var(--color-text-muted)]" data-testid="admin-profile-how">
          {signedInHow(profile, scope.isRemote)}
          <span data-testid="admin-profile-server"> · {server}</span>
          {created ? <span data-testid="admin-profile-created"> · Created {formatDate(created)}</span> : null}
        </div>
      </div>
      {closeButton}
    </div>
  )

  let body: React.ReactNode
  if (ownerChanging) {
    body = (
      <OwnerSetPasswordStep
        scope={scope}
        username={ownerChanging}
        onCancel={() => setOwnerChanging(null)}
        onDone={() => {
          setNotice(ownerPasswordChangedNotice(ownerChanging))
          setOwnerChanging(null)
        }}
      />
    )
  } else if (changing) {
    body = (
      <PasswordRotationStep
        host={changing}
        variant="inline"
        copy={CHANGE_PASSWORD_COPY}
        onCancel={() => setChanging(null)}
        onDone={async ({ newPassword }) => {
          // The step already re-logged in and stored the new session
          // token, so this window keeps working. A remembered password
          // must follow, or the next silent sign-in would use the old one.
          const saved = useConnectHostStore.getState().hosts.find((x) => x.id === changing.id)
          if (saved?.remember ?? changing.remember) {
            const failure = await saveRememberedPassword(changing.id, newPassword)
            if (failure) useToastStore.getState().addToast(failure, 'error', 12000)
          }
          setChanging(null)
          setNotice(PASSWORD_CHANGED_NOTICE)
        }}
      />
    )
  } else {
    body = (
      <>
        {notice ? (
          <p role="status" data-testid="admin-profile-notice" className="m-0 mb-2 text-xs text-[var(--color-text-primary)]">
            {notice}
          </p>
        ) : null}
        <div data-testid="admin-profile-fields">
          {/* Colour first (Rosson): the header chip is its preview, so the
              row is just the picker. */}
          <SettingRow columns label="Colour" testId="admin-profile-color">
            <PersonColorPicker scope={scope} onView={setColorView} />
          </SettingRow>
          {personName.state.kind !== 'error' ? (
            <>
              <SettingRow columns label="First name" testId="admin-profile-first-name">
                <PersonNameInput
                  label="First name"
                  value={nameView?.firstName ?? null}
                  disabled={!namesEditable || personName.busy}
                  placeholder={profile.ownerToken && account === null ? 'Pick Your account first' : undefined}
                  onCommit={(v) => void personName.save({ firstName: v })}
                />
              </SettingRow>
              <SettingRow columns label="Last name" testId="admin-profile-last-name">
                <div className="flex flex-col gap-1 min-w-0 w-full">
                  <PersonNameInput
                    label="Last name"
                    value={nameView?.lastName ?? null}
                    disabled={!namesEditable || personName.busy}
                    placeholder={profile.ownerToken && account === null ? 'Pick Your account first' : undefined}
                    onCommit={(v) => void personName.save({ lastName: v })}
                  />
                  {personName.state.kind === 'unsupported' ? (
                    <p className="m-0 text-[11px] text-[var(--color-text-muted)]">{COLOR_NEEDS_NEWER_SERVER}</p>
                  ) : null}
                  {personName.writeError ? (
                    <p role="alert" className="m-0 text-[11px] text-[var(--color-status-error-soft)]">
                      {personName.writeError}
                    </p>
                  ) : null}
                </div>
              </SettingRow>
            </>
          ) : null}
          {ownerEdits && choices ? (
            <SettingRow columns label="Your account" testId="admin-profile-your-account">
              <div className="flex flex-col gap-1 min-w-0">
                <YourAccountSelect users={choices} owner={owner} readOnly={false} />
                {owner.error ? (
                  <p role="alert" className="m-0 text-[11px] text-[var(--color-status-error-soft)]">
                    {owner.error}
                  </p>
                ) : null}
              </div>
            </SettingRow>
          ) : null}
        </div>

        {onChangePassword ? (
          <div data-testid="admin-profile-account">
            <SettingRow columns label="Password" testId="admin-profile-password">
              <Button variant="quiet" data-testid="admin-profile-change-password" onClick={onChangePassword}>
                Change password
              </Button>
            </SettingRow>
          </div>
        ) : null}

        {/* Your phones (phone-web-off): pair your own phone and revoke your
            own phones. A Connect login types its password first; the owner
            key does not (the daemon decides). */}
        <section className="mt-3" data-testid="admin-profile-phones">
          <div className="mb-1 text-[11px] text-[var(--color-text-secondary)]">Your phones</div>
          <PhonePairing mode="mine" scope={scope} />
        </section>

        {showSignIns ? (
          <section className="mt-3" data-testid="admin-profile-security">
            {/* Collapsed by default, styled like the nav's sections. */}
            <button
              type="button"
              data-testid="admin-profile-security-toggle"
              onClick={() => setSecurityOpen((v) => !v)}
              aria-expanded={securityOpen}
              aria-controls="admin-profile-security-content"
              className={`no-drag w-full flex items-center gap-1.5 py-1.5 text-left cursor-pointer ${NAV_SECTION_HOVER_CLASS}`}
            >
              <NavSectionLabel>Security</NavSectionLabel>
              <span className="flex-1" />
              <NavSectionChevron collapsed={!securityOpen} />
            </button>
            {securityOpen ? (
              <div id="admin-profile-security-content" data-testid="admin-profile-sign-ins" className="pb-1">
                <div className="mb-1 text-[11px] text-[var(--color-text-secondary)]">Recent sign-ins</div>
                {signIns.length === 0 ? (
                  <Muted>No sign-ins on record.</Muted>
                ) : (
                  <SignInGrid events={signIns} showUser={profile.ownerToken} />
                )}
              </div>
            ) : null}
          </section>
        ) : null}
      </>
    )
  }

  return (
    <>
      {header}
      <div className="overflow-y-auto min-h-0" style={{ padding: '6px 18px 16px' }}>
        {body}
      </div>
    </>
  )
}

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), [tabindex]:not([tabindex="-1"])'

export function AdminProfileModal({ onClose }: { onClose: () => void }): React.JSX.Element {
  // The window's own server (like the timer): a focused remote room
  // doesn't change who you are signed in as here.
  const scope = windowTopBarTarget().scope
  const [state, setState] = useState<{ kind: 'loading' } | { kind: 'ok'; profile: AdminProfile } | { kind: 'error'; message: string }>({
    kind: 'loading',
  })
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
  }, [state.kind])

  useEffect(() => {
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

  const closeButton = (
    <button
      ref={closeRef}
      type="button"
      onClick={onClose}
      aria-label="Close"
      title="Close (Esc)"
      data-testid="admin-profile-close"
      className="flex h-6 w-6 flex-shrink-0 items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] cursor-pointer"
    >
      ×
    </button>
  )

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
          // Wide enough for the sign-ins grid; shrinks with small windows.
          width: 'min(680px, calc(100vw - 32px))',
          maxHeight: '78vh',
          display: 'flex',
          flexDirection: 'column',
          overflow: 'hidden',
        }}
        onMouseDown={(e) => e.stopPropagation()}
      >
        {state.kind === 'ok' ? (
          <LoadedProfile profile={state.profile} scope={scope} closeButton={closeButton} />
        ) : (
          <>
            <div
              className="flex items-center justify-between gap-3 flex-shrink-0"
              style={{ padding: '14px 18px', borderBottom: '1px solid var(--color-border)' }}
            >
              <h2 id="admin-profile-title" className="m-0 text-[14px] font-semibold text-[var(--color-text-primary)]">
                Profile
              </h2>
              {closeButton}
            </div>
            <div className="overflow-y-auto min-h-0" style={{ padding: '14px 18px 16px' }}>
              {state.kind === 'loading' ? (
                <Muted>Loading…</Muted>
              ) : (
                <p role="alert" className="m-0 text-xs text-[var(--color-status-error-soft)]">
                  Couldn’t read who you are on {scope.label}: {state.message}
                </p>
              )}
            </div>
          </>
        )}
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
