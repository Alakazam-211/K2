// Home avatars — pictures for agents on other servers
// (prd-home-picker-and-remote-avatars-v1 S4: P10–P12, P16, P17, P19, P20;
// vs-live P27, P28, P30, P33).
//
// A Home row on another server used to paint only its letter. Now:
//   - The app fetches the agent's image from THAT server with THAT server's
//     own login, through the row's host scope (`scopeForHost(hostKey)`):
//     one `projects/list` per server, and `projects/get-icon` only for a
//     workspace whose listing has no image (P11). The local daemon holds no
//     remote logins; it only stores (P10).
//   - The bytes go to this computer's daemon cache (`/cli/home/avatars/put`,
//     owner token, `~/.k2/cache/agent-avatars/`), shared by every window.
//   - Rows paint from the cache as a DATA URL: the app CSP's `img-src`
//     blocks a local `http://127.0.0.1` image (P14).
//   - Fetch on add and on first row load; refresh an image at most once per
//     24 h per row, keyed on the cache's `fetchedAt`, so two windows don't
//     both refetch (P12). An offline server, or one that needs a sign-in,
//     keeps its cached image and is tried again when the pool finds it live.
//   - No image → `missing` (the row paints its letter) (P16). A miss lives
//     short: re-checked after 10 min, then 30 min, then hourly while it
//     stays missing (`AVATAR_MISSING_RECHECK_MS`; the miss count is this
//     window's, so a relaunch starts at 10 min again). Before this a miss
//     kept the 24 h rule, and a peer that set its picture after the first
//     look stayed a letter all day (Appa, 10-28).
//   - An explicit ask re-checks a missing row at once (opening the row, the
//     Add Agent picker opening), and a peer's `agent_identity_changed` with
//     `image` (seen by an open Home room on that server) refetches the row
//     whatever its age. Neither runs twice inside `AVATAR_RECHECK_FLOOR_MS`.
//   - While Home (or a Garden) is on screen, due misses are looked for
//     every `AVATAR_RECHECK_TICK_MS`, so a re-check doesn't wait for focus.
//   - Cleanup: 2 s after any Home row change, and once when Home mounts, the
//     union of every Home's addresses goes to `/cli/home/avatars/prune`, but
//     only when the Homes store was read from storage (P17, P28).
//   - `local` rows on a remote window read this computer's daemon live and
//     cache nothing (P27).
//   - The web client has no local daemon: everything here is off (P20).
//
// Agent colors ride the same listings (Rosson 10-30: a row on another
// server keeps its agent's own color; the machine chip already says where
// it lives):
//   - Every listing this module reads (a stale picture's, the Add Agent
//     picker's on add, the connected server's store, `local` rows) records
//     each row's `color`, and an open Home room's list (a rename, a
//     recolor seen as `agent_identity_changed`) records it too.
//   - A row whose color this computer has never seen counts as due ONCE per
//     window: its server is listed for colors only (no `get-icon`).
//   - Only `#rgb` / `#rrggbb` hex is kept, normalized to lowercase
//     `#rrggbb` (`agentLookColor`, the client side of the daemon's
//     `agent_look::normalize_color`): another server never gets a CSS
//     string into a style.
//   - Colors are kept in localStorage `k2.homes.agentColors.v1` (every
//     window shares it; re-read with the cache on mount and focus) and
//     pruned with the pictures. Storage that throws just means muted until
//     the next listing.
//
// Nothing here runs unless Home is on screen (`HomeShellEffects`), so a
// headless daemon misses nothing: only a screen uses these pictures.

import { useEffect, useRef } from 'react'
import { create, useStore } from 'zustand'
import { daemonCliGet, daemonCliPost } from '@/lib/daemon-cli'
import { scopeForHost } from '@/kessel/server-scope'
import { isWebClient } from '@/lib/is-web'
import { LOCAL_HOME_HOST, activeHomeHostKey, findWorkspaceForRow, parseHomeAddress } from '@/lib/home-address'
import { useHomesStore, selectedHome, type HomeRow, type HomesState } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { hostPool } from '@/lib/host-pool-instance'
import { agentLookColor } from '@/lib/agent-look'

/** A row's image is refreshed at most this often (P12c). */
export const AVATAR_REFRESH_MS = 24 * 60 * 60 * 1000
/** A `missing` row is re-checked after these delays: the first entry for a
 *  miss this window has not re-checked yet, the next after one more miss,
 *  and the last repeats (10 min, 30 min, then hourly). */
