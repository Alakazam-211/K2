// Settings → K2 Connect → URLs: add, repoint and remove published
// (nested) subdomains — the desktop twin of
// `k2 publish subdomain create | point | rm`.
//
// The CONTROL-PLANE half (connect.k2.dev `/subdomains`) is not called
// from the renderer: the tunnel token lives in the daemon's vault, so the
// daemon makes the call for us through its Connect proxy route
// (`POST /cli/connect/proxy {method, endpoint, body?}` → `{status, body}`,
// allowlisted to the publish/nested-subdomain endpoints). The request
// bodies below are byte-for-byte what `cli/k2` sends through
// `_connect_curl`:
//   create  POST   /subdomains          {"label": <label>, "target": <target>}
//   point   PUT    /subdomains/<label>  {"target": <target>}
//   rm      DELETE /subdomains/<label>  (no body)
//
// The LOCAL-daemon half is the same as the CLI's too: the 0074 workspace
// attribution (`/cli/publish/subdomain/claim` with `check=1` before the
// control-plane call, then the stamp after it; `unclaim` after a remove)
// and the map-refresh nudge (`/cli/tunnel/subdomains/refresh`) so the new
// URL shows without waiting for the 60 s poll.

import { daemonCliPost, daemonCliPostQuery } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

/** The daemon route that performs Connect control-plane calls with the
 *  vault-held tunnel token. INTEGRATOR: confirm this name against the
 *  daemon route when both land on main. */
export const CONNECT_PROXY_ROUTE = 'connect/proxy'

export const SUBDOMAIN_HOST = 'k2.dev'

/** Box-level front doors, never a publish target. Same set as
 *  `k2_core::skin::RESERVED_NESTED_LABELS` and the CLI's
 *  `_refuse_reserved_nested_label`. */
export const RESERVED_NESTED_LABELS: Record<string, string> = {
  skin: "'skin' is reserved for your Apps' front door. Pick another label.",
  meet: "'meet' is reserved for K2 Meet (meet.<box>.k2.dev). Pick another label.",
  chat: "'chat' is reserved for K2 chat (chat.<box>.k2.dev). Pick another label.",
  mail: "'mail' is reserved for K2 webmail (mail.<box>.k2.dev). Pick another label.",
}

/** Labels the control plane itself refuses (`NESTED_RESERVED`). */
const CONTROL_PLANE_RESERVED = new Set(['cert', 'connect'])

export type Checked<T> = { ok: true; value: T } | { ok: false; error: string }

/** Same rules as the control plane's `validate_label`: a clean DNS label
 *  as typed (lowercase a–z, 0–9, single dashes, no dash at either end),
 *  3–63 characters, not all digits, and not a reserved name. */
export function validateLabel(raw: string): Checked<string> {
  const label = raw.trim()
  if (!label) return { ok: false, error: 'Enter a label.' }
  if (/[A-Z]/.test(label)) return { ok: false, error: 'Use lowercase letters only.' }
  if (!/^[a-z0-9-]+$/.test(label)) {
    return { ok: false, error: 'Use lowercase letters, digits and dashes only.' }
  }
  if (label.startsWith('-') || label.endsWith('-')) {
    return { ok: false, error: "A label can't start or end with a dash." }
  }
  if (label.includes('--')) return { ok: false, error: "A label can't have two dashes in a row." }
  if (label.length < 3) return { ok: false, error: 'A label needs at least 3 characters.' }
  if (label.length > 63) return { ok: false, error: 'A label can have at most 63 characters.' }
  if (/^[0-9]+$/.test(label)) return { ok: false, error: "A label can't be only digits." }
  const reserved = RESERVED_NESTED_LABELS[label]
  if (reserved) return { ok: false, error: reserved }
  if (CONTROL_PLANE_RESERVED.has(label)) {
    return { ok: false, error: `'${label}' is reserved. Pick another label.` }
  }
  return { ok: true, value: label }
}

