// Published drawer + K2 Connect settings — pure derivation logic, split
// from the components so it's unit-testable without React/DOM.
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

// ── Published services (daemon-owned `k2 publish run`) ───────────────────
//
// The drawer unions (a) GET /cli/publish/list rows for this workspace with
// (b) attributed nested URLs whose label does not match a service name
// (BYO `k2 publish subdomain create`). Helpers live here so the union /
// local-only / Start-vs-Stop rules are unit-testable without React.

/** Canonical empty-state example — not bare `k2 publish`. */
export const PUBLISH_RUN_EXAMPLE = 'k2 publish run <name> --cmd "…" --port <n>'

/** One row from GET `/cli/publish/list` after wire normalization. */
export interface PublishedService {
  name: string
  cmd: string
  cwd: string
  port: number | null
  expose: string
  desired: string
  status: string
  pid: number | null
  url: string | null
  target: string | null
  error: string | null
  lastExitCode: number | null
}

function asString(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function asStringOrNull(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
}

function asFiniteOrNull(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

function parsePublishedService(raw: unknown): PublishedService | null {
  if (!raw || typeof raw !== 'object') return null
  const o = raw as Record<string, unknown>
  const name = asString(o.name).trim()
  if (!name) return null
  return {
    name,
    cmd: asString(o.cmd),
    cwd: asString(o.cwd),
    port: asFiniteOrNull(o.port),
    expose: asString(o.expose),
    desired: asString(o.desired),
    status: asString(o.status),
    pid: asFiniteOrNull(o.pid),
    url: asStringOrNull(o.url),
    target: asStringOrNull(o.target),
    error: asStringOrNull(o.error),
    lastExitCode: asFiniteOrNull(o.lastExitCode ?? o.last_exit_code),
  }
}

/** Parse `{ services: [...] }` from GET `/cli/publish/list`. Junk / nameless
 *  entries are dropped rather than rendered as broken rows. */
export function parsePublishList(raw: unknown): PublishedService[] {
  if (!raw || typeof raw !== 'object') return []
  const services = (raw as { services?: unknown }).services
  if (!Array.isArray(services)) return []
  const out: PublishedService[] = []
  for (const item of services) {
    const parsed = parsePublishedService(item)
    if (parsed) out.push(parsed)
  }
  return out
}

/** Attributed nested URLs that have no matching published-service name —
 *  the BYO `subdomain create` leftover rows. */
export function byoWorkspaceTargets(
  attributed: Record<string, SubdomainTargetInfo>,
  services: readonly PublishedService[],
): Record<string, SubdomainTargetInfo> {
  const names = new Set(services.map((s) => s.name))
  const out: Record<string, SubdomainTargetInfo> = {}
  for (const [label, info] of Object.entries(attributed)) {
    if (!names.has(label)) out[label] = info
  }
  return out
}

/** Local-only rows have no public link (`expose === 'local'` or no url). */
export function isLocalOnly(service: PublishedService): boolean {
  return service.expose === 'local' || !service.url
}

/** Public URL for a service row — null when local-only. */
export function servicePublicUrl(service: PublishedService): string | null {
  if (isLocalOnly(service)) return null
  return service.url
}

/** `localhost:<port>` when the daemon gave a port, else the target. */
export function serviceListenLabel(service: PublishedService): string {
  if (service.port !== null) return `localhost:${service.port}`
  return service.target ?? ''
}

/** Stop is shown while the child is up or still coming up. */
export function isServiceStoppable(service: PublishedService): boolean {
  return service.status === 'running' || service.status === 'starting'
}

/** Stable render order for service rows: sorted by name. */
export function sortedServices(services: readonly PublishedService[]): PublishedService[] {
  return [...services].sort((a, b) => a.name.localeCompare(b.name))
}

/** One-line claim hint for unattributed nested URLs. Null when none. */
export function unattributedHint(count: number): string | null {
  if (count <= 0) return null
  const noun = count === 1 ? '1 nested URL is' : `${count} nested URLs are`
  return `${noun} not claimed to a workspace — k2 publish subdomain claim <label>`
}