export const AVATAR_MISSING_RECHECK_MS: readonly number[] = [10 * 60_000, 30 * 60_000, 60 * 60_000]
/** A forced re-check (user ask, identity event) never refetches a row
 *  fetched less than this long ago, so repeated clicks can't loop. */
export const AVATAR_RECHECK_FLOOR_MS = 5_000
/** While a Home or Garden is on screen, due misses are looked for this
 *  often (a local check; the network is asked only for a due row). */
export const AVATAR_RECHECK_TICK_MS = 2 * 60_000
/** Prune runs this long after the last row change (P17). */
export const AVATAR_PRUNE_DELAY_MS = 2_000
/** One cache GET asks for at most this many addresses (P14). */
export const AVATAR_GET_MAX_ADDRESSES = 100
/** …and keeps its encoded `addresses=` value under this many characters,
 *  well inside the daemon's 16 KiB request head (P30). */
export const AVATAR_GET_MAX_QUERY_CHARS = 8 * 1024
/** After a failed listing, that server waits this long (or for a pool
 *  transition) before it is asked again. */
export const AVATAR_FAILURE_BACKOFF_MS = 60_000

/** localStorage key for the agent colors of Home rows (by row address). */
export const HOME_AGENT_COLORS_KEY = 'k2.homes.agentColors.v1'

/** Parse the stored color map; any entry that is not a row address with a
 *  valid color is dropped. */
export function parseAgentColors(raw: string | null): Record<string, string> {
  if (!raw) return {}
  let doc: unknown
  try {
    doc = JSON.parse(raw)
  } catch {
    return {}
  }
  if (!doc || typeof doc !== 'object') return {}
  const { version, colors } = doc as { version?: unknown; colors?: unknown }
  if (version !== 1 || !colors || typeof colors !== 'object') return {}
  const out: Record<string, string> = {}
  for (const [a, c] of Object.entries(colors as Record<string, unknown>)) {
    const color = typeof c === 'string' ? agentLookColor(c) : null
    if (color && parseHomeAddress(a)) out[a] = color
  }
  return out
}

/** One row's cache entry, as `GET /cli/home/avatars` returns it. */
export interface CachedAvatar {
  dataUrl: string | null
  missing: boolean
  fetchedAt: number
  sha256: string | null
}

/** The fields of a `projects/list` row this module reads. */
export interface ListedAvatarWorkspace {
  id: string
  name: string
  handle?: string | null
  path?: string | null
  /** `projects.color` there; validated with `agentLookColor` before use. */
  color?: string | null
  iconUrl?: string | null
}

interface HomeAvatarState {
  /** This computer's cache, by row address. */
  cached: Record<string, CachedAvatar>
  /** P27: `local` rows on a remote window, read live (never cached). */
  local: Record<string, string | null>
  /** Agent colors (validated `#rrggbb`), by row address. */
  colors: Record<string, string>
}

export const useHomeAvatarStore = create<HomeAvatarState>(() => ({ cached: {}, local: {}, colors: {} }))

/** How long a `missing` entry lives, after `misses` misses in a row this
 *  window has recorded for it (0: read from the cache, not re-checked yet). */
export function missingRecheckMs(misses: number): number {
  const i = Math.min(Math.max(misses - 1, 0), AVATAR_MISSING_RECHECK_MS.length - 1)
  return AVATAR_MISSING_RECHECK_MS[i]
}

/** Is a cache entry due for a refetch at `now`? An image follows the 24 h
 *  rule (P12); a miss follows `missingRecheckMs(misses)`; a forced entry
 *  is due once it is `AVATAR_RECHECK_FLOOR_MS` old. No entry is due. */
export function avatarEntryStale(
  e: Pick<CachedAvatar, 'missing' | 'fetchedAt'> | undefined,
  now: number,
  misses: number,
  forced: boolean,
): boolean {
  if (!e) return true
  const age = now - e.fetchedAt
  if (forced && age >= AVATAR_RECHECK_FLOOR_MS) return true
  return age > (e.missing ? missingRecheckMs(misses) : AVATAR_REFRESH_MS)
}

/** Only `data:image/…` values are images here; an `https://` icon (a remote
 *  `set-icon` stores any string) is treated as no image (P8, P15). */
export function isImageDataUrl(v: unknown): v is string {
  return typeof v === 'string' && v.startsWith('data:image/')
}