export function isReservedNestedLabel(label: string): boolean {
  return Object.prototype.hasOwnProperty.call(RESERVED_NESTED_LABELS, label.trim().toLowerCase())
}

/** Same rules as the control plane's `validate_target`: `host:port`, no
 *  scheme, no spaces, port 1–65535. */
export function validateTarget(raw: string): Checked<string> {
  const target = raw.trim()
  if (!target) return { ok: false, error: 'Enter a target, like localhost:3000.' }
  if (/\s/.test(target)) return { ok: false, error: "A target can't contain spaces." }
  if (target.includes('://')) {
    return { ok: false, error: 'Leave out http:// — use host:port, like localhost:3000.' }
  }
  const colon = target.lastIndexOf(':')
  if (colon < 0) return { ok: false, error: 'Use host:port, like localhost:3000.' }
  const host = target.slice(0, colon)
  const port = target.slice(colon + 1)
  if (!host) return { ok: false, error: 'Use host:port, like localhost:3000.' }
  if (!/^[0-9]+$/.test(port)) return { ok: false, error: `'${port}' is not a port number.` }
  const n = Number(port)
  if (n < 1 || n > 65535) return { ok: false, error: 'The port must be between 1 and 65535.' }
  return { ok: true, value: target }
}

/** The full host the label will get: `<label>.<sub>.k2.dev`. Uses the
 *  running tunnel's public host when it has one (same rule as
 *  `nestedPublicUrl`), else the primary label. Placeholders stand in for
 *  whatever isn't known yet — never a guessed host. */
export function previewHost(label: string, primary: string, publicUrl: string | null): string {
  const l = label.trim().toLowerCase() || '<label>'
  const url = (publicUrl ?? '').trim().replace(/\/+$/, '')
  if (url.startsWith('https://') && url.length > 'https://'.length) {
    return `${l}.${url.slice('https://'.length)}`
  }
  const p = primary.trim()
  return `${l}.${p || '<your subdomain>'}.${SUBDOMAIN_HOST}`
}

// ── Control-plane requests (mirrors cli/k2 `_connect_curl` callers) ──────

export interface ConnectProxyRequest {
  method: 'POST' | 'PUT' | 'DELETE'
  endpoint: string
  body?: Record<string, string>
}

export interface ConnectProxyResponse {
  status: number
  body: unknown
}

export function createRequest(label: string, target: string): ConnectProxyRequest {
  return { method: 'POST', endpoint: '/subdomains', body: { label, target } }
}

export function updateTargetRequest(label: string, target: string): ConnectProxyRequest {
  return { method: 'PUT', endpoint: `/subdomains/${encodeURIComponent(label)}`, body: { target } }
}

export function removeRequest(label: string): ConnectProxyRequest {
  return { method: 'DELETE', endpoint: `/subdomains/${encodeURIComponent(label)}` }
}

// ── Error text (the CLI's `_connect_subdomain_render` table) ─────────────

export const PLAN_DASHBOARD_URL = 'https://k2.dev/dashboard'

export const CONTROL_PLANE_MESSAGES: Record<string, string> = {
  bad_label: 'Invalid label (use lowercase letters, digits, and dashes).',
  bad_target: 'Invalid target endpoint (e.g. localhost:3000).',
  label_taken: 'That label is already taken.',
  label_is_primary: 'That label is your primary subdomain — it cannot be reassigned here.',
  primary_undeletable: 'Your primary subdomain cannot be removed.',
  not_found: 'No such subdomain.',
  invalid_token:
    'Tunnel token rejected — re-select your subdomain in Settings -> K2 Connect to refresh it.',
}

/** The daemon's answer when it has no such route (older daemon). */
export const PROXY_MISSING_MESSAGE =
  "This K2 server can't manage published subdomains from Settings yet. Update it, or use `k2 publish subdomain create` in a terminal."

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

/** Map a non-2xx control-plane answer to the CLI's message. `body` is the
 *  parsed JSON (`{error, detail}`) or the raw text. */
