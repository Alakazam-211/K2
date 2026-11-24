/**
 * Hosted web: keep the open bundle on the server's own web client version.
 *
 * The edge serves each web client at `/app/<ver>/index.html`. Only the
 * launcher at `/` picks the version (`/boot-status` webClientVersion, or
 * version; `/app/current.json` for a ghost server), so a saved
 * `/app/<old>/index.html` link kept loading the old bundle after the server
 * upgraded, and an old client can hang on a newer server.
 *
 * ConnectionGate hands every accepted `/boot-status` body to
 * {@link noteServerBundleVersion} (no second fetch loop). Per page load:
 *   - the FIRST body that names a version is the boot check: a different,
 *     newer version whose bundle is published (HEAD, same-origin, no-store)
 *     → `location.replace` onto it, keeping the route tail, query and hash;
 *   - a LATER change (the daemon was upgraded while the tab was open)
 *     shows a small non-blocking bar ("K2 was updated · Reload"), never a
 *     yank mid-typing.
 *
 * The K2 edge (k2-connect, 0.46.18) serves `/app/<V>/index.html` only to a
 * page the launcher started (its `k2launch` cookie, or a Referer of `/`);
 * anything else gets a 302 to `/` with the query kept. So a HEAD from an old
 * page that ends on `/` means "published, go through the launcher": the move
 * is then to `/?v=<V>` with this page's query and hash, and the launcher
 * carries the hash on. A local or self-served setup (no such edge) answers
 * the HEAD directly and the move goes straight to the bundle.
 *
 * No loops: one move per target per page load, a sessionStorage guard
 * across loads (unreadable storage = already tried), never onto a version
 * older than this bundle (except the ghost sign-in shell, below), and a
 * failed HEAD stays and says why. Desktop (Tauri) never runs any of it.
 *
 * Ghost sign-in shell (PRD boot-status-ghost §3.1): when the first body this
 * page saw hid the version (`public` view), the loader started the newest
 * published bundle, which may be NEWER than the server. Once signed in, the
 * boot check moves onto the server's own version even if that is older:
 * that is the bundle the server was released with.
 */

import { create } from 'zustand'
import { isWebClient } from '@/lib/is-web'

const VERSION_RE = /^\d+\.\d+\.\d+[a-zA-Z0-9.+-]*$/
const HEAD_TIMEOUT_MS = 5000
/** sessionStorage: `{ "<target version>": <ms when we moved> }`. */
export const BUNDLE_MOVE_GUARD_KEY = 'k2.webBundleMove'
/** A move to the same target inside this window counts as already tried. */
export const BUNDLE_MOVE_GUARD_TTL_MS = 10 * 60_000

function validVersion(v: unknown): string | null {
  if (typeof v !== 'string') return null
  const s = v.trim()
  if (!s || s.includes('..') || s.includes('/') || s.includes('\\') || s.includes('%')) return null
  return VERSION_RE.test(s) ? s : null
}

/** The server's web client version, as the loader reads it:
 *  `webClientVersion` when present, else `version`; null when neither is
 *  usable (a ghost `public` view, or a malformed string). */
export function serverWebClientVersion(body: unknown): string | null {
  if (!body || typeof body !== 'object') return null
  const b = body as { webClientVersion?: unknown; version?: unknown }
  return validVersion(b.webClientVersion) ?? validVersion(b.version)
}

/** `/app/<ver>/<tail>` → its parts; null for any other path (the dev
 *  server, the launcher at `/`). */
export function bundlePathParts(pathname: string): { version: string; tail: string } | null {
  const m = /^\/app\/([^/]+)\/(.*)$/.exec(pathname)
  if (!m) return null
  let seg: string
  try {
    seg = decodeURIComponent(m[1])
  } catch {
    return null
  }
  const version = validVersion(seg)
  return version ? { version, tail: m[2] } : null
}

/** Semver-ish order: numeric major.minor.patch, then a release ranks above
 *  its prerelease, then the suffix as text. Both must be valid versions. */