/** Split addresses into cache GETs: ≤ 100 each, encoded value ≤ 8 KiB. */
export function avatarQueryBatches(addresses: readonly string[]): string[][] {
  const out: string[][] = []
  let cur: string[] = []
  let chars = 0
  for (const a of addresses) {
    const len = encodeURIComponent(a).length + (cur.length > 0 ? 3 : 0) // `%2C`
    if (cur.length > 0 && (cur.length >= AVATAR_GET_MAX_ADDRESSES || chars + len > AVATAR_GET_MAX_QUERY_CHARS)) {
      out.push(cur)
      cur = []
      chars = 0
    }
    cur.push(a)
    chars += encodeURIComponent(a).length + (cur.length > 1 ? 3 : 0)
  }
  if (cur.length > 0) out.push(cur)
  return out
}

/** Is a 400 from the cache a refusal of the image itself (too large, a
 *  type it doesn't store…)? Then the row is recorded as having no image. */
function isImageRefusal(err: unknown): boolean {
  const msg = err instanceof Error ? err.message : String(err)
  return /not_data_url|type_refused|type_mismatch|too_large|svg_refused|svg refused|image type|not a .* image|cache takes at most|base64 data:image/.test(
    msg,
  )
}

export interface HomeAvatarDeps {
  now: () => number
  isWeb: () => boolean
  /** `GET /cli/home/avatars` on this computer's daemon. */
  cacheGet: (addresses: string[]) => Promise<Record<string, CachedAvatar>>
  /** `POST /cli/home/avatars/put` on this computer's daemon. */
  cachePut: (address: string, dataUrl: string | null) => Promise<void>
  /** `POST /cli/home/avatars/prune` on this computer's daemon. */
  cachePrune: (keep: string[]) => Promise<void>
  /** `GET /cli/<route>` on `hostKey`, with THAT server's own login. */
  hostGet: <T>(hostKey: string, route: string, params?: Record<string, string>) => Promise<T>
  /** The stored agent colors (`HOME_AGENT_COLORS_KEY`), raw; null = none.
   *  May throw (a locked-down webview). */
  colorsLoad: () => string | null
  /** Store the agent colors (raw JSON). May throw (quota, private mode). */
  colorsSave: (raw: string) => void
}

export interface AvatarSyncInput {
  /** The selected Home's rows. */
  rows: readonly HomeRow[]
  /** The window's server, as a row host key. */
  connectedKey: string
  /** That server's workspaces from the projects store (no network), or
   *  null when the window is not connected. */
  connectedProjects: readonly ListedAvatarWorkspace[] | null
  /** May `hostKey` be asked right now (pool: live and signed in)? */
  reachable: (hostKey: string) => boolean
}

export interface HomeAvatarSync {
  /** Re-read the cache for these addresses (mount, focus, after puts). */
  readCache: (addresses: readonly string[]) => Promise<void>
  /** Fetch every stale non-`local` row whose server can be asked now. */
  refresh: (input: AvatarSyncInput) => Promise<void>
  /** P27: `local` rows on a remote window, read live from this computer. */
  readLocalRows: (rows: readonly HomeRow[], connectedKey: string) => Promise<void>
  /** P33: the picker already holds the listing — put without a list call.
   *  Never throws; a failure is logged and the sync fills it in later. */
  putFromListing: (hostKey: string, address: string, ws: ListedAvatarWorkspace) => Promise<void>
  /** P17: prune the cache to `keep`, after the 2 s debounce. */
  schedulePrune: (keep: () => string[] | null) => void
  /** Run a pending prune now (tests). */
  flushPrune: () => Promise<void>
  /** Record the agent colors of `rows` on `hostKey` from a list of that
   *  server's workspaces another surface already holds (an open Home
   *  room's list). No network. */
  noteListColors: (hostKey: string, list: readonly ListedAvatarWorkspace[], rows: readonly HomeRow[]) => void
  /** Refetch these rows now (within the last `refresh` input's rows):
   *  `'user'` (an explicit ask) only those whose cache says `missing`,
   *  `'changed'` (the peer says its picture changed) any. Both respect
   *  `AVATAR_RECHECK_FLOOR_MS` and the server/pool rules of `refresh`. */
  recheck: (addresses: readonly string[], why: 'user' | 'changed') => Promise<void>
  /** The tick: re-read the cache for misses that are due (another window
   *  may have re-checked them), then refresh what is still due. Asks no
   *  one when nothing is due. */
  recheckDue: (input: AvatarSyncInput) => Promise<void>
}

