// Settings → Apps → <App> → Releases (prd-tauri-core-0467-v1, Rosson 10-29,
// 0.46.7): typed bindings for `/cli/app/updates*`. Thin client: the daemon
// owns the releases, the channels, the uploads to the download host, the
// check-in counts and every rule (forward-only promote, the platform guard,
// the rollback warning); this file parses its answers and sends gestures.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { primaryScope } from '@/kessel/server-scope'

export type ReleaseFile = { platform: string; file: string; size: number; url: string | null; upload: string }
export type Release = {
  id: string
  version: string
  state: string
  error: string | null
  channels: string[]
  size: number
  activeUsers: number
  files: ReleaseFile[]
}
export type Channel = { channel: string; version: string | null }
export type Updates = {
  app: string
  dlId: string | null
  keyId: string | null
  channels: Channel[]
  releases: Release[]
  activeToday: number
  activeWeek: number
  counting: string
  endpoint: string | null
}
export type Downloads = { available: true; total: number; byVersion: Record<string, number> } | { available: false; why: string }

function rec(v: unknown): Record<string, unknown> {
  return v && typeof v === 'object' && !Array.isArray(v) ? (v as Record<string, unknown>) : {}
}
function str(v: unknown): string {
  return typeof v === 'string' ? v : ''
}
function num(v: unknown): number {
  return typeof v === 'number' && Number.isFinite(v) ? v : 0
}
function arr(v: unknown): unknown[] {
  return Array.isArray(v) ? v : []
}

export function parseUpdates(raw: unknown): Updates {
  const r = rec(raw)
  const key = r.key == null ? null : rec(r.key)
  const users = rec(r.activeUsers)
  const ep = r.endpoints == null ? null : rec(r.endpoints)
  return {
    app: str(r.app),
    dlId: str(r.dlId) || null,
    keyId: key ? str(key.keyId) || null : null,
    channels: arr(r.channels).map((c) => {
      const x = rec(c)
      return { channel: str(x.channel), version: str(x.version) || null }
    }),
    releases: arr(r.releases).map((x0) => {
      const x = rec(x0)
      return {
        id: str(x.id),
        version: str(x.version),
        state: str(x.state),
        error: str(x.error) || null,
        channels: arr(x.channels).filter((c): c is string => typeof c === 'string'),
        size: num(x.size),
        activeUsers: num(x.activeUsers),
        files: arr(x.files).map((f0) => {
          const f = rec(f0)
          return { platform: str(f.platform), file: str(f.file), size: num(f.size), url: str(f.url) || null, upload: str(f.upload) }
        }),
      }
    }),
    activeToday: num(users.today),
    activeWeek: num(users.last7Days),
    counting: str(users.counting) || 'checks',
    endpoint: ep ? str(ep.dynamic) || null : null,
  }
}

export function parseDownloads(raw: unknown): Downloads {
  const d = rec(rec(raw).downloads)
  if (d.available !== true) return { available: false, why: str(d.why) || 'unavailable' }
  const byVersion: Record<string, number> = {}
  for (const x0 of arr(d.byVersion)) {
    const x = rec(x0)
    byVersion[str(x.version)] = num(x.downloads)
  }
  return { available: true, total: num(d.total), byVersion }
}

/** `12.3 MiB`, `850 KiB`. */
export function size(n: number): string {
  if (n >= 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(1)} MiB`
  if (n >= 1024) return `${Math.round(n / 1024)} KiB`
  return `${n} B`
}

export async function fetchUpdates(app: string): Promise<Updates> {
  return parseUpdates(await daemonCliGet<unknown>(primaryScope(), 'app/updates', { app }))
}

export async function fetchDownloads(app: string): Promise<Downloads> {
  return parseDownloads(await daemonCliGet<unknown>(primaryScope(), 'app/updates/stats', { app }))
}

/** Point `to` at `version` (`from` only names where it is now, if anywhere). */
export async function promote(app: string, from: string | null, to: string, version: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/updates/promote', from ? { app, from, to, version } : { app, to, version })
}

/** Returns the daemon's warning (clients never downgrade). */
export async function rollback(app: string, channel: string, to: string): Promise<string> {
  return str(rec(await daemonCliPost<unknown>(primaryScope(), 'app/updates/rollback', { app, channel, to })).warning)
}

export async function pull(app: string, version: string): Promise<void> {
  await daemonCliPost<unknown>(primaryScope(), 'app/updates/pull', { app, version })
}