export function compareVersions(a: string, b: string): number {
  const pa = /^(\d+)\.(\d+)\.(\d+)(.*)$/.exec(a)
  const pb = /^(\d+)\.(\d+)\.(\d+)(.*)$/.exec(b)
  if (!pa || !pb) return 0
  for (let i = 1; i <= 3; i++) {
    const d = Number(pa[i]) - Number(pb[i])
    if (d !== 0) return d < 0 ? -1 : 1
  }
  const sa = pa[4]
  const sb = pb[4]
  if (sa === sb) return 0
  if (!sa) return 1
  if (!sb) return -1
  return sa < sb ? -1 : 1
}

/** The bundle index the edge serves for a version (exact-key R2: the
 *  directory `/app/<ver>/` itself 404s). */
export function bundleIndexPath(version: string): string {
  return `/app/${encodeURIComponent(version)}/index.html`
}

export interface PageLocation {
  pathname: string
  search: string
  hash: string
}

/** The same page on another bundle: the route tail, query and hash kept. */
export function bundleTargetUrl(loc: PageLocation, version: string): string | null {
  const parts = bundlePathParts(loc.pathname)
  if (!parts) return null
  const tail = parts.tail || 'index.html'
  return `/app/${encodeURIComponent(version)}/${tail}${loc.search}${loc.hash}`
}

/** The same page through the edge's launcher: `/?v=<version>` plus this
 *  page's other query params and hash (the launcher keeps both). */
export function launcherTargetUrl(loc: PageLocation, version: string): string {
  const params = new URLSearchParams(loc.search)
  params.delete('v')
  const rest = params.toString()
  return `/?v=${encodeURIComponent(version)}${rest ? `&${rest}` : ''}${loc.hash}`
}

export type BundleMoveReason =
  | 'not-web'
  | 'no-bundle-version'
  | 'not-versioned-path'
  | 'no-server-version'
  | 'same'
  | 'older'
  | 'tried'
  | 'already-there'

export type BundleMove =
  | { kind: 'stay'; reason: BundleMoveReason }
  | { kind: 'redirect' | 'prompt'; version: string; target: string }

/**
 * Pure rule: what to do about one accepted body. `phase` is `boot` for the
 * first version this page load learns, `live` for a later change.
 * `shellBundle`: the first body hid the version (ghost sign-in shell).
 * `alreadyTried`: the sessionStorage guard already holds this target.
 */
export function planBundleMove(a: {
  isWeb: boolean
  body: unknown
  bundleVersion: string | undefined
  location: PageLocation
  phase: 'boot' | 'live'
  shellBundle: boolean
  alreadyTried: boolean
}): BundleMove {
  if (!a.isWeb) return { kind: 'stay', reason: 'not-web' }
  const own = validVersion(a.bundleVersion)
  if (!own) return { kind: 'stay', reason: 'no-bundle-version' }
  if (!bundlePathParts(a.location.pathname)) return { kind: 'stay', reason: 'not-versioned-path' }
  const server = serverWebClientVersion(a.body)
  if (!server) return { kind: 'stay', reason: 'no-server-version' }
  if (server === own) return { kind: 'stay', reason: 'same' }
  const downgradeOk = a.phase === 'boot' && a.shellBundle
  if (compareVersions(server, own) < 0 && !downgradeOk) return { kind: 'stay', reason: 'older' }
  const target = bundleTargetUrl(a.location, server)
  if (!target) return { kind: 'stay', reason: 'not-versioned-path' }
  // We are already at that version's URL and the edge still handed us a
  // different build (an SPA fallback): its bundle is not really there.
  if (target === `${a.location.pathname}${a.location.search}${a.location.hash}`) {
    return { kind: 'stay', reason: 'already-there' }
  }
  if (a.phase === 'boot' && a.alreadyTried) return { kind: 'stay', reason: 'tried' }
  return { kind: a.phase === 'boot' ? 'redirect' : 'prompt', version: server, target }
}

