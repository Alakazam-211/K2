// account-db-daemon.ts — where this window's k2.dev account calls go.
//
// 0.46.6: the account server address can be set (`connectDbUrl` setting or
// K2_CONNECT_DB_URL on the daemon); the default is unchanged. The LOCAL
// daemon resolves it (`k2_core::connect_db`) and reports it as
// `accountDbUrl` on GET /cli/tunnel/config. App.tsx installs this loader
// once, so the account calls in `k2-account.ts` go to the same server as the
// daemon's own lease loop. Local daemon only, like the Tunnel panel: the
// account session lives on this machine.

import { getLocalDaemonWs, daemonHttpBase, invalidateDaemonWs } from '@/kessel/daemon-ws'
import { withRemoteRetry } from '@/lib/remote-retry'
import { setAccountDbBaseLoader } from './k2-account'

/** Ask the local daemon which account server to use. `null` = a daemon
 *  too old to say (it has no override, so the default is right). Throws
 *  when the daemon can't be asked: the caller must not guess a host. */
export async function accountDbBaseFromDaemon(): Promise<string | null> {
  const res = await withRemoteRetry(async () => {
    const send = async (): Promise<Response> => {
      const creds = await getLocalDaemonWs()
      return fetch(`${daemonHttpBase(creds)}/cli/tunnel/config?token=${creds.token}`, { method: 'GET' })
    }
    const first = await send()
    if (first.status !== 403) return first
    invalidateDaemonWs()
    return send()
  }, { onRetry: invalidateDaemonWs })
  if (!res.ok) throw new Error(`K2 answered HTTP ${res.status}`)
  const cfg = (await res.json()) as { accountDbUrl?: unknown }
  return typeof cfg.accountDbUrl === 'string' && cfg.accountDbUrl.trim() ? cfg.accountDbUrl : null
}

/** Route this window's account calls through the daemon's answer. */
export function installAccountDbBaseLoader(): void {
  setAccountDbBaseLoader(accountDbBaseFromDaemon)
}