export function createHomeAvatarSync(deps: HomeAvatarDeps): HomeAvatarSync {
  const store = useHomeAvatarStore
  const read = new Set<string>()
  const pending = new Set<string>()
  const inflightHosts = new Map<string, Promise<void>>()
  const failedAt = new Map<string, number>()
  const logged = new Set<string>()
  /** Misses in a row this window recorded, by address. */
  const misses = new Map<string, number>()
  /** Rows a user ask or an identity event wants refetched. */
  const forced = new Set<string>()
  /** The last `refresh` input, and every row any caller (Home, a Garden)
   *  has refreshed, so a recheck can find a row wherever it is shown. */
  let lastInput: AvatarSyncInput | null = null
  const knownRows = new Map<string, HomeRow>()
  let localInflight: Promise<void> | null = null
  let pruneTimer: ReturnType<typeof setTimeout> | null = null
  let pruneKeep: (() => string[] | null) | null = null
  /** Rows whose server answered a listing this window (found or not): a
   *  row with no known color is due for a colors-only listing until then. */
  const colorTried = new Set<string>()

  const logOnce = (key: string, msg: string, err: unknown): void => {
    if (logged.has(key)) return
    logged.add(key)
    console.warn(`[home-avatars] ${msg}`, err)
  }

  /** Re-read the stored colors (another window may have written them). */
  const loadColors = (): void => {
    let raw: string | null
    try {
      raw = deps.colorsLoad()
    } catch (err) {
      logOnce('colors-load', 'could not read the stored agent colors:', err)
      return
    }
    const colors = parseAgentColors(raw)
    const prev = store.getState().colors
    const same =
      Object.keys(colors).length === Object.keys(prev).length &&
      Object.entries(colors).every(([a, c]) => prev[a] === c)
    if (!same) store.setState({ colors })
  }

  const saveColors = (colors: Record<string, string>): void => {
    try {
      deps.colorsSave(JSON.stringify({ version: 1, colors }))
    } catch (err) {
      logOnce('colors-save', 'could not store the agent colors:', err)
    }
  }

  /** Record each `[address, listed color]`; only what `agentLookColor`
   *  accepts is kept (the same check as the agent's color on its own
   *  server), and an invalid color forgets the row's (it paints muted).
   *  Stores only when something changed. */
  const noteColors = (found: ReadonlyArray<readonly [string, string | null | undefined]>): void => {
    const prev = store.getState().colors
    let next: Record<string, string> | null = null
    for (const [address, raw] of found) {
      const color = agentLookColor(raw)
      if (color === ((next ?? prev)[address] ?? null)) continue
      next = next ?? { ...prev }
      if (color) next[address] = color
      else delete next[address]
    }
    if (!next) return
    store.setState({ colors: next })
    saveColors(next)
  }

  const colorDue = (address: string): boolean =>
    store.getState().colors[address] === undefined && !colorTried.has(address)

  const readCache = async (addresses: readonly string[]): Promise<void> => {
    if (deps.isWeb() || addresses.length === 0) return
    loadColors()
    const uniq = [...new Set(addresses)]
    for (const batch of avatarQueryBatches(uniq)) {
      let got: Record<string, CachedAvatar>
      try {
        got = await deps.cacheGet(batch)
      } catch (err) {
        logOnce('cache-get', 'could not read this computer’s avatar cache:', err)
        failedAt.set(LOCAL_HOME_HOST, deps.now())
        return
      }
      for (const a of batch) read.add(a)
      store.setState((s) => {
        const cached = { ...s.cached }
        for (const a of batch) {
          const e = got[a]
          if (e && typeof e.fetchedAt === 'number') cached[a] = e
          else delete cached[a]
        }
        return { cached }
      })
    }
  }

  /** Store `dataUrl` (or a miss) and say what the cache now holds:
   *  `'image'`, `'missing'`, or null when nothing was stored. A miss grows
   *  the row's backoff; an image resets it. */
  const put = async (address: string, dataUrl: string | null): Promise<'image' | 'missing' | null> => {
    const recorded = (kind: 'image' | 'missing'): 'image' | 'missing' => {
      if (kind === 'missing') misses.set(address, (misses.get(address) ?? 0) + 1)
      else misses.delete(address)
      return kind
    }
    try {
      await deps.cachePut(address, dataUrl)
      return recorded(dataUrl === null ? 'missing' : 'image')
    } catch (err) {
      if (dataUrl !== null && isImageRefusal(err)) {
        // The cache won't take this image (too big, odd type): the row
        // paints its letter and is re-checked on the miss backoff.
        try {
          await deps.cachePut(address, null)
          return recorded('missing')
        } catch (err2) {
          logOnce(`put:${address}`, `could not record ${address} as having no image:`, err2)
          return null
        }
      }
      logOnce(`put:${address}`, `could not cache the image for ${address}:`, err)
      failedAt.set(LOCAL_HOME_HOST, deps.now())
      return null
    }
  }

  /** The image for `ws` on `hostKey`: the listing's, else `get-icon` on
   *  that server (`ask` false → listing only). Null = no image. */
  const imageFor = async (hostKey: string, ws: ListedAvatarWorkspace, ask: boolean): Promise<string | null | undefined> => {
    if (isImageDataUrl(ws.iconUrl)) return ws.iconUrl
    if (!ask) return undefined
    if (!ws.path) return null
    const r = await deps.hostGet<{ found?: unknown; dataUrl?: unknown }>(hostKey, 'projects/get-icon', {
      path: ws.path,
      project_id: ws.id,
    })
    return r && r.found === true && isImageDataUrl(r.dataUrl) ? r.dataUrl : null
  }

  const stale = (address: string, now: number): boolean =>
    avatarEntryStale(store.getState().cached[address], now, misses.get(address) ?? 0, forced.has(address))

  /** List `hostKey` once and record what it says for `rows`: always their
   *  colors, and their pictures when `images` (false: the server was
   *  listed only for colors this window has never seen). */
  const refreshHost = async (
    hostKey: string,
    rows: HomeRow[],
    listing: () => Promise<readonly ListedAvatarWorkspace[]>,
    ask: boolean,
    images: boolean,
  ): Promise<void> => {
    let list: readonly ListedAvatarWorkspace[]
    try {
      list = await listing()
    } catch (err) {
      failedAt.set(hostKey, deps.now())
      logOnce(`list:${hostKey}`, `could not list agents on ${hostKey} for their pictures:`, err)
      return
    }
    const found: [string, string | null | undefined][] = []
    for (const row of rows) {
      colorTried.add(row.address)
      const ws = findWorkspaceForRow([...list], row)
      if (ws) found.push([row.address, ws.color])
    }
    noteColors(found)
    if (!images) return
    const done: string[] = []
    for (const row of rows) {
      // The server answered: a forced ask is spent, whatever comes of it.
      forced.delete(row.address)
      const ws = findWorkspaceForRow([...list], row)
      if (!ws) continue
      let image: string | null | undefined
      try {
        image = await imageFor(hostKey, ws, ask)
      } catch (err) {
        failedAt.set(hostKey, deps.now())
        logOnce(`icon:${row.address}`, `could not read the picture for ${row.address}:`, err)
        continue
      }
      // Connected server, no image in the store: nothing to say yet.
      if (image === undefined) continue
      pending.add(row.address)
      try {
        if (await put(row.address, image)) done.push(row.address)
      } finally {
        pending.delete(row.address)
      }
    }
    if (done.length > 0) await readCache(done)
  }

  const refresh = async (input: AvatarSyncInput): Promise<void> => {
    if (deps.isWeb()) return
    lastInput = input
    for (const r of input.rows) knownRows.set(r.address, r)
    const rows = input.rows.filter((r) => {
      const p = parseHomeAddress(r.address)
      return p !== null && p.host !== LOCAL_HOME_HOST
    })
    if (rows.length === 0) return
    // First row load: read what the cache already has before asking anyone.
    const unread = rows.map((r) => r.address).filter((a) => !read.has(a))
    if (unread.length > 0) await readCache(unread)
    if (rows.some((r) => !read.has(r.address))) return // the cache is unreadable: ask nobody
    const now = deps.now()
    const localFail = failedAt.get(LOCAL_HOME_HOST)
    if (localFail !== undefined && now - localFail < AVATAR_FAILURE_BACKOFF_MS) return

    // A server with at least one stale row is listed once, and that one
    // listing refreshes every row on it (T4.1). A server whose only due
    // rows are colors never seen is listed for colors alone.
    const staleHosts = new Set<string>()
    const colorHosts = new Set<string>()
    for (const r of rows) {
      if (pending.has(r.address)) continue
      const host = parseHomeAddress(r.address)!.host
      if (stale(r.address, now) || connectedHasNewImage(r, host, input)) staleHosts.add(host)
      else if (colorDue(r.address)) colorHosts.add(host)
    }
    const byHost = new Map<string, HomeRow[]>()
    for (const r of rows) {
      const host = parseHomeAddress(r.address)!.host
      if ((!staleHosts.has(host) && !colorHosts.has(host)) || pending.has(r.address)) continue
      byHost.set(host, [...(byHost.get(host) ?? []), r])
    }
    const jobs: Promise<void>[] = []
    for (const [host, hostRows] of byHost) {
      if (inflightHosts.has(host)) continue
      if (host === input.connectedKey) {
        const projects = input.connectedProjects
        if (!projects) continue
        // P12a: the connected server's images come from the store.
        const job = refreshHost(host, hostRows, async () => projects, false, staleHosts.has(host))
        jobs.push(job)
        continue
      }
      if (!input.reachable(host)) continue
      const fail = failedAt.get(host)
      if (fail !== undefined && now - fail < AVATAR_FAILURE_BACKOFF_MS) continue
      const job = refreshHost(
        host,
        hostRows,
        async () => {
          const raw = await deps.hostGet<unknown>(host, 'projects/list')
          return Array.isArray(raw) ? (raw as ListedAvatarWorkspace[]) : []
        },
        true,
        staleHosts.has(host),
      ).finally(() => inflightHosts.delete(host))
      inflightHosts.set(host, job)
      jobs.push(job)
    }
    await Promise.all(jobs)
  }

  /** The connected server's store has an image for a row the cache still
   *  calls missing: no network needed to fix that, so don't wait (P12a). */
  const connectedHasNewImage = (row: HomeRow, host: string, input: AvatarSyncInput): boolean => {
    if (host !== input.connectedKey || !input.connectedProjects) return false
    if (store.getState().cached[row.address]?.missing !== true) return false
    const ws = findWorkspaceForRow([...input.connectedProjects], row)
    return !!ws && isImageDataUrl(ws.iconUrl)
  }

  const recheck = async (addresses: readonly string[], why: 'user' | 'changed'): Promise<void> => {
    if (deps.isWeb()) return
    const cached = store.getState().cached
    const rows: HomeRow[] = []
    for (const a of addresses) {
      const p = parseHomeAddress(a)
      if (!p || p.host === LOCAL_HOME_HOST) continue
      if (why === 'user' && cached[a]?.missing !== true) continue
      forced.add(a)
      const row = knownRows.get(a)
      if (row) rows.push(row)
    }
    // Only the asked rows' servers are listed. No input yet (or a row no
    // caller has shown): the next refresh picks the forced rows up.
    if (rows.length > 0 && lastInput) await refresh({ ...lastInput, rows })
  }

  const recheckDue = async (input: AvatarSyncInput): Promise<void> => {
    if (deps.isWeb()) return
    const now = deps.now()
    const cached = store.getState().cached
    const due = input.rows
      .map((r) => r.address)
      .filter((a) => read.has(a) && cached[a]?.missing === true && !pending.has(a) && stale(a, now))
    if (due.length === 0) return
    await readCache(due)
    await refresh(input)
  }

  const readLocalRows = async (rows: readonly HomeRow[], connectedKey: string): Promise<void> => {
    if (deps.isWeb() || connectedKey === LOCAL_HOME_HOST) return
    const localRows = rows.filter((r) => parseHomeAddress(r.address)?.host === LOCAL_HOME_HOST)
    if (localRows.length === 0 || localInflight) return
    localInflight = (async () => {
      let list: ListedAvatarWorkspace[]
      try {
        const raw = await deps.hostGet<unknown>(LOCAL_HOME_HOST, 'projects/list')
        list = Array.isArray(raw) ? (raw as ListedAvatarWorkspace[]) : []
      } catch (err) {
        logOnce('local-list', 'could not list this computer’s agents for their pictures:', err)
        return
      }
      const next: Record<string, string | null> = {}
      const found: [string, string | null | undefined][] = []
      for (const row of localRows) {
        const ws = findWorkspaceForRow(list, row)
        if (!ws) continue
        found.push([row.address, ws.color])
        try {
          next[row.address] = (await imageFor(LOCAL_HOME_HOST, ws, true)) ?? null
        } catch (err) {
          logOnce(`local-icon:${row.address}`, `could not read the picture for ${row.address}:`, err)
        }
      }
      store.setState((s) => ({ local: { ...s.local, ...next } }))
      noteColors(found)
    })().finally(() => {
      localInflight = null
    })
    await localInflight
  }

  const putFromListing = async (hostKey: string, address: string, ws: ListedAvatarWorkspace): Promise<void> => {
    if (deps.isWeb() || hostKey === LOCAL_HOME_HOST) return
    colorTried.add(address)
    noteColors([[address, ws.color]])
    pending.add(address)
    try {
      const image = await imageFor(hostKey, ws, true)
      if (await put(address, image ?? null)) await readCache([address])
    } catch (err) {
      logOnce(`add:${address}`, `could not cache the picture for ${address} on add:`, err)
    } finally {
      pending.delete(address)
    }
  }

  const runPrune = async (): Promise<void> => {
    pruneTimer = null
    const keepFn = pruneKeep
    pruneKeep = null
    const keep = keepFn?.()
    if (!keep) return
    const kept = new Set(keep)
    const colors = store.getState().colors
    if (Object.keys(colors).some((a) => !kept.has(a))) {
      const next = Object.fromEntries(Object.entries(colors).filter(([a]) => kept.has(a)))
      store.setState({ colors: next })
      saveColors(next)
    }
    try {
      await deps.cachePrune(keep)
    } catch (err) {
      logOnce('prune', 'could not clean up the avatar cache:', err)
    }
  }

  const schedulePrune = (keep: () => string[] | null): void => {
    if (deps.isWeb()) return
    pruneKeep = keep
    if (pruneTimer !== null) clearTimeout(pruneTimer)
    pruneTimer = setTimeout(() => void runPrune(), AVATAR_PRUNE_DELAY_MS)
  }

  const flushPrune = async (): Promise<void> => {
    if (pruneTimer === null) return
    clearTimeout(pruneTimer)
    await runPrune()
  }

  const noteListColors = (
    hostKey: string,
    list: readonly ListedAvatarWorkspace[],
    rows: readonly HomeRow[],
  ): void => {
    if (deps.isWeb()) return
    const found: [string, string | null | undefined][] = []
    for (const row of rows) {
      if (parseHomeAddress(row.address)?.host !== hostKey) continue
      const ws = findWorkspaceForRow([...list], row)
      if (ws) found.push([row.address, ws.color])
    }
    noteColors(found)
  }

  return {
    readCache,
    refresh,
    readLocalRows,
    putFromListing,
    schedulePrune,
    flushPrune,
    recheck,
    recheckDue,
    noteListColors,
  }
}