// ── The bar ────────────────────────────────────────────────────────────

export type BundleBar =
  | { kind: 'updated'; version: string; target: string; via: 'direct' | 'launcher' }
  | { kind: 'missing'; version: string; bundleVersion: string }

interface BundleBarState {
  bar: BundleBar | null
  show: (bar: BundleBar | null) => void
}

export const useWebBundleBar = create<BundleBarState>((set) => ({
  bar: null,
  show: (bar) => set({ bar }),
}))

// ── The runner ConnectionGate calls ────────────────────────────────────

export interface BundleEnv {
  isWeb: boolean
  bundleVersion: string | undefined
  location: PageLocation
  replace: (url: string) => void
  fetch: typeof fetch
  storage: Pick<Storage, 'getItem' | 'setItem'> | null
  now: () => number
}

function defaultEnv(): BundleEnv {
  const web = typeof window !== 'undefined' && isWebClient()
  let storage: BundleEnv['storage'] = null
  try {
    storage = web ? window.sessionStorage : null
  } catch {
    storage = null
  }
  return {
    isWeb: web,
    bundleVersion: import.meta.env.VITE_APP_VERSION as string | undefined,
    location: web ? window.location : { pathname: '/', search: '', hash: '' },
    replace: (url) => window.location.replace(url),
    fetch: (...args) => fetch(...args),
    storage,
    now: () => Date.now(),
  }
}

/** One page load's memory (reset by tests). */
interface Tracker {
  firstBody: 'unseen' | 'versionless' | 'versioned'
  bootVersion: string | null
  lastVersion: string | null
  handled: Set<string>
}

let tracker: Tracker = freshTracker()

function freshTracker(): Tracker {
  return { firstBody: 'unseen', bootVersion: null, lastVersion: null, handled: new Set() }
}

/** Tests: forget this page load's checks and hide the bar. */
export function resetBundleVersionTracker(): void {
  tracker = freshTracker()
  useWebBundleBar.getState().show(null)
}

function readGuard(env: BundleEnv): Record<string, number> | null {
  if (!env.storage) return null
  try {
    const raw = env.storage.getItem(BUNDLE_MOVE_GUARD_KEY)
    if (!raw) return {}
    const parsed: unknown = JSON.parse(raw)
    return parsed && typeof parsed === 'object' ? (parsed as Record<string, number>) : {}
  } catch {
    return null
  }
}

function guardHolds(env: BundleEnv, version: string): boolean {
  const guard = readGuard(env)
  // No readable storage: never risk a reload loop.
  if (guard === null) return true
  const at = guard[version]
  return typeof at === 'number' && env.now() - at < BUNDLE_MOVE_GUARD_TTL_MS
}

function writeGuard(env: BundleEnv, version: string): boolean {
  const guard = readGuard(env)
  if (guard === null || !env.storage) return false
  try {
    env.storage.setItem(BUNDLE_MOVE_GUARD_KEY, JSON.stringify({ ...guard, [version]: env.now() }))
    return true
  } catch {
    return false
  }
}

/** How a version's bundle is reached: `direct` (the HEAD answered it),
 *  `launcher` (the edge redirected the HEAD to `/`: published, but only a
 *  launcher-started page may load it), or `missing`. */
export type BundleReach = 'direct' | 'launcher' | 'missing'

function isLauncherPath(url: string): boolean {
  try {
    const path = new URL(url, 'http://x').pathname
    return path === '/' || path === '/index.html'
  } catch {
    return false
  }
}

/** HEAD the version's index (same-origin, no-store, redirects followed so
 *  the final URL shows an edge bounce to `/`); a static server that refuses
 *  HEAD gets a GET, like the loader. Any failure = missing. */
