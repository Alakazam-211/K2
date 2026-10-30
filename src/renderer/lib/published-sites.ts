// Who may open a published site (prd-publish-visibility-and-dev-mode-v1 PV-5,
// PV-12): the client's half. The daemon owns the mode, the gate, the PIN check
// and the one-time links; this file sends gestures and swaps a URL. A PIN goes
// from K2's own form straight to the route and is never kept here.

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { openOffOriginHttp } from '@/lib/open-off-origin-http'
import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { canonicalHostKey, savedHostForKey } from '@/lib/host-key'
import { useConnectHostStore } from '@/stores/connect-host'
import { webFeatures } from '@/web/features'

export type SiteMode = 'private' | 'pin' | 'public'

export interface SiteVisibility {
  label: string
  mode: SiteMode
  dev: boolean
  pinSet: boolean
  reason: string | null
  /** K2's check covers this site on this box (false: E2E off). */
  enforced: boolean
  url: string | null
  warnings: string[]
}

export interface SetVisibility {
  mode: SiteMode
  reason?: string
  pin?: string
  pinDays?: number
  /** Go ahead past the warning that a public site will be read as HTTP/1.1. */
  yes?: boolean
  devIsPublic?: boolean
}

/** What a site's chip says. */
export function siteModeLabel(mode: string | null | undefined, dev = false): string | null {
  const word = mode === 'private' ? 'Private' : mode === 'pin' ? 'PIN' : mode === 'public' ? 'Public' : null
  if (!word) return null
  return dev ? `${word} · dev` : word
}

export async function getSiteVisibility(scope: ServerScope, label: string): Promise<SiteVisibility> {
  return daemonCliGet<SiteVisibility>(scope, 'publish/visibility', { label })
}

export async function setSiteVisibility(scope: ServerScope, label: string, change: SetVisibility): Promise<SiteVisibility> {
  return daemonCliPost<SiteVisibility>(scope, 'publish/visibility', { label, ...change })
}

const NESTED_SITE = /^https:\/\/([a-z0-9-]+)\.([a-z0-9-]+\.k2\.dev)(?:[/?#]|$)/i

/** Could this be a nested site on some K2 server? Keeps ordinary browsing from
 *  asking the daemon anything. */
export function isNestedSiteUrl(url: string): boolean {
  return NESTED_SITE.test(url.trim())
}

interface OpenAnswer {
  swap?: boolean
  url?: string | null
}

async function askForLink(scope: ServerScope, url: string): Promise<string | null> {
  try {
    const r = await daemonCliPost<OpenAnswer>(scope, 'publish/open', { url })
    return r.swap && r.url ? r.url : null
  } catch {
    // A Member, an older server, a view-only room: the plain URL, and the gate
    // says what the person may do.
    return null
  }
}

/** The address a K2 tab should load for `url`: this server's (or, through it,
 *  another saved server's) private or PIN site becomes a one-time link, so the
 *  person just sees the site (PV-5, P-D9). Everything else stays as it is. */
export async function siteUrlForTab(scope: ServerScope, url: string): Promise<string> {
  const m = NESTED_SITE.exec(url.trim())
  if (!m) return url
  const own = await askForLink(scope, url)
  if (own) return own
  const parent = canonicalHostKey(m[2].toLowerCase())
  if (scope.hostKey === parent) return url
  const saved = savedHostForKey(useConnectHostStore.getState().hosts, parent)
  if (!saved) return url
  return (await askForLink(scopeForHost(parent), url)) ?? url
}

/** "Open" on a site. Desktop: the Browser tab, which does the swap as it loads.
 *  The web client has no Browser tab, so it opens a new browser tab with the
 *  link (PA-2). That tab opens first: after the await, a popup blocker would
 *  refuse it. */
export async function openSite(scope: ServerScope, url: string): Promise<void> {
  if (webFeatures.browserPane) {
    openOffOriginHttp(url)
    return
  }
  const tab = window.open('about:blank', '_blank')
  const dest = await siteUrlForTab(scope, url)
  if (tab) {
    tab.opener = null
    tab.location.href = dest
    return
  }
  openOffOriginHttp(dest)
}
