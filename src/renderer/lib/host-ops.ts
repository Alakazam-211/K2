// Per-host operations for the Settings → Connections address-book tiles.
//
// Unlike `lib/daemon-cli.ts` (which always targets the ACTIVE daemon via the
// connect-host store), these helpers act on a SPECIFIC saved host — each tile
// in ConnectionsSection drives THAT host's own daemon using the host object's
// own `{base, token}`, never the active connection. This is what lets the
// owner restart / update / inspect a connected server straight from its tile
// without first switching to it.
//
// The owner-gated routes (restart / update/*) accept a session token whose
// account is an Owner/Admin on the target host (the daemon's
// `require_owner_or_admin`); a Member token 403s and the caller surfaces it.
// `settings/get` accepts any valid session token (best-effort, used only for
// the federation badge).

import type { ConnectHost } from '@/stores/connect-host'
import {
  updateAvailableCopy,
  newerNoArtifactCopy,
  type UpdateCheckResult,
} from '@/components/Settings/sections/update-host'

/** The resolved wire credentials for one saved host. */
export interface HostCreds {
  /** `<scheme>://<host>[:<port>]` — secure+443 omits the port, matching
   *  daemon-ws.ts / connect-host.ts's hostBaseUrl. */
  base: string
  /** The host's in-memory/keychain session token (rides as `?token=`).
   *  Empty string when the host is signed out. */
  token: string
}

/**
 * Build `{base, token}` straight from a saved host object — the same scheme/
 * authority rule the store uses for login (secure ⇒ https, secure+443 omits
 * the port). The token is the host's OWN session token, NOT the active
 * daemon's. An empty token means "signed out" — the caller disables the
 * owner-gated buttons.
 */
export function remoteCreds(
  h: Pick<ConnectHost, 'hostname' | 'port' | 'secure' | 'token'>,
): HostCreds {
  const scheme = h.secure ? 'https' : 'http'
  const authority = h.secure && h.port === 443 ? h.hostname : `${h.hostname}:${h.port}`
  return { base: `${scheme}://${authority}`, token: h.token ?? '' }
}

/** Surface a clean message from a daemon error body (`{"error":"…"}`) or the
 *  raw text, matching daemon-cli's parseDaemonResponse. */
async function parse<T>(res: Response): Promise<T> {
  const text = await res.text()
  if (!res.ok) {
    let msg = text
    try {
      const parsed = JSON.parse(text) as { error?: unknown }
      if (parsed && typeof parsed.error === 'string') msg = parsed.error
    } catch {
      /* fall through with raw text */
    }
    throw new Error(msg || `daemon ${res.status}`)
  }
  if (text.length === 0) return undefined as unknown as T
  try {
    return JSON.parse(text) as T
  } catch {
    return text as unknown as T
  }
}

/** POST `<base>/cli/<route>?token=<token>` to a SPECIFIC host. Owner token
 *  rides the query string (the daemon's require_owner_or_admin reads it
 *  there); no body. Throws on non-2xx with a `{"error":"…"}`-aware message. */
export async function hostOpPost<T = unknown>(
  creds: HostCreds,
  route: string,
  timeoutMs = 30000,
): Promise<T> {
  const url = `${creds.base}/cli/${route}?token=${encodeURIComponent(creds.token)}`
  const res = await fetch(url, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    signal: AbortSignal.timeout(timeoutMs),
  })
  return parse<T>(res)
}

/** GET `<base>/cli/<route>?…&token=<token>` from a SPECIFIC host. */
export async function hostOpGet<T = unknown>(
  creds: HostCreds,
  route: string,
  params?: Record<string, string | number | boolean | undefined | null>,
  timeoutMs = 8000,
): Promise<T> {
  const search = new URLSearchParams()
  if (params) {
    for (const [k, v] of Object.entries(params)) {
      if (v !== undefined && v !== null) search.set(k, String(v))
    }
  }
  search.set('token', creds.token)
  const url = `${creds.base}/cli/${route}?${search.toString()}`
  const res = await fetch(url, { method: 'GET', signal: AbortSignal.timeout(timeoutMs) })
  return parse<T>(res)
}

// ── Update-check → display string ────────────────────────────────────────

/** A digested view of a `POST /cli/daemon/update/check` response, ready to
 *  render on a tile. Reuses the host-named copy from update-host.ts. */
export type CheckSummary =
  | { kind: 'up-to-date'; text: string }
  | { kind: 'available'; text: string; latest: string }
  | { kind: 'newer-no-artifact'; text: string }

/**
 * Fold a check result into one of three display states:
 *   - `available`        → "Update available for <host> — <cur> → <latest>" (+ Update button)
 *   - `newer-no-artifact`→ a newer version exists but no build for this platform
 *   - `up-to-date`       → "<host> is up to date (v<cur>)"
 */
export function summarizeCheck(hostLabel: string, r: UpdateCheckResult): CheckSummary {
  if (r.available) {
    return { kind: 'available', text: updateAvailableCopy(hostLabel, r.current, r.latest), latest: r.latest }
  }
  if (r.newerNoArtifact) {
    return { kind: 'newer-no-artifact', text: newerNoArtifactCopy(hostLabel, r.latest, r.platform) }
  }
  return { kind: 'up-to-date', text: `${hostLabel} is up to date (v${r.current})` }
}

// ── Federation badge ─────────────────────────────────────────────────────

/** Per-tile federation state: pending fetch, the two known states, or
 *  unknown (signed out / unreachable / federation field absent). */
export type FederationState = 'loading' | 'on' | 'off' | 'unknown'

/** The short badge label for a host's federation state. */
export function federationBadgeText(s: FederationState): string {
  switch (s) {
    case 'loading':
      return 'Federation: …'
    case 'on':
      return 'Federation: on'
    case 'off':
      return 'Federation: off'
    case 'unknown':
      return 'Federation: —'
  }
}
