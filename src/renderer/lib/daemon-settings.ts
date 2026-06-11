// Phase 2 Unit 7a — daemon-direct App Settings client.
//
// Before this unit, `invoke('settings_{get,update,reset}')` walked
// through Tauri's `commands/settings.rs` which loaded + merged +
// wrote `~/.k2so/settings.json` in-process. Co-existing with the
// daemon's writers caused the F3 race: rotating companion creds in
// Tauri left live daemon-side sessions valid until TTL expired.
//
// Post-Unit-7a the daemon (`/cli/settings/{get,update,reset}`) is the
// sole writer. These helpers go straight at it from the renderer so:
//
// 1. The Tauri proxy shim (still present for back-compat) can be
//    dropped in Unit 7c without renderer churn.
// 2. The same code path works in K2 Connect mode where Tauri is
//    on Machine A and the daemon is on Machine B — there is no
//    `invoke('settings_*')` to call there.
//
// Shape matches `AppSettingsResponse` from `@shared/types`. The
// daemon returns the canonical post-merge state on every write, so
// callers can update local store state from the response without a
// follow-up read.

import { getDaemonWs, invalidateDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import type { AppSettingsResponse } from '@shared/types'

async function daemonUrl(path: string): Promise<string> {
  const creds = await getDaemonWs()
  return `${daemonHttpBase(creds)}${path}?token=${creds.token}`
}

/** GET `/cli/settings/get` — full settings snapshot.
 *
 *  Phase 2.5 fix (finding #547): retries once on a connection-level
 *  failure after invalidating the cached daemon creds. The renderer
 *  caches the daemon's port + token for the lifetime of the app
 *  process; if the daemon restarts and (pre-Sub-fix-A) rotates its
 *  port, the next call would otherwise route to a dead socket and
 *  the calling store would fall back to defaults — which the user's
 *  next interaction then persists, overwriting real settings.
 *  See `daemon-cli.ts` for the same pattern.
 */
export async function settingsGet(): Promise<AppSettingsResponse> {
  return withConnRetry(async () => {
    const url = await daemonUrl('/cli/settings/get')
    const res = await fetch(url, { method: 'GET' })
    if (!res.ok) {
      const body = await res.text().catch(() => '')
      throw new Error(`settings_get ${res.status}: ${body}`)
    }
    return (await res.json()) as AppSettingsResponse
  })
}

/** POST `/cli/settings/update` — deep-merges `updates` into settings.
 *  Returns the full post-merge `AppSettings`. The daemon-side handler
 *  invalidates active companion sessions when `companion.username` or
 *  `companion.passwordHash` change (F3 closure). */
export async function settingsUpdate(
  updates: Record<string, unknown>,
): Promise<AppSettingsResponse> {
  return withConnRetry(async () => {
    const url = await daemonUrl('/cli/settings/update')
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify(updates),
    })
    if (!res.ok) {
      const body = await res.text().catch(() => '')
      throw new Error(`settings_update ${res.status}: ${body}`)
    }
    return (await res.json()) as AppSettingsResponse
  })
}

/** POST `/cli/settings/reset` — restores defaults, deletes the Keychain
 *  companion password, invalidates every live companion session. */
export async function settingsReset(): Promise<AppSettingsResponse> {
  return withConnRetry(async () => {
    const url = await daemonUrl('/cli/settings/reset')
    const res = await fetch(url, {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: '{}',
    })
    if (!res.ok) {
      const body = await res.text().catch(() => '')
      throw new Error(`settings_reset ${res.status}: ${body}`)
    }
    return (await res.json()) as AppSettingsResponse
  })
}

/** See `daemon-cli.ts::withConnRetry` for the rationale. Kept as a
 *  local copy rather than imported because both modules need to be
 *  safe to load before each other. */
async function withConnRetry<T>(op: () => Promise<T>): Promise<T> {
  try {
    return await op()
  } catch (err) {
    if (!isConnectionLevelError(err)) throw err
    invalidateDaemonWs()
    return await op()
  }
}

function isConnectionLevelError(err: unknown): boolean {
  if (!(err instanceof Error)) return false
  const m = err.message.toLowerCase()
  return (
    m.includes('failed to fetch') ||
    m.includes('load failed') ||
    m.includes('networkerror') ||
    m.includes('connection refused') ||
    m.includes('econnrefused') ||
    m.includes('daemon_ws_url invoke failed') ||
    m.includes('daemon not reachable')
  )
}