export async function bundleReach(fetchFn: typeof fetch, version: string): Promise<BundleReach> {
  const url = bundleIndexPath(version)
  const init = (method: string): RequestInit => ({
    method,
    credentials: 'same-origin',
    cache: 'no-store',
    redirect: 'follow',
    signal: AbortSignal.timeout(HEAD_TIMEOUT_MS),
  })
  try {
    let res = await fetchFn(url, init('HEAD'))
    if (res.status === 405 || res.status === 501) res = await fetchFn(url, init('GET'))
    // A `manual` fetch elsewhere would see an opaque redirect: same answer.
    if (res.type === 'opaqueredirect') return 'launcher'
    if (res.redirected && isLauncherPath(res.url)) return res.ok ? 'launcher' : 'missing'
    return res.ok ? 'direct' : 'missing'
  } catch {
    return 'missing'
  }
}

/**
 * Note one accepted `/boot-status` body (ConnectionGate's poll). Moves onto,
 * or offers, the server's own web client version per the rules above.
 * Returns what it did (for tests and the log).
 */
export async function noteServerBundleVersion(
  body: unknown,
  env: BundleEnv = defaultEnv(),
): Promise<BundleMove | { kind: 'missing'; version: string } | null> {
  if (!env.isWeb) return null
  const version = serverWebClientVersion(body)
  if (tracker.firstBody === 'unseen') tracker.firstBody = version ? 'versioned' : 'versionless'
  if (!version) return null
  const phase: 'boot' | 'live' = tracker.bootVersion === null ? 'boot' : 'live'
  if (phase === 'boot') tracker.bootVersion = version
  else if (version === tracker.lastVersion) return null
  tracker.lastVersion = version

  const own = env.bundleVersion ?? ''
  const plan = planBundleMove({
    isWeb: true,
    body,
    bundleVersion: env.bundleVersion,
    location: env.location,
    phase,
    shellBundle: tracker.firstBody === 'versionless',
    alreadyTried: guardHolds(env, version),
  })
  const bar = useWebBundleBar.getState()
  if (plan.kind === 'stay') {
    // Back on our own version, or a rollback below it: nothing to offer.
    if (plan.reason === 'same' || plan.reason === 'older') bar.show(null)
    else if (plan.reason === 'tried' || plan.reason === 'already-there') {
      console.warn(`[web-bundle] server web client ${version} ≠ bundle ${own}; not moving (${plan.reason})`)
      bar.show({ kind: 'missing', version, bundleVersion: own })
    }
    return plan
  }
  // One HEAD (and at most one move) per target per page load.
  if (tracker.handled.has(version)) return null
  tracker.handled.add(version)
  const reach = await bundleReach(env.fetch, version)
  if (reach === 'missing') {
    console.warn(`[web-bundle] server web client ${version} ≠ bundle ${own}; /app/${version}/ is not published, staying`)
    bar.show({ kind: 'missing', version, bundleVersion: own })
    return { kind: 'missing', version }
  }
  const target = reach === 'launcher' ? launcherTargetUrl(env.location, version) : plan.target
  if (plan.kind === 'redirect') {
    if (!writeGuard(env, version)) {
      // No guard means no proof against a loop: let the person choose.
      bar.show({ kind: 'updated', version, target, via: reach })
      return { ...plan, kind: 'prompt', target }
    }
    console.warn(`[web-bundle] bundle ${own} ≠ server web client ${version}; moving to ${target}`)
    env.replace(target)
    return { ...plan, target }
  }
  bar.show({ kind: 'updated', version, target, via: reach })
  return { ...plan, target }
}

/** The bar's Reload: onto the offered version (straight, or through the
 *  edge's launcher), keeping the route, query and hash the person is on
 *  NOW (not when the bar appeared). */
export function reloadOntoOfferedBundle(
  version: string,
  via: 'direct' | 'launcher',
  env: Pick<BundleEnv, 'replace' | 'location'> = defaultEnv(),
): void {
  const target = via === 'launcher' ? launcherTargetUrl(env.location, version) : bundleTargetUrl(env.location, version)
  if (target) env.replace(target)
}
