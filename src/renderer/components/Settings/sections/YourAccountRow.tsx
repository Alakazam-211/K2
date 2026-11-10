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
import { Button, Callout } from '@/components/ui'
import { SettingDropdown, SettingRow, SettingsGroup } from '../controls/SettingControls'
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
  fullWidth = false,
}: {
  users: AccountChoice[]
  owner: OwnerAccountsState
  readOnly: boolean
  /** Fill a field column (Settings rows use the shared field width). */
  fullWidth?: boolean
}): React.JSX.Element {
  const options = useMemo(() => yourAccountOptions(users, owner.accounts), [users, owner.accounts])
  return (
    <SettingDropdown
      value={yourAccountValue(owner.accounts)}
      options={options}
      disabled={readOnly || owner.busy !== null}
      ariaLabel="Your account"
      fullWidth={fullWidth}
      onChange={(v) => {
        const next = nextOwnerAccounts(owner.accounts, v)
        if (next) void owner.setAccounts(next)
      }}
    />
  )
}

const ACCOUNT_HINT = 'The login you use on your phone or laptop.'

/** Settings → General → Your account. Loading and a failed read are rows of
 *  their own (never a bare heading); a Member, or a server without
 *  owner-linked accounts, sees nothing. With `groupTitle` the row brings
 *  its own card group, so the group is gone whenever the row is. */
export function YourAccountRow({ groupTitle }: { groupTitle?: string } = {}): React.JSX.Element | null {
  // A retry remounts the reads from scratch.
  const [attempt, setAttempt] = useState(0)
  return <YourAccountReads key={attempt} groupTitle={groupTitle} onRetry={() => setAttempt((n) => n + 1)} />
}

function YourAccountReads({ groupTitle, onRetry }: { groupTitle?: string; onRetry: () => void }): React.JSX.Element | null {
  const [who, setWho] = useState<Who | null>(null)
  const [users, setUsers] = useState<AccountChoice[] | null>(null)
  const [readError, setReadError] = useState<string | null>(null)
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
        if (!cancelled) setReadError(e instanceof Error ? e.message : String(e))
      }
    })()
    return () => {
      cancelled = true
    }
  }, [available])

  const access = who ? ownerLinkAccess(who.ownerToken, who.role) : null
  const owner = useOwnerAccounts(available && access !== null && access !== 'hidden')

  if (!available || access === 'hidden') return null

  const wrap = (row: React.JSX.Element): React.JSX.Element =>
    groupTitle ? (
      <SettingsGroup title={groupTitle} card>
        {row}
      </SettingsGroup>
    ) : (
      row
    )

  // The first read failed, or the linked list couldn't be read: say so and
  // offer the read again.
  const failed = readError ?? (!owner.loaded ? owner.error : null)
  if (failed) {
    return wrap(
      <SettingRow
        label="Your account"
        hint={ACCOUNT_HINT}
        settingId="general.your-account"
        testId="your-account-status"
        below={
          <Callout tone="error" role="alert">
            <p className="type-hint">Couldn&apos;t read your account from this server: {failed}</p>
          </Callout>
        }
      >
        <Button onClick={onRetry} className="flex-shrink-0">
          Try again
        </Button>
      </SettingRow>,
    )
  }

  if (access === null || !owner.loaded || users === null) {
    return wrap(
      <SettingRow label="Your account" hint={ACCOUNT_HINT} settingId="general.your-account" testId="your-account-status">
        <span role="status" className="type-hint text-[var(--color-text-secondary)]">
          Loading…
        </span>
      </SettingRow>,
    )
  }

  const readOnly = access === 'read'
  const many = owner.accounts.length > 1

  return wrap(
    <SettingRow
      label="Your account"
      settingId="general.your-account"
      testId="your-account-row"
      field
      hint={
        <span data-testid="your-account-hint">
          {ACCOUNT_HINT} {OWNER_LINK_EXPLAINER}
          {many ? ' More than one login is marked as yours; they stay until you pick a different one.' : ''}
          {readOnly ? ' Only the owner, on the server’s own computer, can change it.' : ''}
        </span>
      }
      below={
        owner.error ? (
          <Callout tone="error" role="alert">
            <p className="type-hint">Couldn&apos;t save your account: {owner.error}</p>
          </Callout>
        ) : undefined
      }
    >
      <YourAccountSelect users={users} owner={owner} readOnly={readOnly} fullWidth />
    </SettingRow>,
  )
}
