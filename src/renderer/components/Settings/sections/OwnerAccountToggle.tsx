// Owner-linked accounts (prd-thread-push-notifications-v1 §3). The owner
// picks the Connect login their own phone or laptop uses in Settings →
// General → Your account (0.46.0, `YourAccountRow`; it replaced the
// per-row "This is me (owner)" checkbox on Admin Access, which now shows
// an "Owner's account" tag on that row). Linked logins count as the owner
// PERSON for notification prefs, coalescing, the push `principal`, "you"
// labels and the person colour.
//
// Identity only, never privilege: a linked login keeps its own role and
// gains no owner-only power. Only the owner token may change the list
// (`POST /cli/owner/accounts`); an Admin (or an Owner-role login) sees it
// read-only; a Member never sees it.

import React, { useCallback, useEffect, useRef, useState } from 'react'
import {
  fetchOwnerAccounts,
  setOwnerAccounts,
  toggledOwnerAccounts,
  type OwnerAccounts,
} from './notify-api'

/** The tag on the owner's linked row in Admin Access. */
export const OWNER_ACCOUNT_TAG = "Owner's account"

export const OWNER_LINK_EXPLAINER =
  "Your phone or laptop signing in with this login counts as you: notifications, 'you' labels and your colour. It doesn't change what the login can do."

/** What this viewer may do with the switch. */
export type OwnerLinkAccess = 'edit' | 'read' | 'hidden'

/** `edit` for the owner token only (the daemon refuses every login, even
 *  an Owner-role one); `read` for any other Admin+ login; else `hidden`. */
export function ownerLinkAccess(ownerToken: boolean, role: string | null): OwnerLinkAccess {
  if (ownerToken) return 'edit'
  if (role === 'owner' || role === 'admin') return 'read'
  return 'hidden'
}

export interface OwnerAccountsState {
  accounts: string[]
  aliases: string[]
  loaded: boolean
  error: string | null
  /** Username whose write is in flight. */
  busy: string | null
  isLinked: (username: string) => boolean
  setLinked: (username: string, linked: boolean) => Promise<void>
  /** Replace the whole list (Settings → General → Your account). */
  setAccounts: (accounts: string[]) => Promise<void>
}

/** The linked list, loaded when `enabled` (Admin+ on a server with
 *  `thread-push-v1`). */
export function useOwnerAccounts(enabled: boolean): OwnerAccountsState {
  const [data, setData] = useState<OwnerAccounts>({ accounts: [], aliases: [] })
  const [loaded, setLoaded] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [busy, setBusy] = useState<string | null>(null)
  const latest = useRef(data)
  latest.current = data

  useEffect(() => {
    if (!enabled) {
      setData({ accounts: [], aliases: [] })
      setLoaded(false)
      return
    }
    let cancelled = false
    fetchOwnerAccounts().then(
      (d) => {
        if (cancelled) return
        setData(d)
        setLoaded(true)
        setError(null)
      },
      (e: unknown) => {
        if (cancelled) return
        setError(e instanceof Error ? e.message : String(e))
      },
    )
    return () => {
      cancelled = true
    }
  }, [enabled])

  const isLinked = useCallback(
    (username: string) => data.accounts.includes(username.trim().toLowerCase()),
    [data],
  )

  const setLinked = useCallback(async (username: string, linked: boolean) => {
    setBusy(username)
    setError(null)
    try {
      setData(await setOwnerAccounts(toggledOwnerAccounts(latest.current.accounts, username, linked)))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }, [])

  const setAccounts = useCallback(async (accounts: string[]) => {
    setBusy(accounts[0] ?? '')
    setError(null)
    try {
      setData(await setOwnerAccounts(accounts))
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }, [])

  return { accounts: data.accounts, aliases: data.aliases, loaded, error, busy, isLinked, setLinked, setAccounts }
}

/** Admin Access row tag for the login the owner picked as theirs in
 *  Settings → General → Your account (0.46.0: replaces the per-row
 *  "This is me (owner)" checkbox). Renders nothing for other rows. */
export function OwnerAccountTag({ username, linked }: { username: string; linked: boolean }): React.JSX.Element | null {
  if (!linked) return null
  return (
    <span
      data-testid={`owner-account-tag-${username}`}
      title={OWNER_LINK_EXPLAINER}
      className="inline-flex w-fit items-center px-1.5 py-px text-[9px] font-medium uppercase tracking-wider border border-[var(--color-border)] text-[var(--color-text-secondary)] bg-[var(--color-bg-elevated)] select-none"
    >
      {OWNER_ACCOUNT_TAG}
    </span>
  )
}
