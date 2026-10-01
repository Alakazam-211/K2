// Home M1 — MS6 / MS64: per-window view state that belongs to one server's
// workspace is stored under `<hostKey>|<key>`, so two servers with the same
// project ids, session ids or paths never read or write the same entry.
//
// `hostScopedKey(hostKey, key)` builds the key. The first call in a webview
// runs a one-time migration (marker HOST_SCOPED_KEYS_MARKER) of the three
// older formats:
//
//   1. Unprefixed keys (UNPREFIXED_KEY_PREFIXES, and the entries inside the
//      `k2so:selected-tabs` map) move to `local|<key>`. A window always
//      boots on `local`, so that is where they were most likely written.
//      Some were written while the window was on a remote; under `local|`
//      they become harmless orphans.
//   2. Keys already keyed by the store's `activeHostKey` (`local` or
//      `<id>:<hostname>:<port>`) — the launch bar, the session view tab and
//      split, and the project-group read cursor — map the id through the
//      saved-server list to its host key. An id that is no longer saved is
//      dropped.
//   3. Nothing else is touched. In particular `k2.homes.v1` and
//      `k2.connect-hosts.v1` stay as they are.

import { homeHostKey, LOCAL_HOME_HOST } from '@/lib/host-key'
import { useConnectHostStore, type ConnectHost } from '@/stores/connect-host'
import { onSavedHostRekey } from '@/lib/connect-host-hooks'

export const HOST_SCOPED_KEYS_MARKER = 'k2.hostScopedKeys.v1'

/** Format 1: per-workspace / per-session keys written with no server. */
export const UNPREFIXED_KEY_PREFIXES: readonly string[] = [
  'k2:composer:draft:',
  'k2:composer:caret:',
  'urls-ports.section-collapsed.',
  'worktrees.section-collapsed.',
  'workspace-api.section-collapsed.',
  'connected-agents.section-collapsed.',
  'heartbeats.archive-collapsed.',
  'heartbeats.section-collapsed.',
]

/** The selected-tab map: one item whose ENTRIES are re-keyed. */
export const SELECTED_TABS_STORAGE_KEY = 'k2so:selected-tabs'

/** Format 2: `<prefix><activeHostKey>[:<rest>]` → `<hostKey>|<base><rest>`.
 *  `hasRest` is false when the old key ends at the host. `base` is what the
 *  reader now asks for (e.g. `presets.ts` `showLaunchBarStorageKey`). */
export const ACTIVE_HOST_KEYED: ReadonlyArray<{ prefix: string; hasRest: boolean; base: string }> = [
  { prefix: 'k2.showLaunchBar.', hasRest: false, base: 'k2.showLaunchBar' },
  { prefix: 'k2:session-view-tab:', hasRest: true, base: 'k2:session-view-tab:' },
  { prefix: 'k2:session-view-split:', hasRest: true, base: 'k2:session-view-split:' },
  { prefix: 'k2:project-groups:last-seen:', hasRest: true, base: 'k2:project-groups:last-seen:' },
]

export function hostScopedKey(hostKey: string, key: string): string {
  ensureHostScopedKeysMigrated()
  return `${hostKey}|${key}`
}

/** Minimal storage surface (localStorage shape) so tests can pass a fake. */
export interface MigratableStorage {
  readonly length: number
  key(index: number): string | null
  getItem(key: string): string | null
  setItem(key: string, value: string): void
  removeItem(key: string): void
}

/** Map an old `activeHostKey` prefix at the start of `tail` to a host key.
 *  Returns the host key and what follows it, or null when the id is not
 *  saved any more. */
function splitActiveHostKey(
  tail: string,
  hasRest: boolean,
  hosts: ReadonlyArray<Pick<ConnectHost, 'id' | 'hostname' | 'port' | 'secure'>>,
): { hostKey: string; rest: string } | null {
  const candidates: Array<{ old: string; hostKey: string }> = [
    { old: LOCAL_HOME_HOST, hostKey: LOCAL_HOME_HOST },
    ...hosts.map((h) => ({ old: `${h.id}:${h.hostname}:${h.port}`, hostKey: homeHostKey(h) })),
  ]
  for (const c of candidates) {
    if (!hasRest && tail === c.old) return { hostKey: c.hostKey, rest: '' }
    if (hasRest && tail.startsWith(`${c.old}:`)) {
      return { hostKey: c.hostKey, rest: tail.slice(c.old.length + 1) }
    }
  }
  return null
}

/** Run the one-time migration on `storage`. Idempotent: does nothing once
 *  the marker is set. Returns true when it ran. */
