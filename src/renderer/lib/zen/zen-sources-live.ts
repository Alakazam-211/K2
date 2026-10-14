// Garden v3 data sources — the reader's live wiring (contract-sources-v1
// §11.4): which scope an address reads through, and when the app socket
// carries `sources_changed`. Pure logic lives in `zen-sources.ts`.
//
// - `name` (this computer) and `name::<the window's server>` read through
//   the window's own room scope (`primaryRoom().scope`, as `zen-data` does;
//   Zen never calls `primaryScope()`): its socket is open, so its
//   `sources_changed` arrives on that bus (GV44d, GV45).
// - `name::host` for another server reads through `scopeForHost(host)`
//   with your saved login there, only when that host is a SAVED server
//   (GV44); otherwise `server_not_saved`. These servers are polled.
// - `name` while the window is on another server reads this computer's
//   daemon (`scopeForHost('local')`, the local owner token), polled.

import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { useConnectHostStore } from '@/stores/connect-host'
import { primaryRoom } from '@/stores/room'
import { onSourcesChanged } from '@/stores/session-events'
import { LOCAL_HOME_HOST, canonicalHostKey, savedHostForKey } from '@/lib/host-key'
import { activeHomeHostKey } from '@/lib/home-address'
import { readSource, refreshSource, sourceHistory, sourcesChanges } from '@/lib/sources/sources-api'
import { createZenSourcesReader, type ZenSourcesDeps, type ZenSourcesReader } from './zen-sources'

function windowHostKey(): string {
  return activeHomeHostKey(useConnectHostStore.getState().activeHost)
}

/** The scope an address's host reads through, or null when it isn't saved. */
export function zenSourceScope(host: string | null): ServerScope | null {
  const key = host === null ? LOCAL_HOME_HOST : canonicalHostKey(host)
  if (key === windowHostKey()) return primaryRoom().scope
  if (key === LOCAL_HOME_HOST) return scopeForHost(LOCAL_HOME_HOST)
  if (!savedHostForKey(useConnectHostStore.getState().hosts, key)) return null
  return scopeForHost(key)
}

export const ZEN_SOURCES_LIVE_DEPS: ZenSourcesDeps = {
  scopeFor: zenSourceScope,
  read: readSource,
  history: sourceHistory,
  changes: sourcesChanges,
  refresh: refreshSource,
  // Only the window's own server has a socket we know is open.
  onChanged: (scope, fn) => (scope.isPrimary ? onSourcesChanged(scope, fn) : null),
  visible: () => typeof document === 'undefined' || document.visibilityState !== 'hidden',
  setInterval: (fn, ms) => setInterval(fn, ms),
  clearInterval: (t) => clearInterval(t as ReturnType<typeof setInterval>),
}

let reader: ZenSourcesReader | null = null
let readerWindowKey: string | null = null

/** The window's reader. A server switch (the window's server changes)
 *  starts a fresh one, so scopes and sockets follow the window. */
export function zenSourcesReader(): ZenSourcesReader {
  const key = windowHostKey()
  if (!reader || readerWindowKey !== key) {
    reader?.dispose()
    reader = createZenSourcesReader(ZEN_SOURCES_LIVE_DEPS)
    readerWindowKey = key
  }
  return reader
}
