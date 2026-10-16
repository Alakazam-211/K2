// Top bar "Admin profile" (Rosson 2026-10-16, 0.46.0 UI experiment): a
// person icon right of the Zen toggle. A click opens a read-only modal with
// who this window is signed in as on its server: name, role, how (the owner
// key on this computer, or a Connect login), the "This is me (owner)"
// links, account dates and recent sign-ins. Every value comes from an
// existing daemon route (`admin-profile-api.ts`); the full controls live in
// Settings → User Access.

import { useCallback, useEffect, useRef, useState } from 'react'
import { DialogScrim, Surface } from '@/components/ui'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'
import { useSettingsStore } from '@/stores/settings'
import type { AuthAuditEvent } from '@/components/Settings/sections/access-audit-api'
import { OWNER_LINK_LABEL } from '@/components/Settings/sections/OwnerAccountToggle'
import { loadAdminProfile, type AdminProfile, type ProfileRole, type Read } from './admin-profile-api'

const noDrag = { WebkitAppRegion: 'no-drag' } as React.CSSProperties

function PersonIcon(): React.JSX.Element {
  return (
    <svg
      width="14"
      height="14"
      viewBox="0 0 14 14"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.3"
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      data-admin-profile-icon=""
    >
      <circle cx="7" cy="4.6" r="2.5" />
      <path d="M2.2 12.4c0-2.55 2.15-4.3 4.8-4.3s4.8 1.75 4.8 4.3" />
    </svg>
  )
}

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

function ProfileBody({ profile, scope }: { profile: AdminProfile; scope: ServerScope }): React.JSX.Element {
  const ownerDisplayName = useSettingsStore((s) => s.ownerDisplayName)
  const host = scope.connectHost()
  const name = profile.ownerToken ? ownerDisplayName.trim() || 'owner' : (profile.username ?? 'unknown')
  const me = profile.username?.trim().toLowerCase() ?? null

  return (
    <>
      <Section title="You" testId="admin-profile-you">
        <Row label="Name" testId="admin-profile-name">
          {name}
          {profile.ownerToken ? (
            <span className="text-[var(--color-text-muted)]"> (Settings → General → Your name)</span>
          ) : (
            <span className="text-[var(--color-text-muted)]"> (your Connect username)</span>
          )}
        </Row>
        <Row label="Role" testId="admin-profile-role">{roleLabel(profile.role)}</Row>
        <Row label="Signed in" testId="admin-profile-how">{signedInHow(profile, scope.isRemote)}</Row>
        <Row label="Server" testId="admin-profile-server">
          {scope.label}
          {host && host.hostname !== scope.label ? <span className="text-[var(--color-text-secondary)]"> · {host.hostname}</span> : null}
          {profile.publicUrl ? <span className="text-[var(--color-text-secondary)]"> · {profile.publicUrl}</span> : null}
        </Row>
      </Section>

      <Section title={`“${OWNER_LINK_LABEL}”`} testId="admin-profile-linked">
        {profile.linked.kind !== 'ok' ? (
          <Muted>{unavailable(profile.linked)}</Muted>
        ) : profile.ownerToken ? (
          profile.linked.value.length === 0 ? (
            <Muted>No Connect login is marked as you yet. Tick “{OWNER_LINK_LABEL}” on your phone or laptop login in User Access.</Muted>
          ) : (
            <Row label="Your logins">{profile.linked.value.join(', ')}</Row>
          )
        ) : me && profile.linked.value.includes(me) ? (
          <Muted>This login is marked as the owner’s own.</Muted>
        ) : (
          <Muted>
            This login isn’t marked as the owner’s.
            {profile.linked.value.length > 0 ? ` The owner’s logins: ${profile.linked.value.join(', ')}.` : ''}
          </Muted>
        )}
      </Section>

      <Section title="Account" testId="admin-profile-account">
        {profile.accounts.kind !== 'ok' ? (
          <Muted>{unavailable(profile.accounts)}</Muted>
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
  const scope = primaryScope()
  const openSettings = useSettingsStore((s) => s.openSettings)
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
          ) : (
            <ProfileBody profile={state.profile} scope={scope} />
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
        <PersonIcon />
      </button>
      {open ? <AdminProfileModal onClose={close} /> : null}
    </>
  )
}
