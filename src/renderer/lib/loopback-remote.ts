// C8/C17: loopback in the Browser pane is this Mac. A Connect host is not.

import type { ActiveHost } from '@/stores/connect-host'

const LOOPBACK_HOSTS = new Set(['localhost', '127.0.0.1', '::1', '[::1]'])

/** True when `url` is http(s) to localhost / 127.0.0.1 / ::1. */
export function isLoopbackHttpUrl(url: string): boolean {
  try {
    const parsed = new URL(url)
    if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return false
    return LOOPBACK_HOSTS.has(parsed.hostname.toLowerCase())
  } catch {
    return false
  }
}

/**
 * Remote Connect host + loopback URL → refuse before `browser_create` /
 * `browser_navigate`. `activeHost === 'local'` always allows loopback.
 * Do not put this on the child's Rust `on_navigation` (Gmail OAuth).
 */
export function loopbackForbiddenOnRemote(activeHost: ActiveHost, url: string): boolean {
  if (activeHost === 'local') return false
  return isLoopbackHttpUrl(url)
}

export const LOOPBACK_ON_REMOTE_ERROR =
  'This address is on this Mac, not the connected server. Remote localhost forwarding is not available.'
