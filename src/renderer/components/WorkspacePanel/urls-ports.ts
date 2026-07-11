// URLs & Ports drawer — pure derivation logic, split from
// `UrlsPortsSection.tsx` so it's unit-testable without React/DOM.
//
// The daemon's `/cli/tunnel/subdomains` map carries nested LABELS and
// their internal targets (`staging → localhost:3000`); the public host a
// browser reaches each label at is `<label>.<primary-host>` — i.e. the
// label prefixed onto the tunnel's own public host (`rosson.k2.dev`).
// Deriving from `public_url` first keeps us honest against whatever host
// the daemon actually predicted; the `primary` label + the canonical
// `k2.dev` suffix is the fallback when the tunnel status doesn't carry a
// URL (e.g. tunnel stopped but the map is still cached).

/** The canonical K2 Connect subdomain suffix — mirrors
 *  `k2_core::tunnel::config::SUBDOMAIN_HOST`. Fallback only; a live
 *  `public_url` always wins. */
export const SUBDOMAIN_HOST = 'k2.dev'

/** Derive the public https URL for a nested subdomain label.
 *
 *  Preference order:
 *  1. `publicUrl` (`https://<primary-host>`) → `https://<label>.<primary-host>`
 *  2. `primary` label → `https://<label>.<primary>.k2.dev`
 *  3. neither known → `null` (the caller renders the label + target only —
 *     never fabricate a host we can't actually predict).
 */
export function nestedPublicUrl(
  label: string,
  primary: string,
  publicUrl: string | null,
): string | null {
  const cleanLabel = label.trim()
  if (!cleanLabel) return null
  const url = (publicUrl ?? '').trim().replace(/\/+$/, '')
  if (url.startsWith('https://')) {
    const host = url.slice('https://'.length)
    if (host) return `https://${cleanLabel}.${host}`
  }
  const cleanPrimary = primary.trim()
  if (cleanPrimary) return `https://${cleanLabel}.${cleanPrimary}.${SUBDOMAIN_HOST}`
  return null
}

/** Stable render order for the nested-target rows: sorted by label. The
 *  daemon serializes a HashMap, so wire order is arbitrary — sorting keeps
 *  the table from reshuffling across refreshes. */
export function sortedTargets(targets: Record<string, string>): Array<[string, string]> {
  return Object.entries(targets).sort(([a], [b]) => a.localeCompare(b))
}

/** Count for the section-header badge: the tunnel's own public URL (when
 *  running with a URL) + every nested target. Honest zero when the tunnel
 *  is down and no map is cached. */
export function urlCount(
  running: boolean,
  publicUrl: string | null,
  targets: Record<string, string>,
): number {
  return (running && publicUrl ? 1 : 0) + Object.keys(targets).length
}
