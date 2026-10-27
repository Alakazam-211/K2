// App Approvals across servers (0.46.5 patch-up: "App Approvals only shows
// the active server").
//
// The held App requests of every server the person OWNS, not only the
// window's. The server list is the Home pool's (`lib/host-pool.ts`), the
// same one Home's multi-server rows use: the window's own server first, then
// each other server this window is signed in to (`auth: 'ok'`) whose whoami
// role is `owner`. Only an owner approves, so an Admin or Member login, a
// server not signed in, or one the pool has not checked yet is left out (it
// shows up once the pool has read it). Each server is read, approved and
// denied with ITS scope and login, and an approval takes a fresh owner
// confirm on that server's own vault (`vaultConfirmOn`): nothing is relayed
// through the window's server, and no daemon route is new.
//
// Pure bits only (no React); the section and the badge store use them.

import type { HostEntry } from '@/lib/host-pool'
import { canonicalHostKey } from '@/lib/host-key'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import type { AppApprovalsList } from '@/components/Settings/sections/app-approvals-api'

export interface ApprovalServer {
  hostKey: string
  /** The saved server's label, or "This computer". */
  label: string
  scope: ServerScope
  /** The window's own server (its push drives the live reload). */
  isWindow: boolean
}

/** The servers App Approvals reads, the window's own first, then the other
 *  owned ones by host key (a stable order). */
export function approvalServers(
  entries: Readonly<Record<string, HostEntry>>,
  primary: ServerScope,
  forHost: (hostKey: string) => ServerScope = scopeForHost,
): ApprovalServer[] {
  const out: ApprovalServer[] = [{ hostKey: primary.hostKey, label: primary.label, scope: primary, isWindow: true }]
  const seen = new Set([canonicalHostKey(primary.hostKey)])
  const others = Object.values(entries)
    .filter((e) => e.saved && e.auth === 'ok' && e.role === 'owner')
    .sort((a, b) => a.hostKey.localeCompare(b.hostKey))
  for (const e of others) {
    const key = canonicalHostKey(e.hostKey)
    if (seen.has(key)) continue
    seen.add(key)
    const scope = forHost(e.hostKey)
    out.push({ hostKey: e.hostKey, label: scope.label, scope, isWindow: false })
  }
  return out
}

/** One server's answer: its list, or why it could not be read. */
export interface ServerApprovals {
  server: ApprovalServer
  list: AppApprovalsList | null
  error: string | null
}

/** Read every server at once; one server failing never hides the others. */
export async function fetchApprovalsAcross(
  servers: readonly ApprovalServer[],
  state: 'pending' | 'all',
  read: (state: 'pending' | 'all', scope: ServerScope) => Promise<AppApprovalsList>,
  errorText: (e: unknown) => string,
): Promise<ServerApprovals[]> {
  return Promise.all(
    servers.map(async (server) => {
      try {
        return { server, list: await read(state, server.scope), error: null }
      } catch (e) {
        return { server, list: null, error: errorText(e) }
      }
    }),
  )
}