// ── The app's instance ───────────────────────────────────────────────────

const appDeps: HomeAvatarDeps = {
  now: () => Date.now(),
  isWeb: isWebClient,
  cacheGet: async (addresses) => {
    const r = await daemonCliGet<{ avatars?: Record<string, CachedAvatar> }>(
      scopeForHost(LOCAL_HOME_HOST),
      'home/avatars',
      { addresses: addresses.join(',') },
    )
    return r && typeof r === 'object' && r.avatars && typeof r.avatars === 'object' ? r.avatars : {}
  },
  cachePut: async (address, dataUrl) => {
    await daemonCliPost(scopeForHost(LOCAL_HOME_HOST), 'home/avatars/put', { address, dataUrl })
  },
  cachePrune: async (keep) => {
    await daemonCliPost(scopeForHost(LOCAL_HOME_HOST), 'home/avatars/prune', { keep })
  },
  hostGet: (hostKey, route, params) => daemonCliGet(scopeForHost(hostKey), route, params),
  colorsLoad: () => globalThis.localStorage?.getItem(HOME_AGENT_COLORS_KEY) ?? null,
  colorsSave: (raw) => globalThis.localStorage?.setItem(HOME_AGENT_COLORS_KEY, raw),
}

export const homeAvatarSync: HomeAvatarSync = createHomeAvatarSync(appDeps)