export function migrateHostScopedKeys(
  storage: MigratableStorage,
  hosts: ReadonlyArray<Pick<ConnectHost, 'id' | 'hostname' | 'port' | 'secure'>>,
): boolean {
  if (storage.getItem(HOST_SCOPED_KEYS_MARKER) !== null) return false
  const keys: string[] = []
  for (let i = 0; i < storage.length; i++) {
    const k = storage.key(i)
    if (k !== null) keys.push(k)
  }
  for (const k of keys) {
    if (k.includes('|')) continue
    const value = storage.getItem(k)
    if (value === null) continue
    if (UNPREFIXED_KEY_PREFIXES.some((p) => k.startsWith(p))) {
      storage.setItem(`${LOCAL_HOME_HOST}|${k}`, value)
      storage.removeItem(k)
      continue
    }
    const keyed = ACTIVE_HOST_KEYED.find((e) => k.startsWith(e.prefix))
    if (keyed) {
      const split = splitActiveHostKey(k.slice(keyed.prefix.length), keyed.hasRest, hosts)
      storage.removeItem(k)
      if (split) storage.setItem(`${split.hostKey}|${keyed.base}${split.rest}`, value)
    }
  }
  const selected = storage.getItem(SELECTED_TABS_STORAGE_KEY)
  if (selected !== null) {
    let parsed: unknown = null
    try {
      parsed = JSON.parse(selected)
    } catch {
      parsed = null
    }
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      const out: Record<string, string> = {}
      for (const [k, v] of Object.entries(parsed as Record<string, unknown>)) {
        if (typeof v !== 'string') continue
        out[k.includes('|') ? k : `${LOCAL_HOME_HOST}|${k}`] = v
      }
      storage.setItem(SELECTED_TABS_STORAGE_KEY, JSON.stringify(out))
    }
  }
  storage.setItem(HOST_SCOPED_KEYS_MARKER, '1')
  return true
}

let migrated = false

/** Run the migration once per webview against the real localStorage. Safe
 *  where localStorage is missing (node tests, headless). */
export function ensureHostScopedKeysMigrated(): void {
  if (migrated) return
  migrated = true
  let storage: Storage | undefined
  try {
    storage = typeof localStorage === 'undefined' ? undefined : localStorage
  } catch {
    storage = undefined
  }
  if (!storage || typeof storage.key !== 'function') return
  try {
    migrateHostScopedKeys(storage, useConnectHostStore.getState().hosts)
  } catch (err) {
    // Storage full / blocked: the old keys stay where they are and the app
    // reads fresh prefixed keys. Nothing is lost; log it.
    console.warn('[host-scoped-storage] migration skipped:', err)
  }
}

export function __resetHostScopedKeysMigrationForTests(): void {
  migrated = false
}

/** MS61: a saved server's host key changed. Move every `<oldKey>|…` key
 *  (and every `<oldKey>|…` entry of the selected-tab map) to `<newKey>|…`.
 *  An entry already written under the new key wins. Idempotent, so every
 *  window may run it. Returns how many items moved. */
export function rekeyHostScopedStorage(storage: MigratableStorage, oldKey: string, newKey: string): number {
  if (oldKey === newKey) return 0
  const from = `${oldKey}|`
  const to = `${newKey}|`
  const keys: string[] = []
  for (let i = 0; i < storage.length; i++) {
    const k = storage.key(i)
    if (k !== null && k.startsWith(from)) keys.push(k)
  }
  let moved = 0
  for (const k of keys) {
    const value = storage.getItem(k)
    storage.removeItem(k)
    if (value === null) continue
    const target = to + k.slice(from.length)
    if (storage.getItem(target) === null) storage.setItem(target, value)
    moved += 1
  }
  const selected = storage.getItem(SELECTED_TABS_STORAGE_KEY)
  if (selected !== null) {
    let parsed: unknown = null
    try {
      parsed = JSON.parse(selected)
    } catch {
      parsed = null
    }
    if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
      const map = parsed as Record<string, unknown>
      let changed = false
      for (const k of Object.keys(map)) {
        if (!k.startsWith(from)) continue
        const target = to + k.slice(from.length)
        if (!(target in map)) map[target] = map[k]
        delete map[k]
        changed = true
        moved += 1
      }
      if (changed) storage.setItem(SELECTED_TABS_STORAGE_KEY, JSON.stringify(map))
    }
  }
  return moved
}

onSavedHostRekey((oldKey, newKey) => {
  let storage: Storage | undefined
  try {
    storage = typeof localStorage === 'undefined' ? undefined : localStorage
  } catch {
    storage = undefined
  }
  if (!storage || typeof storage.key !== 'function') return
  try {
    rekeyHostScopedStorage(storage, oldKey, newKey)
  } catch (err) {
    console.warn('[host-scoped-storage] re-key skipped:', err)
  }
})
