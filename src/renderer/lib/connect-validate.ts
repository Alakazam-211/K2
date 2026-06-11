// Validation for a candidate K2 Connect host before committing a switch
// (PRD §1 full-screen sign-in: "validate via /boot-status + token →
// connect"). Shared by the full-screen sign-in and the Settings →
// Connections "Test connection" affordance.
//
// We do NOT route this through `getDaemonWs()` (which reads the ACTIVE
// host) — sign-in validates a host the user is ABOUT to select, before
// it's active. So we build the base URL from the candidate fields
// directly, mirroring daemon-ws.ts's scheme/port rules.

import type { ConnectHost } from '@/stores/connect-host'

/** The fields needed to reach a daemon — a subset of ConnectHost plus
 *  the token under trial (which may not be the stored one yet). */
export interface CandidateHost {
  hostname: string
  port: number
  secure: boolean
  token: string
}

export type ValidateResult =
  | { ok: true; version: string; protocol: number }
  | { ok: false; reason: string }

/** Daemon /boot-status shape (mirrors ConnectionGate's DaemonBootStatus). */
interface BootStatus {
  version: string
  protocol: number
  phase: string
  detail: string
}

/** Build `<scheme>://<host>[:<port>]` for a candidate, matching
 *  daemon-ws.ts: secure+443 omits the port; everything else carries it. */
function candidateBase(c: Pick<CandidateHost, 'hostname' | 'port' | 'secure'>): string {
  const scheme = c.secure ? 'https' : 'http'
  const authority = c.secure && c.port === 443 ? c.hostname : `${c.hostname}:${c.port}`
  return `${scheme}://${authority}`
}

/**
 * Validate a candidate host+token by hitting its `/boot-status?token=...`.
 *
 * Success = a 2xx /boot-status response (the token was accepted by the
 * daemon's route guard) that parses to a compatible protocol. A 401/403
 * means the token was rejected → re-prompt. A network error / timeout
 * means unreachable.
 *
 * `minProtocol` defaults to 1 (the daemon's current PROTOCOL); kept as a
 * param so callers can stay in lockstep with ConnectionGate's
 * MIN_COMPATIBLE_PROTOCOL.
 */
export async function validateHost(
  c: CandidateHost,
  minProtocol = 1,
  timeoutMs = 4000,
): Promise<ValidateResult> {
  const base = candidateBase(c)
  const url = `${base}/boot-status${c.token ? `?token=${encodeURIComponent(c.token)}` : ''}`
  let resp: Response
  try {
    resp = await fetch(url, { signal: AbortSignal.timeout(timeoutMs) })
  } catch {
    return { ok: false, reason: `Couldn't reach ${c.hostname}. Check the address and your network.` }
  }
  if (resp.status === 401 || resp.status === 403) {
    return { ok: false, reason: 'The server rejected this password.' }
  }
  if (!resp.ok) {
    return { ok: false, reason: `Server returned ${resp.status}. It may not be a K2 daemon.` }
  }
  let status: BootStatus
  try {
    status = (await resp.json()) as BootStatus
  } catch {
    return { ok: false, reason: 'Server response was not a valid K2 boot status.' }
  }
  if (typeof status.protocol !== 'number' || status.protocol < minProtocol) {
    return {
      ok: false,
      reason: `Server protocol ${status.protocol ?? '?'} is too old for this app (needs ≥ ${minProtocol}).`,
    }
  }
  return { ok: true, version: status.version, protocol: status.protocol }
}

/**
 * Parse a user-entered K2 server URL (connect-users #617) into the
 * hostname / secure / port fields a ConnectHost needs. Accepts a bare
 * host (`rosson.k2.dev`) — defaulted to `https://` — or a full URL
 * (`https://rosson.k2.dev`, `http://192.168.1.5:47800`).
 *
 *   - scheme `https` (or absent) → secure:true, port defaults to 443
 *   - scheme `http`              → secure:false, port defaults to 80
 *   - an explicit `:port` always wins over the scheme default
 */
export type ParsedServerUrl =
  | { ok: true; hostname: string; secure: boolean; port: number }
  | { ok: false; reason: string }

export function parseServerUrl(raw: string): ParsedServerUrl {
  const trimmed = raw.trim()
  if (!trimmed) return { ok: false, reason: 'Enter the K2 server URL.' }
  // Default to https:// when no scheme is given so a bare `sub.k2.dev`
  // works (the common hosted case).
  const withScheme = /^[a-z][a-z0-9+.-]*:\/\//i.test(trimmed) ? trimmed : `https://${trimmed}`
  let u: URL
  try {
    u = new URL(withScheme)
  } catch {
    return { ok: false, reason: "That doesn't look like a valid URL." }
  }
  if (u.protocol !== 'https:' && u.protocol !== 'http:') {
    return { ok: false, reason: 'URL must start with https:// or http://.' }
  }
  const hostname = u.hostname
  if (!hostname) return { ok: false, reason: 'URL is missing a hostname.' }
  const secure = u.protocol === 'https:'
  const port = u.port ? Number(u.port) : secure ? 443 : 80
  if (!Number.isInteger(port) || port <= 0 || port > 65535) {
    return { ok: false, reason: 'Port must be 1–65535.' }
  }
  return { ok: true, hostname, secure, port }
}

/** Username rule for connect-users (#617): lowercase alphanumerics plus
 *  `_`/`-`, at least 2 chars — mirrors the daemon's account-name rule. */
export function isValidUsername(username: string): boolean {
  return /^[a-z0-9_-]{2,}$/.test(username.trim())
}

/** Convenience: validate using a saved ConnectHost + an explicit token
 *  (the user may be re-entering a rejected/expired one). */
export function validateConnectHost(
  host: ConnectHost,
  token: string,
  minProtocol = 1,
): Promise<ValidateResult> {
  return validateHost(
    { hostname: host.hostname, port: host.port, secure: host.secure, token },
    minProtocol,
  )
}
