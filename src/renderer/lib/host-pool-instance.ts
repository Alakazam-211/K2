// Home M2 — the app's connection pool and its wiring.
//
// `lib/host-pool.ts` is the pure pool (every dependency injected, so tests
// can run several "windows" side by side). This module builds the one pool
// this webview uses, over the real connect-host store, keychain, login lease
// and Tauri events, and wires it in:
//   - `daemonCli*` on a pinned scope for another server revives through it
//     (`setPinnedAuthRecovery`);
//   - a saved server's re-key (MS61) drops the old key's entry;
//   - a login that lands anywhere is broadcast (MS28), and other windows'
//     broadcasts are applied (`installHostSessionSync`, from ConnectionGate).

import { emit, listen } from '@tauri-apps/api/event'
import { daemonHttpBase, getLocalDaemonWs } from '@/kessel/daemon-ws'
import { noteServerVersion, scopeForHost } from '@/kessel/server-scope'
import { daemonCliPost, withHostCliSlot } from '@/lib/daemon-cli'
import { setPinnedAuthRecovery, setPoolSocketCloseSink, setPoolStatusSource } from '@/lib/pool-hooks'
import { homeHostKey } from '@/lib/host-key'
import { createHostPool, type BootBody, type HostPool } from '@/lib/host-pool'
import { loginCoordinator, WINDOW_INSTANCE_ID } from '@/lib/host-login-coord'
import { createHostSessionSync, type HostSessionEvent } from '@/lib/host-session-sync'
import { isWebClient } from '@/lib/is-web'
import { onLoginLanded, onSavedHostRekey } from '@/lib/connect-host-hooks'
import { CLI_CONNECTED_RETRY_DELAYS_MS, withRemoteRetry } from '@/lib/remote-retry'
import {
  forgetToken,
  hostBaseUrl,
  loginToHost,
  resolvePassword,
  resolveToken,
  useConnectHostStore,
  type ConnectHost,
} from '@/stores/connect-host'

const REQUEST_TIMEOUT_MS = 5_000

function windowHostKey(): string {
  return homeHostKey(useConnectHostStore.getState().activeHost)
}

export const hostPool: HostPool = createHostPool({
  hosts: () => useConnectHostStore.getState().hosts,
  windowHostKey,
  localCreds: async () => {
    if (isWebClient()) throw new Error('no local daemon on the web client')
    const c = await getLocalDaemonWs()
    return { base: daemonHttpBase(c), token: c.token }
  },
  // The pool's own requests share the per-server `/cli` cap (MS25) and
  // the connected one-quick-retry schedule `[0]` (MS23).
  http: (hostKey, url, init) =>
    withHostCliSlot(scopeForHost(hostKey), () =>
      withRemoteRetry(() => fetch(url, { ...init, signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS) }), {
        delaysMs: CLI_CONNECTED_RETRY_DELAYS_MS,
      }),
    ),
  bootStatus: async (hostKey, base) => {
    try {
      return await withHostCliSlot(scopeForHost(hostKey), () =>
        withRemoteRetry(
          async () => {
            const res = await fetch(`${base}/boot-status`, {
              method: 'GET',
              signal: AbortSignal.timeout(4_000),
            })
            if (!res.ok) return null
            return (await res.json()) as BootBody
          },
          { delaysMs: CLI_CONNECTED_RETRY_DELAYS_MS },
        ),
      )
    } catch {
      // Unreachable after the one quick retry: offline.
      return null
    }
  },
  resolvePassword,
  login: (host, password) => loginToHost(host, password),
  dropSessionInMemory: (hostId) => useConnectHostStore.getState().dropSessionInMemory(hostId),
  coord: loginCoordinator,
  noteVersion: noteServerVersion,
  // MS39: on THAT server's scope, so its cap, its login and its one
  // revive-and-replay apply; never the window's server.
  activate: async (hostKey, projectId) => {
    await daemonCliPost(scopeForHost(hostKey), 'projects/activate', { projectId })
  },
  now: () => Date.now(),
})

// MS10 / MS77: a pinned scope's refused request revives through the pool.
setPinnedAuthRecovery(async (scope) =>
  (await hostPool.revive(scope.hostKey)) === 'revived' ? 'revived' : 'not-revived',
)

// 0.43.2 Z19: the top bar of a focused Home room reads that server's reach,
// login and role from here.
setPoolStatusSource(hostPool.store)

// MS61: a re-keyed server starts over under its new key.
onSavedHostRekey((oldKey) => hostPool.forget(oldKey))

const sessionSync = createHostSessionSync({
  windowId: WINDOW_INSTANCE_ID,
  emit: (event, payload) => emit(event, payload),
  listen: async (event, handler) => {
    const unlisten = await listen<HostSessionEvent>(event, (e) => handler(e.payload))
    return unlisten
  },
  hosts: () => useConnectHostStore.getState().hosts,
  windowHostKey,
  resolveToken,
  setHostToken: (hostId, token) => useConnectHostStore.getState().setHostToken(hostId, token),
  dropSessionInMemory: (hostId) => useConnectHostStore.getState().dropSessionInMemory(hostId),
  coord: loginCoordinator,
  pool: hostPool,
})

onLoginLanded((event) => sessionSync.loginLanded(event))

let installed: Promise<() => void> | null = null

/** Listen for other windows' logins and sign-outs. Once per webview. */
export function installHostSessionSync(): Promise<() => void> {
  if (isWebClient()) return Promise.resolve(() => {})
  if (!installed) {
    installed = sessionSync.install().catch((err: unknown) => {
      console.warn('[host-session] could not listen for other windows:', err)
      installed = null
      return () => {}
    })
  }
  return installed
}

// MS71: every daemon socket close reaches the pool.
setPoolSocketCloseSink((hostKey, code) => hostPool.noteSocketClose(hostKey, code))

/**
 * The user signs out of a saved server from Settings (an explicit gesture,
 * so the keychain token may go — MS36). Logs this session out on the server
 * (best effort), forgets the token in memory and in the keychain, keeps the
 * remembered password, blocks automatic re-login until the next sign-in in
 * any window, and tells the other windows.
 */
export async function signOutOfHost(host: ConnectHost): Promise<void> {
  const hostKey = homeHostKey(host)
  loginCoordinator.setBlock(hostKey, 'signed-out')
  const token = host.token
  useConnectHostStore.getState().dropSessionInMemory(host.id)
  hostPool.noteSignedOut(hostKey)
  sessionSync.signedOut(host)
  if (token.length > 0) {
    try {
      await fetch(`${hostBaseUrl(host)}/cli/auth/logout?token=${encodeURIComponent(token)}`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: '{}',
        signal: AbortSignal.timeout(REQUEST_TIMEOUT_MS),
      })
    } catch (err) {
      // The server is unreachable; the token is still forgotten here.
      console.warn('[host-session] logout request failed:', err)
    }
  }
  await forgetToken(host.id)
}

/** Test-only: forget every pool entry. */
export function __resetHostPoolForTests(): void {
  hostPool.store.setState({ entries: {} })
}
