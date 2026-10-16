// Shared bits for Settings → Gardens → External Data (contract-sources-v1
// §8). Owner: B2. Pure helpers (no React) so they're easy to test.
//
// Times are SECONDS. Ages come from the server's own clock
// (`serverNow - updatedAt`), never this computer's.

import type { Counts, Display, JsonValue, SourceResult, TriggerOn } from '@/lib/sources/sources-types'
import { primaryScope, scopeForHost, type ServerScope } from '@/kessel/server-scope'

export const btn =
  'px-2 py-1 text-[10px] border border-[var(--color-border)] text-[var(--color-text-secondary)] hover:border-[var(--color-text-muted)] cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed'
export const btnPrimary =
  'px-3 py-1.5 text-[11px] font-medium bg-[var(--color-accent)]/15 text-[var(--color-text-primary)] hover:bg-[var(--color-accent)]/25 cursor-pointer disabled:opacity-40 disabled:cursor-not-allowed'
export const input =
  'w-full px-2 py-1 text-[12px] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-[var(--color-text-primary)]'
export const muted = 'text-[var(--color-text-muted)]'
export const danger = 'text-[var(--color-danger,#e5484d)]'

/** The error envelope inside a thrown daemon error:
 *  `{"error": {"code", "hint", ...extra}}`. */
export interface SourcesError {
  code: string
  hint: string
  [extra: string]: unknown
}

export function sourcesErrorOf(e: unknown): SourcesError | null {
  const msg = e instanceof Error ? e.message : typeof e === 'string' ? e : ''
  const start = msg.indexOf('{')
  if (start < 0) return null
  try {
    const parsed = JSON.parse(msg.slice(start)) as { error?: unknown }
    const err = parsed?.error
    if (!err || typeof err !== 'object') return null
    const o = err as Record<string, unknown>
    if (typeof o.code !== 'string') return null
    return { ...o, code: o.code, hint: typeof o.hint === 'string' ? o.hint : '' }
  } catch {
    return null
  }
}

/** What a person reads for a failed call. */
export function sourcesErrorText(e: unknown): string {
  const info = sourcesErrorOf(e)
  if (info) {
    if (info.code === 'not_built') return 'This part of External Data is not on this server yet.'
    if (info.code === 'person_only') return 'This needs a person (Owner or Admin) on this server.'
    if (info.code === 'cap_not_granted') return 'Your login on this server can’t see that source (admins only).'
    if (info.code === 'role_required') return 'Your login on this server can’t do that.'
    // vault_confirm_required (0.46.0) or confirm_required (a 0.45.x server).
    if (info.code === 'vault_confirm_required' || info.code === 'confirm_required')
      return 'This needs your confirm (Touch ID, your login or the vault password). Try again and confirm when asked.'
    if (info.code === 'no_confirm_method')
      return 'No way to confirm here. Bind from Settings over your Connect login, or set a vault password.'
    return info.hint || info.code
  }
  return e instanceof Error ? e.message : String(e)
}

/** `40 s ago`, `12 min ago`, `3 h ago`, `2 d ago`. */
export function ago(secs: number): string {
  const s = Math.max(0, Math.round(secs))
  if (s < 60) return `${s} s ago`
  if (s < 3600) return `${Math.floor(s / 60)} min ago`
  if (s < 86_400) return `${Math.floor(s / 3600)} h ago`
  return `${Math.floor(s / 86_400)} d ago`
}

/** `30 s`, `5 min`, `2 h`, `1 d`. */
export function every(secs: number): string {
  if (secs % 86_400 === 0) return `${secs / 86_400} d`
  if (secs % 3600 === 0) return `${secs / 3600} h`
  if (secs % 60 === 0) return `${secs / 60} min`
  return `${secs} s`
}

/** Age of the last good check, by the server's clock; null before one. */
export function ageOf(view: Pick<SourceResult, 'serverNow' | 'updatedAt'>): string | null {
  return view.updatedAt == null ? null : ago(view.serverNow - view.updatedAt)
}

/** HH:MM UTC. */
export function hhmm(t: number): string {
  const d = new Date(t * 1000)
  return `${String(d.getUTCHours()).padStart(2, '0')}:${String(d.getUTCMinutes()).padStart(2, '0')} UTC`
}

/** The state dot's colour. Unknown has its own grey-blue. */
export function dotColor(display: Display | 'proposed'): string {
  switch (display) {
    case 'ok':
      return 'var(--color-success,#30a46c)'
    case 'warn':
      return 'var(--color-status-warn)'
    case 'crit':
      return 'var(--color-danger,#e5484d)'
    case 'unknown':
      return '#7d8fb3'
    default:
      return 'var(--color-text-muted)'
  }
}

/** `18 ok, 2 unknown` (zero counts left out; unknown is never "down"). */
export function countsLine(c: Counts): string {
  const parts: string[] = []
  if (c.crit) parts.push(`${c.crit} red`)
  if (c.warn) parts.push(`${c.warn} amber`)
  if (c.ok) parts.push(`${c.ok} ok`)
  if (c.unknown) parts.push(`${c.unknown} unknown`)
  return parts.join(', ')
}

/** A URL with query values masked (`?teamId=•••`). */
export function maskQuery(url: string): string {
  const q = url.indexOf('?')
  if (q < 0) return url
  const hash = url.indexOf('#', q)
  const query = url.slice(q + 1, hash < 0 ? undefined : hash)
  const masked = query
    .split('&')
    .filter((p) => p !== '')
    .map((p) => `${p.split('=')[0]}=•••`)
    .join('&')
  return `${url.slice(0, q)}?${masked}`
}

/** `went_crit` → "goes red", `stale_for` → "has no good check for 30 min". */
export function onLabel(on: TriggerOn): string {
  switch (on.kind) {
    case 'went_crit':
      return 'goes red'
    case 'went_warn':
      return 'goes amber'
    case 'recovered':
      return 'recovers'
    case 'item_went_crit':
      return 'an item goes red'
    case 'stale_for':
      return `has no good check for ${every(on.secs)}`
  }
}

/** Who made it, for a person: `owner`, `ada`, or an agent's handle. */
export function makerLabel(madeBy: string, handle?: string | null): string {
  if (madeBy === 'owner') return 'the owner'
  if (madeBy.startsWith('login:')) return madeBy.slice(6)
  if (madeBy.startsWith('agent:')) return handle || 'an agent'
  return madeBy
}

/** The scope for a server by host key: the window's own scope when it is
 *  the window's server, else a pinned one. */
export function scopeForHostKey(hostKey: string): ServerScope {
  const primary = primaryScope()
  if (primary.hostKey === hostKey) return primary
  return scopeForHost(hostKey)
}

/** Short, safe text for a JSON value in a tree sample. */
export function sampleText(v: JsonValue | undefined): string {
  if (v === undefined) return ''
  const s = typeof v === 'string' ? JSON.stringify(v) : JSON.stringify(v)
  return s.length > 80 ? `${s.slice(0, 79)}…` : s
}

/** `sha256_hex(join("\n", sort(["<sourceId>:<secretName>"])))`: the
 *  `secretId` a `source-batch` confirm is bound to (contract §4.3). */
export async function batchHash(pairs: Array<{ id: string; secret: string }>): Promise<string> {
  const lines = pairs.map((p) => `${p.id}:${p.secret}`).sort()
  const data = new TextEncoder().encode(lines.join('\n'))
  const digest = await crypto.subtle.digest('SHA-256', data)
  return Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, '0'))
    .join('')
}
