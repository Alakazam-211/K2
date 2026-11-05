// The viewer's role on the window's remote server, kept current for as long
// as Settings → General is open (Restart and Update rows). Rules and copy:
// host-role.ts.
//
// The role used to be read once per host id at mount; one failed read
// (the connection still recovering after a boot, reload or server switch,
// the token not in memory yet, a blip) left the button greyed with "Only the
// host owner or an admin…" until the page was closed (Rosson 2026-11-05).
// Now it is read again:
//   - when the server, its login token or its sign-in epoch changes (the
//     keychain landing after boot, a re-login, a token rotation);
//   - when the connection comes back to `connected`;
//   - after a failed read, on a 1 s / 3 s / 10 s / 30 s backoff;
//   - when the window regains focus (a role changed elsewhere);
//   - on demand (`recheck`), when the host turns an action down.

import { useCallback, useEffect, useRef, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import { roleRetryDelayMs, roleStateFrom, shouldRecheckRole, type HostRoleState } from './host-role'

const CHECKING: HostRoleState = { kind: 'checking' }
/** Focus re-reads at most this often. */
const FOCUS_RECHECK_MIN_MS = 15_000

export interface ActiveHostRole {
  role: HostRoleState
  /** Read whoami now; resolves with what the host said this time. */
  recheck: () => Promise<HostRoleState>
}

/** Merge a fresh read into the shown state: a known role is not lost to a
 *  read that couldn't be sent or answered (a blip); a host that refuses the
 *  login, or names another role, replaces it. */
function merge(prev: HostRoleState, next: HostRoleState): HostRoleState {
  if (prev.kind === 'known' && (next.kind === 'checking' || next.kind === 'unreadable')) return prev
  return next
}

export function useActiveHostRole(): ActiveHostRole {
  const active = useConnectHostStore((s) => s.activeHost)
  const recoveryKind = useConnectHostStore((s) => s.recovery.kind)
  const epoch = useConnectHostStore((s) => s.sessionEpoch)
  const isRemote = active !== 'local'
  const hostId = isRemote ? active.id : ''
  const token = isRemote ? (active.token ?? '') : ''

  const [role, setRole] = useState<HostRoleState>(CHECKING)
  const roleRef = useRef<HostRoleState>(CHECKING)
  // Bumped when the server changes: a read started for another server is
  // dropped.
  const genRef = useRef(0)
  const inflightRef = useRef<Promise<HostRoleState> | null>(null)
  const attemptRef = useRef(0)
  const lastReadAtRef = useRef(0)
  const mountedRef = useRef(true)

  const commit = useCallback((next: HostRoleState): void => {
    roleRef.current = next
    setRole(next)
  }, [])

  const read = useCallback((): Promise<HostRoleState> => {
    if (inflightRef.current) return inflightRef.current
    const gen = genRef.current
    lastReadAtRef.current = Date.now()
    const p = (async (): Promise<HostRoleState> => {
      let next: HostRoleState
      try {
        next = roleStateFrom({ body: await daemonCliGet<unknown>(primaryScope(), 'auth/whoami') })
      } catch (err) {
        next = roleStateFrom({ err })
      }
      if (mountedRef.current && gen === genRef.current) {
        if (next.kind === 'known') attemptRef.current = 0
        commit(merge(roleRef.current, next))
      }
      return next
    })()
    inflightRef.current = p
    void p.finally(() => {
      if (inflightRef.current === p) inflightRef.current = null
    })
    return p
  }, [commit])

  useEffect(() => {
    mountedRef.current = true
    return () => {
      mountedRef.current = false
    }
  }, [])

  // A new server starts from "checking"; a new token or sign-in on the same
  // server re-reads while the row keeps what it knew.
  const lastHostRef = useRef<string | null>(null)
  useEffect(() => {
    if (!isRemote) {
      lastHostRef.current = null
      genRef.current += 1
      inflightRef.current = null
      commit(CHECKING)
      return
    }
    if (lastHostRef.current !== hostId) {
      lastHostRef.current = hostId
      genRef.current += 1
      inflightRef.current = null
      attemptRef.current = 0
      commit(CHECKING)
    }
    void read()
  }, [isRemote, hostId, token, epoch, read, commit])

  // The connection is back: read again (the read during recovery was never
  // sent, and the role may have changed while it was away).
  const prevRecoveryRef = useRef(recoveryKind)
  useEffect(() => {
    const was = prevRecoveryRef.current
    prevRecoveryRef.current = recoveryKind
    if (isRemote && recoveryKind === 'connected' && was !== 'connected') void read()
  }, [recoveryKind, isRemote, read])

  // Not known yet: read again on a backoff until it is.
  useEffect(() => {
    if (!isRemote || !shouldRecheckRole(role)) return
    const delay = roleRetryDelayMs(attemptRef.current)
    const id = window.setTimeout(() => {
      attemptRef.current += 1
      void read()
    }, delay)
    return () => window.clearTimeout(id)
  }, [role, isRemote, read])

  // Back to the window: a role changed elsewhere shows without a reload.
  useEffect(() => {
    if (!isRemote) return
    const onFocus = (): void => {
      if (Date.now() - lastReadAtRef.current >= FOCUS_RECHECK_MIN_MS) void read()
    }
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [isRemote, read])

  return { role, recheck: read }
}
