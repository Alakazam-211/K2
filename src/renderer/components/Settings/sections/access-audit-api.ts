// Dedicated GET for Settings → Logs → Access Audit.
//
// Must NOT go through K2ConnectSection `userGet`: that helper treats 403 as
// a stale token (invalidateDaemonWs + retry) and would drop the WS cache
// when an Admin hits the owner-only audit route. 403/404 here are terminal.
// 401 is retried once (fresh creds); a second 401 is a real auth fail.

import { getDaemonWs, invalidateDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { cliSearchParams, withDaemonFetch } from '@/web/session-token'

export const AUDIT_TAIL_DEFAULT = 200
export const AUDIT_TAIL_CHOICES = [50, 200, 1000] as const

export type AuthAuditEvent = {
  ts: string
  event: string
  user: string
  outcome: string
  ingress: string
  ip: string
}

export type ViewerRole = 'owner' | 'admin' | 'member' | 'viewer'

export type UsersAuditResult =
  | { kind: 'ok'; events: AuthAuditEvent[]; tail: number }
  | { kind: 'not-found' }
  | { kind: 'forbidden' }
  | { kind: 'unauthorized' }
  | { kind: 'error'; message: string }

function isAbortError(err: unknown): boolean {
  if (err instanceof Error && err.name === 'AbortError') return true
  return typeof DOMException !== 'undefined' && err instanceof DOMException && err.name === 'AbortError'
}

function strField(raw: Record<string, unknown>, key: string, fallback = ''): string {
  const v = raw[key]
  return typeof v === 'string' ? v : fallback
}

export function parseAuditEvents(body: unknown): AuthAuditEvent[] {
  if (!body || typeof body !== 'object') return []
  const events = (body as { events?: unknown }).events
  if (!Array.isArray(events)) return []
  const out: AuthAuditEvent[] = []
  for (const raw of events) {
    if (!raw || typeof raw !== 'object') continue
    const r = raw as Record<string, unknown>
    if (typeof r.event !== 'string' && typeof r.ts !== 'string') continue
    out.push({
      ts: strField(r, 'ts'),
      event: strField(r, 'event'),
      user: strField(r, 'user'),
      outcome: strField(r, 'outcome'),
      ingress: strField(r, 'ingress'),
      ip: strField(r, 'ip', '-'),
    })
  }
  return out
}

/** Daemon `events[]` is oldest-first; the table displays newest first. */
export function newestFirst(events: AuthAuditEvent[]): AuthAuditEvent[] {
  return events.slice().reverse()
}

export function isSuccessLogin(ev: AuthAuditEvent): boolean {
  return ev.event === 'login' && ev.outcome === 'ok'
}

export function formatAuditTimeUtc(ts: string): string {
  const d = new Date(ts)
  if (Number.isNaN(d.getTime())) return ts || '—'
  const pad = (n: number): string => String(n).padStart(2, '0')
  return `${d.getUTCFullYear()}-${pad(d.getUTCMonth() + 1)}-${pad(d.getUTCDate())} ${pad(d.getUTCHours())}:${pad(d.getUTCMinutes())}:${pad(d.getUTCSeconds())}`
}

function auditUrl(token: string, hostBase: string, tail: number): string {
  const search = cliSearchParams(token, { tail })
  const q = search.toString()
  return `${hostBase}/cli/users/audit${q ? `?${q}` : ''}`
}

export async function fetchUsersAudit(opts: {
  tail: number
  signal?: AbortSignal
}): Promise<UsersAuditResult> {
  const send = async (): Promise<Response> => {
    const creds = await getDaemonWs()
    return fetch(
      auditUrl(creds.token, daemonHttpBase(creds), opts.tail),
      withDaemonFetch({ method: 'GET', signal: opts.signal }),
    )
  }

  let res: Response
  try {
    res = await send()
  } catch (err) {
    if (opts.signal?.aborted || isAbortError(err)) throw err
    return { kind: 'error', message: err instanceof Error ? err.message : 'Could not load login history.' }
  }

  if (res.status === 404) return { kind: 'not-found' }
  // Owner-only route. Admin 403 is expected — do NOT treat as stale token.
  if (res.status === 403) return { kind: 'forbidden' }

  if (res.status === 401) {
    invalidateDaemonWs()
    try {
      res = await send()
    } catch (err) {
      if (opts.signal?.aborted || isAbortError(err)) throw err
      return { kind: 'error', message: err instanceof Error ? err.message : 'Could not load login history.' }
    }
    if (res.status === 401) return { kind: 'unauthorized' }
    if (res.status === 404) return { kind: 'not-found' }
    if (res.status === 403) return { kind: 'forbidden' }
  }

  if (!res.ok) {
    const text = await res.text().catch(() => '')
    return { kind: 'error', message: text || `Could not load login history (${res.status}).` }
  }

  let body: unknown
  try {
    body = JSON.parse(await res.text())
  } catch {
    return { kind: 'error', message: 'Could not load login history.' }
  }

  const tail =
    body && typeof body === 'object' && typeof (body as { tail?: unknown }).tail === 'number'
      ? (body as { tail: number }).tail
      : opts.tail

  return { kind: 'ok', events: parseAuditEvents(body), tail }
}

export async function fetchWhoamiRole(opts?: { signal?: AbortSignal }): Promise<ViewerRole | null> {
  let res: Response
  try {
    const creds = await getDaemonWs()
    const search = cliSearchParams(creds.token)
    const q = search.toString()
    res = await fetch(
      `${daemonHttpBase(creds)}/cli/auth/whoami${q ? `?${q}` : ''}`,
      withDaemonFetch({ method: 'GET', signal: opts?.signal }),
    )
  } catch (err) {
    if (opts?.signal?.aborted || isAbortError(err)) throw err
    return null
  }
  if (!res.ok) return null
  let data: { role?: unknown; owner?: unknown }
  try {
    data = (await res.json()) as { role?: unknown; owner?: unknown }
  } catch {
    return null
  }
  const role = data.role
  if (role === 'owner' || role === 'admin' || role === 'member' || role === 'viewer') return role
  if (data.owner === true) return 'owner'
  return null
}
