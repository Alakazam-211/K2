/**
 * Window title.
 *
 * Desktop (Mission Control / Exposé): `K2 | <server nickname>`, or
 * `K2 | local` for the local daemon.
 * Hosted web: `<subdomain> | K2` e.g. `z3thon | K2`.
 */

import { isWebClient } from '@/lib/is-web'

/**
 * First DNS label of the host (customer sub on *.app.k2.dev / *.k2.dev).
 * Returns null when there is no useful label.
 */
export function webSubdomainLabel(
  hostname: string | undefined | null = typeof window !== 'undefined'
    ? window.location.hostname
    : '',
): string | null {
  if (!hostname) return null
  const h = String(hostname).trim().toLowerCase()
  if (!h) return null
  // Strip brackets from IPv6 literals; never treat pure IPs as a sub brand.
  if (h.startsWith('[')) return null
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(h)) return null
  const label = h.split('.')[0]?.trim()
  if (!label) return null
  return label
}

/** One line for the window title. Drops line breaks a nickname might contain. */
export function missionControlServerName(label: string | null | undefined): string {
  const name = (label ?? '').replace(/[\r\n]+/g, ' ').trim()
  return name || 'local'
}

/** Base title: `K2 | local` on desktop, `z3thon | K2` on web. */
export function k2BasePageTitle(
  hostname?: string | null,
  serverLabel?: string | null,
): string {
  if (!isWebClient()) return `K2 | ${missionControlServerName(serverLabel)}`
  const sub = webSubdomainLabel(hostname)
  if (!sub || sub === 'localhost') {
    // Local web-serve still gets a clear brand; no fake sub.
    if (sub === 'localhost') return 'localhost | K2'
    return 'K2'
  }
  return `${sub} | K2`
}

/**
 * Full tab title including optional zoom suffix (desktop zoom UI).
 * Zoom ≠ 1 → `K2 | local — 125%` / `z3thon | K2 — 125%`.
 */
export function k2PageTitle(
  zoom = 1,
  hostname?: string | null,
  serverLabel?: string | null,
): string {
  const base = k2BasePageTitle(hostname, serverLabel)
  if (!zoom || zoom === 1) return base
  return `${base} — ${Math.round(zoom * 100)}%`
}

/** Apply the hosted-web tab title (no-op on desktop). */
export function applyWebPageTitle(): void {
  if (!isWebClient()) return
  if (typeof document === 'undefined') return
  document.title = k2BasePageTitle()
}
