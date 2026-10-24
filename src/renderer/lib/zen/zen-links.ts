// `k2.open(url)` (catalog `links.open`, cap `links:open`; 0.46.4, Baden's
// Board via Appa 2026-10-23): a custom widget opens a link in the person's
// browser.
//
// - The custom layer (`zen-custom-bridge`) checks the cap, the rate (once a
//   second, 10 a minute), the person's press in the frame within 5 s (a
//   press, not focus: like `layout.*`), and the link itself
//   (`zenLinkTarget`: http or https only, no user name or password, at most
//   2,048 characters).
// - Then this file: the first time a site (the link's origin) is opened
//   from a Garden, K2 asks the person on its own card, outside the sealed
//   frame (`ZenLinkSheet`): Don't open, Open once, or Always open this site
//   from this Garden. "Always" is kept by this computer's daemon
//   (`GET/POST /cli/zen/links*`, `~/.k2/zen/links.json`): agents can read
//   that list, only the person's app can add to it.
// - Opening goes through `@tauri-apps/plugin-opener`'s `openUrl`: the system
//   browser on the desktop, a new tab on the web client (its shim).

import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { zenLocalScope } from './zen-api'

/** The longest link `k2.open` takes (= the daemon's `links::MAX_URL_CHARS`). */
export const ZEN_LINK_MAX_CHARS = 2048

/** `links.open`'s budget per widget (refusals count toward UW29). */
export const ZEN_LINKS_OPEN = { perSecond: 1, perMinute: 10 } as const

/** A link K2 will open: the URL as given (trimmed) and its site (origin). */
export interface ZenLinkTarget {
  url: string
  site: string
}

/** The link `k2.open` was given, or why not (a sentence for `bad_args`). */
export function zenLinkTarget(raw: unknown): ZenLinkTarget | string {
  if (typeof raw !== 'string') return 'links.open takes a link (a string)'
  const url = raw.trim()
  if (!url || [...url].length > ZEN_LINK_MAX_CHARS) return `a link is 1 to ${ZEN_LINK_MAX_CHARS} characters`
  let u: URL
  try {
    u = new URL(url)
  } catch {
    return 'that isn’t a full http or https link'
  }
  if (u.protocol !== 'http:' && u.protocol !== 'https:') return 'K2 opens only http and https links'
  if (u.username || u.password) return 'K2 never opens a link with a user name or password in it'
  if (!u.hostname) return 'the link has no host'
  // `URL.origin` lower-cases the host and drops a default port, as the
  // daemon's `links::site_of` does.
  return { url, site: u.origin }
}

/** The person's answer on K2's card. */
export type ZenLinkAnswer = 'deny' | 'once' | 'always'

/** What K2's card shows (all of it K2's words; the widget only gave the link). */
export interface ZenLinkAsk {
  url: string
  site: string
  /** The widget's display name. */
  widget: string
  /** The Garden's name, for "Always open … from <Garden>". */
  garden: string
}

/** What `createZenLinks` needs (tests pass fakes). */
export interface ZenLinksDeps {
  gardenId(): string
  /** The sites this Garden opens without asking (the daemon's list). */
  sites(gardenId: string): Promise<readonly string[]>
  /** "Always open": the daemon keeps it. */
  allow(gardenId: string, site: string): Promise<void>
  /** K2's card; resolves with the person's answer. */
  ask(ask: ZenLinkAsk): Promise<ZenLinkAnswer>
  describe(): { widget: string; garden: string }
  openUrl(url: string): Promise<void>
}

/** `open(url, site)` for the custom layer: true when K2 opened it. */
export function createZenLinks(deps: ZenLinksDeps): { open(url: string, site: string): Promise<boolean> } {
  return {
    async open(url, site) {
      const garden = deps.gardenId()
      const known = await deps.sites(garden)
      if (!known.includes(site)) {
        const answer = await deps.ask({ url, site, ...deps.describe() })
        if (answer === 'deny') return false
        if (answer === 'always') {
          try {
            await deps.allow(garden, site)
          } catch (err) {
            // The person said yes: open it; they'll be asked again next time.
            console.warn('[zen] k2.open: keeping the site for this Garden failed:', err)
          }
        }
      }
      await deps.openUrl(url)
      return true
    },
  }
}

/** `GET /cli/zen/links?garden=` on this computer's daemon. */
export async function fetchZenLinkSites(gardenId: string): Promise<readonly string[]> {
  const r = await daemonCliGet<{ sites?: unknown }>(zenLocalScope(), 'zen/links', { garden: gardenId })
  if (!Array.isArray(r.sites) || !r.sites.every((s) => typeof s === 'string')) {
    throw new Error('zen/links answered without a sites list')
  }
  return r.sites as string[]
}

/** `POST /cli/zen/links/allow` (the owner token: only the person's app). */
export async function allowZenLinkSite(gardenId: string, site: string): Promise<void> {
  await daemonCliPost(zenLocalScope(), 'zen/links/allow', { garden: gardenId, site })
}
