// Hosted web: the open bundle follows the server's web client version.
import { describe, it, expect, beforeEach, vi } from 'vitest'
import { renderToStaticMarkup } from 'react-dom/server'
import React from 'react'
import {
  BUNDLE_MOVE_GUARD_KEY,
  BUNDLE_MOVE_GUARD_TTL_MS,
  bundlePathParts,
  compareVersions,
  noteServerBundleVersion,
  planBundleMove,
  reloadOntoOfferedBundle,
  resetBundleVersionTracker,
  serverWebClientVersion,
  useWebBundleBar,
  type BundleEnv,
} from './bundle-version'
import { WebBundleBarView } from './WebBundleBar'

const renderBar = () =>
  renderToStaticMarkup(
    React.createElement(WebBundleBarView, { bar: useWebBundleBar.getState().bar!, onDismiss: () => {} }),
  )

const FULL = (version: string) => ({ view: 'full', version, protocol: 1, phase: 'ready' })
const PUBLIC = { view: 'public', protocol: 1, phase: 'ready' }

class MemStorage {
  map = new Map<string, string>()
  getItem(k: string): string | null {
    return this.map.has(k) ? (this.map.get(k) as string) : null
  }
  setItem(k: string, v: string): void {
    this.map.set(k, v)
  }
}

/** `edge`: the K2 edge, which 302s a non-launcher page's `/app/<V>/` fetch
 *  to `/` (fetch follows it: a 200 launcher page, `redirected`, url `/`). */
function makeEnv(over: Partial<BundleEnv> & { exists?: (url: string) => boolean; edge?: boolean } = {}) {
  const replace = vi.fn<(url: string) => void>()
  const heads: string[] = []
  const exists = over.exists ?? (() => true)
  const edge = over.edge ?? false
  const fetchFn = vi.fn(async (url: RequestInfo | URL, init?: RequestInit) => {
    heads.push(`${init?.method} ${String(url)} ${init?.cache}`)
    if (edge) return { ok: true, status: 200, type: 'basic', redirected: true, url: 'https://box.k2.dev/' }
    const ok = exists(String(url))
    return { ok, status: ok ? 200 : 404, type: 'basic', redirected: false, url: `https://box.k2.dev${String(url)}` }
  }) as unknown as typeof fetch
  const storage = new MemStorage()
  const env: BundleEnv = {
    isWeb: true,
    bundleVersion: '0.46.17',
    location: { pathname: '/app/0.46.17/index.html', search: '?x=1', hash: '#ticket=abc' },
    replace,
    fetch: fetchFn,
    storage,
    now: () => 1_000_000,
    ...over,
  }
  return { env, replace, heads, storage }
}

