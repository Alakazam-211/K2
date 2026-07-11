// URLs drawer + K2 Connect settings — pure derivation logic, split from
// the components so it's unit-testable without React/DOM.
//
// The daemon's `/cli/tunnel/subdomains` map carries nested LABELS, their
// internal targets and (0074) the attributed workspace's project id
// (`staging → { target: localhost:3000, projectId: <uuid>|null }`); the
// public host a browser reaches each label at is `<label>.<primary-host>`
// — i.e. the label prefixed onto the tunnel's own public host
// (`rosson.k2.dev`). Deriving from `public_url` first keeps us honest
// against whatever host the daemon actually predicted; the `primary`
// label + the canonical `k2.dev` suffix is the fallback when the tunnel
// status doesn't carry a URL (e.g. tunnel stopped but the map is still
// cached).

/** The canonical K2 Connect subdomain suffix — mirrors
 *  `k2_core::tunnel::config::SUBDOMAIN_HOST`. Fallback only; a live
 *  `public_url` always wins. */
export const SUBDOMAIN_HOST = 'k2.dev'

/** One nested target after normalization: the internal endpoint plus the
 *  attributed workspace's `projects.id` (null = unattributed). Mirrors the
 *  daemon's `SubdomainTargetWire`. */
export interface SubdomainTargetInfo {
  target: string
  projectId: string | null
}

/** Normalize a wire `targets` value into `label → {target, projectId}`.
 *
 *  Accepts BOTH shapes honestly:
 *  - 0074 daemons: `{ label: { target, projectId } }`
 *  - pre-0074 daemons (remote hosts on older builds): `{ label: "host:port" }`
 *    → projectId null (unattributed — the older daemon has no attribution).
 *  Junk entries (non-string / non-object values, blank targets) are
 *  dropped rather than rendered as broken rows. */
export function normalizeTargets(raw: unknown): Record<string, SubdomainTargetInfo> {
  const out: Record<string, SubdomainTargetInfo> = {}
  if (!raw || typeof raw !== 'object') return out
  for (const [label, value] of Object.entries(raw as Record<string, unknown>)) {
    if (typeof value === 'string') {
      if (value.trim()) out[label] = { target: value, projectId: null }
      continue
    }
    if (value && typeof value === 'object') {
      const target = (value as { target?: unknown }).target
      if (typeof target !== 'string' || !target.trim()) continue
      const pid = (value as { projectId?: unknown }).projectId
      out[label] = { target, projectId: typeof pid === 'string' && pid ? pid : null }
    }
  }
  return out
}

/** The subset of `targets` attributed to `projectId` — what the
 *  workspace-scoped drawer section renders. Empty projectId matches
 *  nothing (never leak server-wide rows into an unresolved workspace). */
export function workspaceTargets(
  targets: Record<string, SubdomainTargetInfo>,
  projectId: string,
): Record<string, SubdomainTargetInfo> {
  const out: Record<string, SubdomainTargetInfo> = {}
  if (!projectId) return out
  for (const [label, info] of Object.entries(targets)) {
    if (info.projectId === projectId) out[label] = info
  }
  return out
}

/** How many server-wide nested URLs have NO workspace attribution — the
 *  drawer's claim-hint counter (`k2 publish subdomain claim <label>`). */
export function unattributedCount(targets: Record<string, SubdomainTargetInfo>): number {
  return Object.values(targets).filter((t) => t.projectId === null).length
}

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
export function sortedTargets(
  targets: Record<string, SubdomainTargetInfo>,
): Array<[string, SubdomainTargetInfo]> {
  return Object.entries(targets).sort(([a], [b]) => a.localeCompare(b))
}
