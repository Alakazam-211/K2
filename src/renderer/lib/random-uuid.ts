/**
 * UUID helper that works outside secure contexts.
 *
 * `crypto.randomUUID` is only defined in secure contexts (HTTPS / localhost).
 * Hosted web over plain HTTP (LAN IP, remote vite:dev:web) throws
 * `TypeError: crypto.randomUUID is not a function` — which killed toasts
 * and tab restore on drop. Prefer the native API; fall back to
 * `getRandomValues` (widely available even on non-secure HTTP).
 */

function uuidFromGetRandomValues(): string {
  const bytes = new Uint8Array(16)
  if (typeof crypto !== 'undefined' && typeof crypto.getRandomValues === 'function') {
    crypto.getRandomValues(bytes)
  } else {
    // Last-resort non-crypto fallback (should not hit in any real browser).
    for (let i = 0; i < 16; i++) bytes[i] = (Math.random() * 256) | 0
  }
  // RFC 4122 version 4
  bytes[6] = (bytes[6]! & 0x0f) | 0x40
  bytes[8] = (bytes[8]! & 0x3f) | 0x80
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`
}

/** Generate a UUID v4 string. Safe on HTTP (non-secure) contexts. */
export function randomUUID(): string {
  if (
    typeof crypto !== 'undefined' &&
    typeof crypto.randomUUID === 'function'
  ) {
    return crypto.randomUUID()
  }
  return uuidFromGetRandomValues()
}

/**
 * Install `crypto.randomUUID` when missing so existing call sites keep
 * working without a tree-wide rewrite. Call once at renderer boot.
 */
export function installRandomUUIDPolyfill(): void {
  if (typeof crypto === 'undefined') return
  if (typeof crypto.randomUUID === 'function') return
  try {
    Object.defineProperty(crypto, 'randomUUID', {
      value: uuidFromGetRandomValues,
      configurable: true,
      writable: true,
    })
  } catch {
    // Some environments freeze crypto; call sites should use randomUUID().
  }
}