/** P33, for the Add Agent picker: cache the image of a row it just added
 *  from a listing it already holds. Fire and forget; never blocks the add. */
export function putHomeAvatarOnAdd(hostKey: string, address: string, ws: ListedAvatarWorkspace): void {
  void homeAvatarSync.putFromListing(hostKey, address, ws)
}

/** Every address on every Home, or null when the store did not come from
 *  storage (P28: one storage error must never wipe the cache). */
export function pruneKeepFrom(state: Pick<HomesState, 'homes' | 'storageOk'>): string[] | null {
  if (!state.storageOk) return null
  const keep = new Set<string>()
  for (const h of state.homes) for (const r of h.rows) keep.add(r.address)
  return [...keep].sort()
}

/** The picture for a Home row from this computer's cache, or null (paint
 *  the letter) (P19). Not a hook: Zen's data layer reads it for its rows. */
export function homeRowAvatarFrom(
  address: string,
  connectedKey: string,
  state: Pick<HomeAvatarState, 'cached' | 'local'>,
): string | null {
  if (isWebClient()) return null
  const parsed = parseHomeAddress(address)
  if (!parsed) return null
  if (parsed.host === LOCAL_HOME_HOST) return connectedKey === LOCAL_HOME_HOST ? null : (state.local[address] ?? null)
  const cached = state.cached[address]?.dataUrl ?? null
  return isImageDataUrl(cached) ? cached : null
}

