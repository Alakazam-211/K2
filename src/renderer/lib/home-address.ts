// Home P1 (prd-home-v1 H14 / vs-live H14) — how a Home row names an agent.
//
// A row's key is `handle::host`, lowercase, built with `formatAgentHost`.
// `host` is:
//   - `local` for the daemon on the computer running this window (matches
//     `ActiveHost = 'local'`; the list is per-client, so that is correct);
//   - a saved server's `hostname`, plus `:port` when the port is not the
//     default for its scheme (so LAN rows like `192.168.1.20:38471` stay
//     distinct).
// A row is matched to a saved server by hostname + port, never by the
// client-made `ConnectHost.id` (that changes when a server is re-added).
// `workspaceId` rides along only as a rename-repair hint: identity is the
// handle, and a cloned workspace gets a new id anyway.

import { formatAgentHost, slugifyAddressToken } from '@/lib/federation'
import type { ActiveHost, ConnectHost } from '@/stores/connect-host'

export const LOCAL_HOME_HOST = 'local'

/** Row host key for `local` or a saved server. */
export function homeHostKey(
  h: 'local' | Pick<ConnectHost, 'hostname' | 'port' | 'secure'>,
): string {
  if (h === 'local') return LOCAL_HOME_HOST
  const hostname = h.hostname.trim().toLowerCase()
  const defaultPort = h.secure ? 443 : 80
  return h.port === defaultPort ? hostname : `${hostname}:${h.port}`
}

/** The connected server's row host key. */
export function activeHomeHostKey(active: ActiveHost): string {
  return homeHostKey(active)
}

/** `handle::host`, lowercase. */
export function homeAddress(handle: string, hostKey: string): string {
  return formatAgentHost(handle, hostKey)
}

/** Split a row address on its first `::`. Unlike `parseAgentAtHost` this
 *  accepts `local` and dotless LAN names — every saved server is a valid
 *  row host. Null for anything that is not `<handle>::<host>`. */
export function parseHomeAddress(address: string): { handle: string; host: string } | null {
  const t = address.trim().toLowerCase()
  const i = t.indexOf('::')
  if (i <= 0) return null
  const handle = t.slice(0, i)
  const host = t.slice(i + 2)
  if (!handle || !host || host.includes('::')) return null
  return { handle, host }
}

/** A workspace's address token: its daemon `handle`, else the D4 slug of
 *  its name (handles can be empty until backfilled). */
export function workspaceHandle(p: { handle?: string | null; name: string }): string | null {
  const h = (p.handle ?? '').trim().toLowerCase()
  if (h) return h
  return slugifyAddressToken(p.name)
}

export interface HomeRowRef {
  address: string
  workspaceId: string | null
}

/** Find the workspace a row points at in one server's list. Handle wins
 *  (identity). If no workspace has the handle, the stored id finds a
 *  renamed one (repair). Null when neither matches. */
export function findWorkspaceForRow<P extends { id: string; handle?: string | null; name: string }>(
  workspaces: P[],
  row: HomeRowRef,
): P | null {
  const parsed = parseHomeAddress(row.address)
  if (!parsed) return null
  const byHandle = workspaces.find((w) => workspaceHandle(w) === parsed.handle)
  if (byHandle) return byHandle
  if (row.workspaceId) {
    const byId = workspaces.find((w) => w.id === row.workspaceId)
    if (byId) return byId
  }
  return null
}

/** The saved server a row host key points at. Several saved entries can
 *  share an address; prefer the one holding a login. */
export function savedHostForKey(hosts: ConnectHost[], hostKey: string): ConnectHost | null {
  const matches = hosts.filter((h) => homeHostKey(h) === hostKey)
  if (matches.length === 0) return null
  return matches.find((h) => h.token.length > 0) ?? matches[0]
}
