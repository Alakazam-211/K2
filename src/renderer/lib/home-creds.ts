// Home P1 — `{base, token}` for a Home row's server, WITHOUT switching.
// A saved server uses its own saved login (host-ops `remoteCreds`, the
// Settings → Connections tile path); `local` is this computer's daemon
// (owner token via `getLocalDaemonWs`, the "Clone to this computer" path).

import { remoteCreds, type HostCreds } from '@/lib/host-ops'
import { daemonHttpBase, getLocalDaemonWs } from '@/kessel/daemon-ws'
import { isWebClient } from '@/lib/is-web'
import type { ConnectHost } from '@/stores/connect-host'
import { LOCAL_HOME_HOST, savedHostForKey } from '@/lib/home-address'

export type HostCredsResult =
  | { kind: 'ok'; creds: HostCreds; self: string }
  | { kind: 'unsaved' }
  | { kind: 'unreachable' }

export async function credsForHomeHost(hostKey: string, hosts: ConnectHost[]): Promise<HostCredsResult> {
  if (hostKey === LOCAL_HOME_HOST) {
    if (isWebClient()) return { kind: 'unreachable' }
    try {
      const c = await getLocalDaemonWs()
      return { kind: 'ok', creds: { base: daemonHttpBase(c), token: c.token }, self: 'owner' }
    } catch (err) {
      console.debug('[home] local daemon creds unavailable:', err)
      return { kind: 'unreachable' }
    }
  }
  const host = savedHostForKey(hosts, hostKey)
  if (!host) return { kind: 'unsaved' }
  return { kind: 'ok', creds: remoteCreds(host), self: host.username || 'owner' }
}