export function controlPlaneErrorMessage(status: number, body: unknown): string {
  const obj = body && typeof body === 'object' ? (body as Record<string, unknown>) : null
  const err = obj ? str(obj.error) : ''
  const detail = obj ? str(obj.detail) : ''
  if (err === 'pro_required') {
    return `${detail || 'This subdomain'} needs the Pro plan to create nested subdomains. Upgrade your plan on the dashboard: ${PLAN_DASHBOARD_URL}`
  }
  const known = CONTROL_PLANE_MESSAGES[err]
  if (known) return detail ? `${known} (${detail})` : known
  if (status === 401) return 'Unauthorized — your tunnel token is missing or expired.'
  if (err) return `Control plane error (${err})${detail ? `: ${detail}` : ''}`
  const raw = typeof body === 'string' ? body.trim() : obj ? JSON.stringify(obj) : ''
  return `Control plane returned HTTP ${status}${raw ? `: ${raw.slice(0, 200)}` : ''}`
}

export class PublishedSubdomainError extends Error {
  /** The control-plane error code when there was one (`label_taken`, …). */
  readonly code: string | null
  constructor(message: string, code: string | null = null) {
    super(message)
    this.name = 'PublishedSubdomainError'
    this.code = code
  }
}

/** A thrown daemon error (the proxy route itself failed) → user text. A
 *  daemon that passes the control plane's status straight through throws
 *  with the bare code as its message, so known codes map here too. */
export function proxyFailureMessage(err: unknown): string {
  const msg = err instanceof Error ? err.message.trim() : String(err ?? '').trim()
  if (msg === 'route not found') return PROXY_MISSING_MESSAGE
  if (msg === 'pro_required') return controlPlaneErrorMessage(403, { error: msg })
  if (CONTROL_PLANE_MESSAGES[msg]) return CONTROL_PLANE_MESSAGES[msg]
  return msg || 'The K2 server did not answer.'
}

async function callConnect(scope: ServerScope, req: ConnectProxyRequest): Promise<unknown> {
  let resp: ConnectProxyResponse
  try {
    resp = await daemonCliPost<ConnectProxyResponse>(scope, CONNECT_PROXY_ROUTE, req)
  } catch (e) {
    const msg = e instanceof Error ? e.message.trim() : ''
    throw new PublishedSubdomainError(
      proxyFailureMessage(e),
      CONTROL_PLANE_MESSAGES[msg] || msg === 'pro_required' ? msg : null,
    )
  }
  if (!resp || typeof resp !== 'object' || typeof resp.status !== 'number') {
    throw new PublishedSubdomainError('The K2 server sent an answer Settings could not read.')
  }
  if (resp.status < 200 || resp.status >= 300) {
    const b = resp.body
    const code = b && typeof b === 'object' ? str((b as Record<string, unknown>).error) || null : null
    throw new PublishedSubdomainError(controlPlaneErrorMessage(resp.status, b), code)
  }
  return resp.body
}

// ── Local-daemon half (0074 attribution + map refresh) ───────────────────

const CLAIM_ROUTE = 'publish/subdomain/claim'
const UNCLAIM_ROUTE = 'publish/subdomain/unclaim'
const REFRESH_ROUTE = 'tunnel/subdomains/refresh'

/** Ask the daemon (writes nothing) whether `projectId` may take `label`
 *  BEFORE the control-plane call, so a name another workspace on this
 *  server owns is never touched. Throws the daemon's reason. */
async function claimCheck(scope: ServerScope, label: string, projectId: string): Promise<void> {
  try {
    await daemonCliPostQuery(scope, CLAIM_ROUTE, { label, project: projectId, check: 1 })
  } catch (e) {
    throw new PublishedSubdomainError(e instanceof Error ? e.message : String(e))
  }
}

/** Ask the daemon to re-pull the map now. Best effort, like the CLI's
 *  `_nudge_subdomain_refresh`: the control-plane change already happened
 *  and the map converges on the next poll anyway. Returns whether it ran. */