/** The picture for a Home row, or null (paint the letter) (P19). */
export function useHomeRowAvatar(address: string): string | null {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const entry = useHomeAvatarStore((s) => s.cached[address])
  const local = useHomeAvatarStore((s) => s.local[address])
  return homeRowAvatarFrom(address, connectedKey, {
    cached: entry ? { [address]: entry } : {},
    local: local === undefined ? {} : { [address]: local },
  })
}

/** The agent's own color for a Home row (validated `#rrggbb`), or null
 *  when this computer doesn't know it yet (paint the muted color). */
export function useHomeRowColor(address: string): string | null {
  return useHomeAvatarStore((s) => s.colors[address] ?? null)
}

/** Keeps the selected Home's pictures current and prunes the cache.
 *  Mounted in `HomeShellEffects`, so it runs only while Home is on screen. */
export function useHomeAvatarSync(sync: HomeAvatarSync = homeAvatarSync): void {
  const home = useHomesStore(selectedHome)
  useHomeAvatarRowSync(home.rows, sync)

  // P17 / P28: prune 2 s after any row change on any Home, and once now.
  useEffect(() => {
    const keep = (): string[] | null => pruneKeepFrom(useHomesStore.getState())
    sync.schedulePrune(keep)
    let last = (keep() ?? []).join('\n')
    return useHomesStore.subscribe((s) => {
      const next = (pruneKeepFrom(s) ?? []).join('\n')
      if (next === last) return
      last = next
      sync.schedulePrune(keep)
    })
  }, [sync])
}

