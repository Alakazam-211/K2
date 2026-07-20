/**
 * localStorage key for the non-secret Connect host address book.
 *
 * Canonical writes always go to `k2.*`. Pre-rename web/desktop builds used
 * `k2so.*` — dual-read prefers the new key, falls back to legacy, and
 * migrate-on-write drops the legacy entry so we stop accumulating k2so debt.
 */

/** Canonical localStorage key (all writes go here). */
export const CONNECT_HOSTS_STORAGE_KEY = 'k2.connect-hosts.v1'

/** Pre-rename key; dual-read only, never written. */
export const LEGACY_CONNECT_HOSTS_STORAGE_KEY = 'k2so.connect-hosts.v1'

/** Dual-read: prefer `k2.*`, fall back to `k2so.*`. */
export function readConnectHostsStorage(
  storage: Pick<Storage, 'getItem'>,
): string | null {
  return (
    storage.getItem(CONNECT_HOSTS_STORAGE_KEY) ??
    storage.getItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY)
  )
}

/**
 * Write the canonical key and remove the legacy key (migrate-on-write).
 * Callers still own try/catch for quota / private-mode failures.
 */
export function writeConnectHostsStorage(
  storage: Pick<Storage, 'setItem' | 'removeItem'>,
  json: string,
): void {
  storage.setItem(CONNECT_HOSTS_STORAGE_KEY, json)
  storage.removeItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY)
}

/** Clear both keys (tests / full reset). */
export function clearConnectHostsStorage(
  storage: Pick<Storage, 'removeItem'>,
): void {
  storage.removeItem(CONNECT_HOSTS_STORAGE_KEY)
  storage.removeItem(LEGACY_CONNECT_HOSTS_STORAGE_KEY)
}