describe('web bundle version', () => {
  beforeEach(() => {
    resetBundleVersionTracker()
    vi.spyOn(console, 'warn').mockImplementation(() => {})
  })

  it('reads webClientVersion first, then version, and rejects path tricks', () => {
    expect(serverWebClientVersion({ version: '0.46.18', webClientVersion: '0.46.19' })).toBe('0.46.19')
    expect(serverWebClientVersion({ version: '0.46.18', webClientVersion: ' ' })).toBe('0.46.18')
    expect(serverWebClientVersion(PUBLIC)).toBeNull()
    expect(serverWebClientVersion({ version: '../0.46.18' })).toBeNull()
    expect(serverWebClientVersion({ version: '0.46.18/x' })).toBeNull()
    expect(bundlePathParts('/app/0.40.71/index.html')).toEqual({ version: '0.40.71', tail: 'index.html' })
    expect(bundlePathParts('/app/0.40.71/')).toEqual({ version: '0.40.71', tail: '' })
    expect(bundlePathParts('/')).toBeNull()
    expect(compareVersions('0.46.18', '0.46.17')).toBe(1)
    expect(compareVersions('0.46.9', '0.46.10')).toBe(-1)
    expect(compareVersions('0.46.18-rc.1', '0.46.18')).toBe(-1)
  })

  it('an older bundle moves to the newer one, keeping the route, query and hash', async () => {
    const { env, replace, heads, storage } = makeEnv()
    const r = await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(heads).toEqual(['HEAD /app/0.46.18/index.html no-store'])
    expect(replace).toHaveBeenCalledTimes(1)
    expect(replace).toHaveBeenCalledWith('/app/0.46.18/index.html?x=1#ticket=abc')
    expect(r).toMatchObject({ kind: 'redirect', version: '0.46.18' })
    expect(JSON.parse(storage.getItem(BUNDLE_MOVE_GUARD_KEY) as string)).toEqual({ '0.46.18': 1_000_000 })
    expect(useWebBundleBar.getState().bar).toBeNull()
  })

  it('a saved /app/<ver>/ link (no index.html) lands on the new index', async () => {
    const { env, replace } = makeEnv({
      bundleVersion: '0.40.71',
      location: { pathname: '/app/0.40.71/', search: '', hash: '#home' },
    })
    await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(replace).toHaveBeenCalledWith('/app/0.46.18/index.html#home')
  })

  it('same version: no HEAD, no move, no bar', async () => {
    const { env, replace, heads } = makeEnv()
    const r = await noteServerBundleVersion(FULL('0.46.17'), env)
    expect(r).toEqual({ kind: 'stay', reason: 'same' })
    expect(heads).toEqual([])
    expect(replace).not.toHaveBeenCalled()
    expect(useWebBundleBar.getState().bar).toBeNull()
  })

  it('target bundle missing: no move, and a clear message', async () => {
    const { env, replace, heads } = makeEnv({ exists: () => false })
    const r = await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(heads).toHaveLength(1)
    expect(replace).not.toHaveBeenCalled()
    expect(r).toEqual({ kind: 'missing', version: '0.46.18' })
    expect(useWebBundleBar.getState().bar).toEqual({ kind: 'missing', version: '0.46.18', bundleVersion: '0.46.17' })
    const html = renderBar()
    expect(html).toContain("This server runs K2 0.46.18, but its web client isn&#x27;t published yet")
    expect(html).not.toContain('Reload')
    // The next poll with the same version does not HEAD again.
    await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(heads).toHaveLength(1)
  })

  it('a HEAD network error stays too', async () => {
    const { env, replace } = makeEnv()
    env.fetch = (async () => {
      throw new TypeError('network')
    }) as unknown as typeof fetch
    const r = await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(r).toEqual({ kind: 'missing', version: '0.46.18' })
    expect(replace).not.toHaveBeenCalled()
  })

  it('a newer bundle on an older server never downgrades', async () => {
    const { env, replace, heads } = makeEnv({
      bundleVersion: '0.46.18',
      location: { pathname: '/app/0.46.18/index.html', search: '', hash: '' },
    })
    const r = await noteServerBundleVersion(FULL('0.46.17'), env)
    expect(r).toEqual({ kind: 'stay', reason: 'older' })
    expect(heads).toEqual([])
    expect(replace).not.toHaveBeenCalled()
  })

  it('the ghost sign-in shell (first body hid the version) still moves onto the server version once signed in', async () => {
    const { env, replace } = makeEnv({
      bundleVersion: '0.46.18',
      location: { pathname: '/app/0.46.18/index.html', search: '', hash: '#a' },
    })
    expect(await noteServerBundleVersion(PUBLIC, env)).toBeNull()
    await noteServerBundleVersion(FULL('0.46.17'), env)
    expect(replace).toHaveBeenCalledWith('/app/0.46.17/index.html#a')
  })

  it('loop guard: a target already tried this tab session is not moved to again', async () => {
    const first = makeEnv()
    await noteServerBundleVersion(FULL('0.46.18'), first.env)
    expect(first.replace).toHaveBeenCalledTimes(1)

    // The next page load (same tab, so the same sessionStorage) is still on
    // 0.46.17: the edge served an old bundle for the new path.
    resetBundleVersionTracker()
    const second = makeEnv({ storage: first.storage })
    const r = await noteServerBundleVersion(FULL('0.46.18'), second.env)
    expect(r).toEqual({ kind: 'stay', reason: 'tried' })
    expect(second.replace).not.toHaveBeenCalled()
    expect(second.heads).toEqual([])
    expect(useWebBundleBar.getState().bar).toMatchObject({ kind: 'missing', version: '0.46.18' })

    // Past the guard window it may try once more.
    resetBundleVersionTracker()
    const later = makeEnv({ storage: first.storage, now: () => 1_000_000 + BUNDLE_MOVE_GUARD_TTL_MS + 1 })
    await noteServerBundleVersion(FULL('0.46.18'), later.env)
    expect(later.replace).toHaveBeenCalledTimes(1)
  })

  it('loop guard: one move per target per page load, even if replace did not unload', async () => {
    const { env, replace, heads } = makeEnv()
    await noteServerBundleVersion(FULL('0.46.18'), env)
    await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(replace).toHaveBeenCalledTimes(1)
    expect(heads).toHaveLength(1)
  })

  it('loop guard: unreadable storage never auto-moves', async () => {
    const { env, replace } = makeEnv({
      storage: {
        getItem: () => {
          throw new Error('SecurityError')
        },
        setItem: () => {
          throw new Error('SecurityError')
        },
      },
    })
    const r = await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(r).toEqual({ kind: 'stay', reason: 'tried' })
    expect(replace).not.toHaveBeenCalled()
  })

  it('already at the target path but the edge served another build: stay and say so', () => {
    const plan = planBundleMove({
      isWeb: true,
      body: FULL('0.46.18'),
      bundleVersion: '0.46.17',
      location: { pathname: '/app/0.46.18/index.html', search: '', hash: '' },
      phase: 'boot',
      shellBundle: false,
      alreadyTried: false,
    })
    expect(plan).toEqual({ kind: 'stay', reason: 'already-there' })
  })

  it('desktop and the dev server never move', () => {
    const base = {
      body: FULL('0.46.18'),
      bundleVersion: '0.46.17',
      location: { pathname: '/app/0.46.17/index.html', search: '', hash: '' },
      phase: 'boot' as const,
      shellBundle: false,
      alreadyTried: false,
    }
    expect(planBundleMove({ ...base, isWeb: false })).toEqual({ kind: 'stay', reason: 'not-web' })
    expect(planBundleMove({ ...base, isWeb: true, location: { pathname: '/', search: '', hash: '' } })).toEqual({
      kind: 'stay',
      reason: 'not-versioned-path',
    })
  })

  it('a live version change shows the Reload bar, not a redirect', async () => {
    const { env, replace, heads } = makeEnv()
    await noteServerBundleVersion(FULL('0.46.17'), env) // boot: same
    await noteServerBundleVersion(FULL('0.46.17'), env) // health tick
    expect(useWebBundleBar.getState().bar).toBeNull()

    const r = await noteServerBundleVersion(FULL('0.46.18'), env) // daemon upgraded
    expect(r).toMatchObject({ kind: 'prompt', version: '0.46.18' })
    expect(heads).toEqual(['HEAD /app/0.46.18/index.html no-store'])
    expect(replace).not.toHaveBeenCalled()
    expect(useWebBundleBar.getState().bar).toMatchObject({ kind: 'updated', version: '0.46.18' })
    const html = renderBar()
    expect(html).toContain('K2 was updated to 0.46.18.')
    expect(html).toContain('Reload')

    // Reload keeps the route the person is on now.
    const click = vi.fn<(url: string) => void>()
    reloadOntoOfferedBundle('0.46.18', 'direct', {
      replace: click,
      location: { pathname: '/app/0.46.17/index.html', search: '', hash: '#moved-on' },
    })
    expect(click).toHaveBeenCalledWith('/app/0.46.18/index.html#moved-on')

    // The daemon goes back to our version: the bar comes down.
    await noteServerBundleVersion(FULL('0.46.17'), env)
    expect(useWebBundleBar.getState().bar).toBeNull()
    expect(replace).not.toHaveBeenCalled()
  })

  it('a live change whose bundle is missing shows the message, no Reload', async () => {
    const { env, replace } = makeEnv({ exists: () => false })
    await noteServerBundleVersion(FULL('0.46.17'), env)
    await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(replace).not.toHaveBeenCalled()
    expect(useWebBundleBar.getState().bar).toMatchObject({ kind: 'missing', version: '0.46.18' })
  })

  it('behind the K2 edge (HEAD bounced to /): moves through the launcher with ?v=, the query and the hash', async () => {
    const { env, replace, heads, storage } = makeEnv({ edge: true })
    const r = await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(heads).toEqual(['HEAD /app/0.46.18/index.html no-store'])
    expect(replace).toHaveBeenCalledTimes(1)
    expect(replace).toHaveBeenCalledWith('/?v=0.46.18&x=1#ticket=abc')
    expect(r).toMatchObject({ kind: 'redirect', version: '0.46.18', target: '/?v=0.46.18&x=1#ticket=abc' })
    // The guard holds the version, so a bounce back to this old bundle stays.
    expect(JSON.parse(storage.getItem(BUNDLE_MOVE_GUARD_KEY) as string)).toEqual({ '0.46.18': 1_000_000 })
  })

  it('behind the K2 edge, an opaque redirect (redirect: manual) also means "via the launcher"', async () => {
    const { env, replace } = makeEnv()
    env.fetch = (async () => ({ ok: false, status: 0, type: 'opaqueredirect', redirected: false, url: '' })) as unknown as typeof fetch
    await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(replace).toHaveBeenCalledWith('/?v=0.46.18&x=1#ticket=abc')
  })

  it('behind the K2 edge, a live change offers Reload through the launcher', async () => {
    const { env, replace } = makeEnv({ edge: true })
    await noteServerBundleVersion(FULL('0.46.17'), env)
    await noteServerBundleVersion(FULL('0.46.18'), env)
    expect(replace).not.toHaveBeenCalled()
    expect(useWebBundleBar.getState().bar).toMatchObject({ kind: 'updated', version: '0.46.18', via: 'launcher' })
    const click = vi.fn<(url: string) => void>()
    reloadOntoOfferedBundle('0.46.18', 'launcher', {
      replace: click,
      location: { pathname: '/app/0.46.17/index.html', search: '?v=0.46.17&y=2', hash: '#now' },
    })
    expect(click).toHaveBeenCalledWith('/?v=0.46.18&y=2#now')
  })

  it('a bounce to / that itself fails is still missing', async () => {
    const { env, replace } = makeEnv()
    env.fetch = (async () => ({ ok: false, status: 404, type: 'basic', redirected: true, url: 'https://box.k2.dev/' })) as unknown as typeof fetch
    expect(await noteServerBundleVersion(FULL('0.46.18'), env)).toEqual({ kind: 'missing', version: '0.46.18' })
    expect(replace).not.toHaveBeenCalled()
  })

  it('not the web client: does nothing', async () => {
    const { env, replace, heads } = makeEnv({ isWeb: false })
    expect(await noteServerBundleVersion(FULL('0.46.18'), env)).toBeNull()
    expect(replace).not.toHaveBeenCalled()
    expect(heads).toEqual([])
  })
})
