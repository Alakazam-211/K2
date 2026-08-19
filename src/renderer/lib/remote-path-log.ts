// Breadcrumbs for remote-path first-crack diagnosis.
// Filter DevTools on `[remote-path]`. Never log tokens or full query strings.

import { useConnectHostStore } from '@/stores/connect-host'

export function isRemoteActiveHost(): boolean {
  return useConnectHostStore.getState().activeHost !== 'local'
}

/** Strip `token=` / other query so console pastes stay safe. */
export function redactRemoteUrl(url: string): string {
  try {
    const u = new URL(url)
    const path = u.pathname
    return `${u.host}${path}`
  } catch {
    return url.split('?')[0] ?? url
  }
}

export function classifyRemoteFetchError(err: unknown): string {
  if (err instanceof DOMException && err.name === 'TimeoutError') return 'timeout'
  if (err instanceof Error) {
    const m = err.message.toLowerCase()
    if (err.name === 'TimeoutError' || err.name === 'AbortError') return 'timeout'
    if (m.includes('access-control') || m.includes('not allowed by')) return 'cors'
    if (m.includes('load failed') || m.includes('failed to fetch')) return 'load-failed'
    if (m.includes('networkerror')) return 'network'
    return err.name || 'error'
  }
  return 'unknown'
}

let lastRemoteBootOkAt: number | null = null

export function noteRemoteBootOk(): void {
  lastRemoteBootOkAt = Date.now()
}

export function msSinceRemoteBootOk(): number | null {
  if (lastRemoteBootOkAt == null) return null
  return Date.now() - lastRemoteBootOkAt
}

export function logRemotePath(event: string, fields?: Record<string, unknown>): void {
  if (!isRemoteActiveHost()) return
  // Always-on: this is how we catch the first crack in a field session.
  // eslint-disable-next-line no-console
  console.warn('[remote-path]', event, fields ?? {})
}
