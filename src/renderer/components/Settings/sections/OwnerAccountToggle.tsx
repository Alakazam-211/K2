// Settings → Admin Access → Users / Access: "This is me (owner)" on each
// Connect user row (prd-thread-push-notifications-v1 §3, owner-linked
// accounts). The owner marks the logins their own phone or laptop use;
// those logins then count as the owner PERSON for notification prefs,
// coalescing, the push `principal` and "you" labels.
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

export const OWNER_LINK_LABEL = 'This is me (owner)'

export const OWNER_LINK_EXPLAINER =
  "Your phone or laptop logging in as this account counts as you for notifications and 'you' labels. It doesn't change what the account can do."

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

  return { accounts: data.accounts, aliases: data.aliases, loaded, error, busy, isLinked, setLinked }
}

/** The per-row checkbox. Same peer-checked square as the row's "Enabled". */
export function OwnerAccountToggle({
  username,
  access,
  linked,
  busy,
  onChange,
}: {
  username: string
  access: OwnerLinkAccess
  linked: boolean
  busy?: boolean
  onChange: (linked: boolean) => void
}): React.JSX.Element | null {
  if (access === 'hidden') return null
  const readOnly = access === 'read'
  return (
    <label
      data-testid={`owner-link-${username}`}
      title={readOnly ? 'Only the owner can change this.' : OWNER_LINK_EXPLAINER}
      className={`flex items-center gap-1.5 select-none no-drag ${readOnly ? 'cursor-default' : 'cursor-pointer'}`}
    >
      <input
        type="checkbox"
        aria-label={`${username}: ${OWNER_LINK_LABEL}`}
        checked={linked}
        disabled={readOnly || busy}
        onChange={(e) => {
          if (readOnly || busy) return
          onChange(e.target.checked)
        }}
        className="peer sr-only"
      />
      <span
        aria-hidden="true"
        className={`w-3 h-3 flex-shrink-0 flex items-center justify-center border transition-colors border-[var(--color-border)] bg-[var(--color-bg-elevated)] peer-checked:bg-[var(--color-accent)] peer-checked:border-[var(--color-accent)] peer-focus-visible:ring-1 peer-focus-visible:ring-[var(--color-accent)] ${
          readOnly || busy ? 'opacity-60' : ''
        }`}
      >
        {linked && (
          <svg viewBox="0 0 12 12" className="w-2.5 h-2.5" fill="none" stroke="var(--color-on-accent)" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
            <path d="M2.5 6.5 L5 9 L9.5 3.5" />
          </svg>
        )}
      </span>
      <span className="text-[10px] text-[var(--color-text-secondary)]">{OWNER_LINK_LABEL}</span>
    </label>
  )
}
