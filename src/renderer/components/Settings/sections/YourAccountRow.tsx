// Settings → General → You → "Your account" (0.46.0, Rosson 2026-10-17).
// The owner picks the Connect login their own phone or laptop signs in
// with. It replaced the per-row "This is me (owner)" checkbox on Admin
// Access (that row now shows an "Owner's account" tag).
//
// Same daemon truth and floors as before (`ownerAccounts`,
// `POST /cli/owner/accounts`): only the owner token may change it; an
// Admin or Owner-role login sees it read-only; a Member doesn't see it.
// Picking a login sets the list to `[that login]`; "None" clears it. When
// more than one login is already linked the trigger reads
// "z3thon + 1 more" and the others stay until a different login is
// picked.

import React, { useEffect, useMemo, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { SettingDropdown } from '../controls/SettingControls'
import { ownerLinkAccess, useOwnerAccounts, OWNER_LINK_EXPLAINER, type OwnerAccountsState } from './OwnerAccountToggle'
import { threadPushAvailable } from './useNotifyPrefs'

/** Dropdown value for "None". */
export const NO_ACCOUNT = ''
/** Dropdown value for "several are linked; keep them". */
export const MANY_ACCOUNTS = '__many__'

export interface AccountChoice {
  username: string
  role: string
}

/** The logins the dropdown offers: Owner and Admin roles, sorted. */
export function accountChoices(users: AccountChoice[]): AccountChoice[] {
  return users
    .filter((u) => u.role === 'owner' || u.role === 'admin')
    .slice()
    .sort((a, b) => a.username.localeCompare(b.username))
}

/** The owner's account as shown: the first linked login (sorted), or null. */
export function primaryOwnerAccount(accounts: string[]): string | null {
  return accounts.length > 0 ? [...accounts].sort()[0] : null
}

/** "None", "z3thon" or "z3thon + 1 more". */
export function yourAccountLabel(accounts: string[]): string {
  const first = primaryOwnerAccount(accounts)
  if (!first) return 'None'
  return accounts.length > 1 ? `${first} + ${accounts.length - 1} more` : first
}

/** The dropdown's value for the linked list. */
export function yourAccountValue(accounts: string[]): string {
  if (accounts.length === 0) return NO_ACCOUNT
  return accounts.length > 1 ? MANY_ACCOUNTS : accounts[0]
}

/** The list after picking `value`, or null for no change. Picking the
 *  login already shown first keeps the others (several linked). */
export function nextOwnerAccounts(current: string[], value: string): string[] | null {
  if (value === MANY_ACCOUNTS) return null
  if (value === NO_ACCOUNT) return current.length === 0 ? null : []
  const name = value.trim().toLowerCase()
  if (current.length === 1 && current[0] === name) return null
  if (current.length > 1 && primaryOwnerAccount(current) === name) return null
  return [name]
}

interface Who {
  ownerToken: boolean
  role: string | null
}

function asRole(v: unknown): string | null {
  return v === 'owner' || v === 'admin' || v === 'member' ? v : null
}

/** `GET /cli/users` → the enabled logins the dropdown may offer. */
export function parseAccountChoices(raw: unknown): AccountChoice[] {
  const users = (raw as { users?: unknown } | null)?.users
  if (!Array.isArray(users)) return []
  const out: AccountChoice[] = []
  for (const u of users) {
    const r = (u ?? {}) as Record<string, unknown>
    if (typeof r.username === 'string' && r.disabled !== true) out.push({ username: r.username, role: asRole(r.role) ?? 'member' })
  }
  return out
}

/** The dropdown's options: None, "z3thon + 1 more" when several are
 *  linked, the Owner/Admin logins, and a linked login that isn't one. */
export function yourAccountOptions(users: AccountChoice[], accounts: string[]): { value: string; label: string; badge?: string }[] {
  const opts: { value: string; label: string; badge?: string }[] = [{ value: NO_ACCOUNT, label: 'None' }]
  if (accounts.length > 1) opts.push({ value: MANY_ACCOUNTS, label: yourAccountLabel(accounts) })
  for (const c of accountChoices(users)) opts.push({ value: c.username, label: c.username, badge: c.role === 'owner' ? 'Owner' : 'Admin' })
  // A linked login that is not Owner/Admin (or disabled) still shows.
  for (const a of accounts) {
    if (accounts.length === 1 && !opts.some((o) => o.value === a)) opts.push({ value: a, label: a })
  }
  return opts
}

/** The "Your account" dropdown itself. Shared by Settings → General and
 *  the profile modal so both write the same way (`owner.setAccounts`,
 *  optimistic with rollback). */
export function YourAccountSelect({
  users,
  owner,
  readOnly,
}: {
  users: AccountChoice[]
  owner: OwnerAccountsState
  readOnly: boolean
}): React.JSX.Element {
  const options = useMemo(() => yourAccountOptions(users, owner.accounts), [users, owner.accounts])
  return (
    <SettingDropdown
      value={yourAccountValue(owner.accounts)}
      options={options}
      disabled={readOnly || owner.busy !== null}
      ariaLabel="Your account"
      onChange={(v) => {
        const next = nextOwnerAccounts(owner.accounts, v)
        if (next) void owner.setAccounts(next)
      }}
    />
  )
}

export function YourAccountRow(): React.JSX.Element | null {
  const [who, setWho] = useState<Who | null>(null)
  const [users, setUsers] = useState<AccountChoice[] | null>(null)
  const available = threadPushAvailable()

  useEffect(() => {
    if (!available) return
    let cancelled = false
    void (async () => {
      try {
        const w = await daemonCliGet<{ owner?: unknown; role?: unknown }>(primaryScope(), 'auth/whoami')
        const next: Who = { ownerToken: w.owner === true, role: asRole(w.role) ?? (w.owner === true ? 'owner' : null) }
        if (cancelled) return
        setWho(next)
        if (ownerLinkAccess(next.ownerToken, next.role) === 'hidden') return
        const list = parseAccountChoices(await daemonCliGet<unknown>(primaryScope(), 'users'))
        if (!cancelled) setUsers(list)
      } catch (e) {
        if (!cancelled) console.debug('[settings] your account: whoami/users skipped:', e)
      }
    })()
    return () => {
      cancelled = true
    }
  }, [available])

  const access = who ? ownerLinkAccess(who.ownerToken, who.role) : 'hidden'
  const owner = useOwnerAccounts(available && access !== 'hidden')

  if (!available || access === 'hidden' || !owner.loaded || users === null) return null
  const readOnly = access === 'read'
  const many = owner.accounts.length > 1

  return (
    <div
      className="flex items-center justify-between py-2 border-b border-[var(--color-border)]"
      data-settings-id="general.your-account"
      data-testid="your-account-row"
    >
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Your account</span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5" data-testid="your-account-hint">
          The Connect login you use on your phone or laptop. {OWNER_LINK_EXPLAINER}
          {many ? ' More than one login is marked as yours; they stay until you pick a different one.' : ''}
          {readOnly ? ' Only the owner, on the server’s own computer, can change it.' : ''}
        </p>
        {owner.error && (
          <p role="alert" className="text-[10px] text-[var(--color-status-error-soft)] mt-0.5">
            {owner.error}
          </p>
        )}
      </div>
      <YourAccountSelect users={users} owner={owner} readOnly={readOnly} />
    </div>
  )
}