/** Keeps the pictures of `rows` current: reads the shared cache on mount
 *  and window focus, fills in stale rows from their servers, and reads
 *  `local` rows live on a remote window. No pruning (`useHomeAvatarSync`
 *  owns that). Zen runs it for the Homes its Gardens show, which need not
 *  be the selected Home. */
export function useHomeAvatarRowSync(rows: readonly HomeRow[], sync: HomeAvatarSync = homeAvatarSync): void {
  const connectedKey = useConnectHostStore((s) => activeHomeHostKey(s.activeHost))
  const connected = useConnectHostStore((s) => s.connectionStatus === 'connected')
  const projects = useProjectsStore((s) => s.projects)
  const hostKeys = [...new Set(rows.map((r) => parseHomeAddress(r.address)?.host ?? ''))].filter(Boolean).sort()
  // Only reach/login TRANSITIONS re-run the sync, never each 30 s check.
  const reachKey = useStore(hostPool.store, (s) =>
    hostKeys.map((k) => `${k}:${s.entries[k]?.reach ?? '-'}:${s.entries[k]?.auth ?? '-'}`).join('|'),
  )
  const addressesKey = rows.map((r) => r.address).join('\n')
  const localRowsKey = rows
    .map((r) => r.address)
    .filter((a) => parseHomeAddress(a)?.host === LOCAL_HOME_HOST)
    .join('\n')
  const inputRef = useRef<AvatarSyncInput | null>(null)
  inputRef.current = {
    rows,
    connectedKey,
    connectedProjects: connected ? projects : null,
    reachable: (key) => {
      const e = hostPool.store.getState().entries[key]
      return !!e && e.reach === 'live' && e.auth === 'ok'
    },
  }

  // Home mount and window focus: re-read the shared cache (another window
  // may have written it), then fill in anything stale.
  useEffect(() => {
    const again = (): void => {
      const input = inputRef.current
      if (!input) return
      void sync
        .readCache(input.rows.map((r) => r.address))
        .then(() => sync.refresh(inputRef.current ?? input))
    }
    again()
    window.addEventListener('focus', again)
    // A miss lives 10 min (then longer): look for due ones while on screen,
    // so a focused window doesn't wait for the next focus to see a picture
    // a peer set. Fetching is the app's job by design (P10); a headless
    // daemon has no screen that needs these pictures.
    const tick = setInterval(() => {
      const input = inputRef.current
      if (input) void sync.recheckDue(input)
    }, AVATAR_RECHECK_TICK_MS)
    return () => {
      window.removeEventListener('focus', again)
      clearInterval(tick)
    }
  }, [sync])

  // A row added or moved, a server coming back, a login landing, the
  // connected server's list changing: fill in what's stale. The cache's
  // `fetchedAt` keeps this to once a day per row.
  const first = useRef(true)
  useEffect(() => {
    if (first.current) {
      first.current = false
      return
    }
    const input = inputRef.current
    if (!input) return
    void sync.refresh(input)
  }, [sync, addressesKey, reachKey, connectedKey, connected, projects])

  // P27: `local` rows on a remote window read this computer's daemon once
  // per Home mount, and again only when those rows or the window's server
  // change.
  useEffect(() => {
    const input = inputRef.current
    if (input && localRowsKey) void sync.readLocalRows(input.rows, input.connectedKey)
  }, [sync, localRowsKey, connectedKey])
}
