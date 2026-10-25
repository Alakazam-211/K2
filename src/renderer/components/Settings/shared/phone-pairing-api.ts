// Phone pairing (phone-web-off, 2026-10-24): Settings → K2 Companion (the
// owner's view) and the profile modal (your own phones). Thin over the
// daemon's
//   GET  /cli/auth/devices[?mine=1] → {devices[], pairAs}
//   POST /cli/auth/device/pair-code  → {code, user, expiresAt, pairUrl, …}
//   POST /cli/auth/device/revoke     → {revoked, username}
// The daemon decides who a code pairs as, which logins the owner may pick,
// whether a password is needed, and every refusal; this only carries it.

import { getDaemonWs, invalidateDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { cliSearchParams, withDaemonFetch } from '@/web/session-token'
import { primaryScope, type ServerScope } from '@/kessel/server-scope'

export interface PairedPhone {
  id: string
  username: string
  deviceId: string
  name: string
  createdAt: string
  expiresAt: string
  maxExpiresAt: string
  pairedBy: string | null
  current: boolean
}

export interface DaemonError {
  code: string
  hint: string
}

/** Who a new code pairs as (the daemon's answer), or why there is none. */
export interface PairAs {
  user: string | null
  source: string | null
  needsPassword: boolean
  /** Logins the caller may pick instead (the owner token only). */
  choices: string[] | null
  error: DaemonError | null
}

export interface DevicesView {
  devices: PairedPhone[]
  pairAs: PairAs
}

export interface PairCode {
  code: string
  user: string
  expiresAt: string
  expiresInSecs: number
  host: string | null
  pairUrl: string | null
}

export type Result<T> = { ok: true; value: T } | { ok: false; error: DaemonError }

function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}

export function parseError(body: unknown, status: number): DaemonError {
  if (body && typeof body === 'object') {
    const e = (body as { error?: unknown }).error
    if (e && typeof e === 'object') {
      const r = e as Record<string, unknown>
      return { code: str(r.code) || String(status), hint: str(r.hint) || str(r.message) || `HTTP ${status}` }
    }
    if (typeof e === 'string') return { code: String(status), hint: e }
  }
  return { code: String(status), hint: `HTTP ${status}` }
}

export function parseDevices(body: unknown): DevicesView {
  const b = (body && typeof body === 'object' ? body : {}) as Record<string, unknown>
  const raw = Array.isArray(b.devices) ? b.devices : []
  const devices: PairedPhone[] = []
  for (const d of raw) {
    if (!d || typeof d !== 'object') continue
    const r = d as Record<string, unknown>
    devices.push({
      id: str(r.id),
      username: str(r.username),
      deviceId: str(r.deviceId),
      name: str(r.name),
      createdAt: str(r.createdAt),
      expiresAt: str(r.expiresAt),
      maxExpiresAt: str(r.maxExpiresAt),
      pairedBy: typeof r.pairedBy === 'string' ? r.pairedBy : null,
      current: r.current === true,
    })
  }
  const pa = (b.pairAs && typeof b.pairAs === 'object' ? b.pairAs : {}) as Record<string, unknown>
  const err = pa.error && typeof pa.error === 'object' ? (pa.error as Record<string, unknown>) : null
  const choices = Array.isArray(pa.choices) ? pa.choices.filter((c): c is string => typeof c === 'string') : null
  return {
    devices,
    pairAs: {
      user: typeof pa.user === 'string' ? pa.user : null,
      source: typeof pa.source === 'string' ? pa.source : null,
      needsPassword: pa.needsPassword === true,
      choices,
      error: err ? { code: str(err.code), hint: str(err.hint) } : null,
    },
  }
}

export function parsePairCode(body: unknown): PairCode {
  const b = (body && typeof body === 'object' ? body : {}) as Record<string, unknown>
  return {
    code: str(b.code),
    user: str(b.user),
    expiresAt: str(b.expiresAt),
    expiresInSecs: typeof b.expiresInSecs === 'number' ? b.expiresInSecs : 600,
    host: typeof b.host === 'string' ? b.host : null,
    pairUrl: typeof b.pairUrl === 'string' ? b.pairUrl : null,
  }
}

async function call(
  scope: ServerScope,
  path: string,
  params: Record<string, string>,
  init: RequestInit,
): Promise<{ status: number; body: unknown }> {
  try {
    return await callOnce(scope, path, params, init)
  } catch (err) {
    // Unreachable daemon / no credentials yet: an answer, never a throw.
    const hint = err instanceof Error ? err.message : 'the server did not answer'
    return { status: 0, body: { error: { code: 'unreachable', hint: `Couldn’t reach the server: ${hint}` } } }
  }
}

async function callOnce(
  scope: ServerScope,
  path: string,
  params: Record<string, string>,
  init: RequestInit,
): Promise<{ status: number; body: unknown }> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs(scope)
    const q = cliSearchParams(creds.token, params).toString()
    return fetch(`${daemonHttpBase(creds)}${path}${q ? `?${q}` : ''}`, withDaemonFetch(init))
  }
  let res = await send()
  // A rotated daemon token answers 401 login_required once: re-read creds.
  if (res.status === 401 && str(parseError(await res.clone().json().catch(() => null), 401).code) === 'login_required') {
    invalidateDaemonWs()
    res = await send()
  }
  const text = await res.text()
  let body: unknown = null
  try {
    body = text ? JSON.parse(text) : null
  } catch {
    body = { error: { code: String(res.status), hint: text.slice(0, 200) } }
  }
  return { status: res.status, body }
}

/** `mine`: only the phones of the login a new code pairs as (profile modal). */
export async function fetchPairedPhones(opts: { mine: boolean; scope?: ServerScope }): Promise<Result<DevicesView>> {
  const { status, body } = await call(
    opts.scope ?? primaryScope(),
    '/cli/auth/devices',
    opts.mine ? { mine: '1' } : {},
    { method: 'GET' },
  )
  if (status !== 200) return { ok: false, error: parseError(body, status) }
  return { ok: true, value: parseDevices(body) }
}

export async function makePairCode(opts: {
  password?: string
  user?: string
  scope?: ServerScope
}): Promise<Result<PairCode>> {
  const payload: Record<string, string> = {}
  if (opts.password) payload.password = opts.password
  if (opts.user) payload.user = opts.user
  const { status, body } = await call(opts.scope ?? primaryScope(), '/cli/auth/device/pair-code', {}, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify(payload),
  })
  if (status !== 200) return { ok: false, error: parseError(body, status) }
  return { ok: true, value: parsePairCode(body) }
}

export async function revokePairedPhone(id: string, scope?: ServerScope): Promise<Result<string>> {
  const { status, body } = await call(scope ?? primaryScope(), '/cli/auth/device/revoke', {}, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json' },
    body: JSON.stringify({ id }),
  })
  if (status !== 200) return { ok: false, error: parseError(body, status) }
  return { ok: true, value: id }
}

/** Seconds left until `expiresAt` at `nowMs` (never negative). */
export function secondsLeft(expiresAt: string, nowMs: number): number {
  const t = Date.parse(expiresAt)
  if (Number.isNaN(t)) return 0
  return Math.max(0, Math.floor((t - nowMs) / 1000))
}