async function refreshMap(scope: ServerScope): Promise<boolean> {
  try {
    await daemonCliPostQuery(scope, REFRESH_ROUTE)
    return true
  } catch {
    return false
  }
}

export interface MutationResult {
  /** The daemon re-pulled the map (else it updates within a minute). */
  refreshed: boolean
  /** The control-plane change worked but a follow-up didn't. */
  warning: string | null
}

export interface OwnerOption {
  /** `projects.id` of the owning workspace; null = the server (none). */
  projectId: string | null
}

export interface PublishedSubdomainsApi {
  create(label: string, target: string, opts?: OwnerOption): Promise<MutationResult>
  updateTarget(label: string, target: string, opts?: OwnerOption): Promise<MutationResult>
  remove(label: string, opts?: OwnerOption): Promise<MutationResult>
}

function must<T>(c: Checked<T>): T {
  if (!c.ok) throw new PublishedSubdomainError(c.error)
  return c.value
}

/** The API for `scope`'s server (Settings passes the window's server). */
export function publishedSubdomains(scope: ServerScope): PublishedSubdomainsApi {
  return {
    async create(rawLabel, rawTarget, opts) {
      const label = must(validateLabel(rawLabel))
      const target = must(validateTarget(rawTarget))
      const projectId = opts?.projectId ?? null
      if (projectId) await claimCheck(scope, label, projectId)
      await callConnect(scope, createRequest(label, target))
      let warning: string | null = null
      if (projectId) {
        try {
          await daemonCliPostQuery(scope, CLAIM_ROUTE, { label, project: projectId, target })
        } catch (e) {
          warning = `Created, but the owning workspace wasn't recorded: ${e instanceof Error ? e.message : String(e)}. Adopt it from that workspace with: k2 publish subdomain claim ${label}`
        }
      }
      return { refreshed: await refreshMap(scope), warning }
    },

    async updateTarget(rawLabel, rawTarget, opts) {
      const label = rawLabel.trim().toLowerCase()
      if (!label) throw new PublishedSubdomainError('Enter a label.')
      const reserved = RESERVED_NESTED_LABELS[label]
      if (reserved) throw new PublishedSubdomainError(reserved)
      const target = must(validateTarget(rawTarget))
      const projectId = opts?.projectId ?? null
      if (projectId) await claimCheck(scope, label, projectId)
      await callConnect(scope, updateTargetRequest(label, target))
      let warning: string | null = null
      if (projectId) {
        try {
          await daemonCliPostQuery(scope, CLAIM_ROUTE, { label, project: projectId, target })
        } catch (e) {
          warning = `Repointed, but the workspace record wasn't updated: ${e instanceof Error ? e.message : String(e)}.`
        }
      }
      return { refreshed: await refreshMap(scope), warning }
    },

    async remove(rawLabel, opts) {
      const label = rawLabel.trim().toLowerCase()
      if (!label) throw new PublishedSubdomainError('Enter a label.')
      await callConnect(scope, removeRequest(label))
      let warning: string | null = null
      const projectId = opts?.projectId ?? null
      if (projectId) {
        try {
          await daemonCliPostQuery(scope, UNCLAIM_ROUTE, { label, project: projectId })
        } catch (e) {
          warning = `Removed, but the workspace record wasn't cleared: ${e instanceof Error ? e.message : String(e)}. Clear it from that workspace with: k2 publish subdomain unclaim ${label}`
        }
      }
      return { refreshed: await refreshMap(scope), warning }
    },
  }
}

/** Who sees the add / change / remove controls: the LOCAL server's owner
 *  or an admin. A remote host's tunnel is managed on that machine (the
 *  same rule that hides the rest of the Connect controls there). */
export function canManagePublishedSubdomains(isRemote: boolean, role: string | null): boolean {
  if (isRemote) return false
  return role === 'owner' || role === 'admin'
}
