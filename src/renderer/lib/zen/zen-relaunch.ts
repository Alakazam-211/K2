// Relaunch a Garden's widgets after a daemon restart (Rosson 2026-11-05:
// "If K2 restarts while widgets are running in the Fleet Ops 2 Garden,
// relaunch the widgets after the restart is done").
//
// Zen stays mounted while its daemon restarts (ZenHost sits outside the
// ConnectionGate's keyed App, Z6), so nothing used to start the widgets
// again: the page was read only on a Garden switch or `zen_changed` (missed
// while the socket was down), so a frame kept running a bundle hash the new
// daemon refuses (`stale_widget` → Reload → "changed while its page was
// loading", for good); a store subscription kept its old cursor until the
// next push; and a widget that failed or timed out during the gap stayed on
// its stopped / load-failed card until a manual Reload.
//
// What this does, per server (`hostKey`):
//   - Restart = a new daemon instance id for that server: an app socket's
//     hello (`instance_id`) or the host pool's status check (`onRestart`).
//     The same id again is an ordinary socket blip: nothing happens (the
//     sockets' own reconnect handling covers it).
//   - Then wait until that server's `/boot-status` says ready (polled with
//     backoff), and relaunch ONCE: a second hello or a newer id while
//     waiting folds into the same relaunch.
//   - The server Zen reads its Gardens from (`zenLocalScope()`): re-read the
//     Garden list and the page (the `zen_changed` path), clear every stop on
//     the Garden, remount every custom frame (a frame whose hash changed was
//     already remounted by the new page: it is not remounted twice), and
//     restart the Thread feeds on that server.
//   - Any other server (a remote room the Garden reaches): remount only the
//     custom frames that reach it (agents / Thread caps over the Garden's
//     reach, or a bound data source on it) and restart its Thread feeds.
// A widget's own saves live in the daemon (store, saves): a remount reads
// them again, nothing is carried over from the old frame.
//
// Per-window view state only; the daemon owns the truth (daemon-first).

import { hostPool } from '@/lib/host-pool-instance'
import { canonicalHostKey } from '@/lib/host-key'
import { scopeForHost } from '@/kessel/server-scope'
import { primaryRoom } from '@/stores/room'
import { onAppHello } from '@/stores/session-events'
import { useZenConfigStore, zenLocalScope } from './zen-api'
import { relaunchZenWidgets, zenPlacementKey } from './zen-custom-run'
import type { ZenCustomWidgetPayload } from './zen-custom-types'
import { restartZenFeedsOn } from './zen-data'
import { ZEN_CUSTOM_KIND, type ZenResolvedPage } from './zen-page'
import { zenWidgetReachRows } from './zen-custom-scope'
import { parseSourceAddress } from './zen-sources'

/** A server's `/boot-status` as the watch needs it. */
export interface ZenBootAnswer {
  ready: boolean
  instanceId: string | null
}

export interface ZenRestartWatchDeps {
  /** `hostKey`'s `/boot-status` now, or null when it didn't answer. */
  boot(hostKey: string): Promise<ZenBootAnswer | null>
  /** `hostKey` restarted and is ready again. */
  relaunch(hostKey: string): void
  setTimer(fn: () => void, ms: number): unknown
  clearTimer(t: unknown): void
}

/** Waits between `/boot-status` checks while a restarted server boots. */
export const ZEN_RESTART_READY_DELAYS_MS: readonly number[] = [250, 500, 1_000, 2_000, 4_000, 8_000, 15_000]

export interface ZenRestartWatch {
  /** `hostKey` reports daemon instance `instanceId` (a hello, a status
   *  check). `prev` is the id the reporter held before (the host pool's
   *  restart event), used only when this watch has none for the server. */
  note(hostKey: string, instanceId: string | null | undefined, prev?: string | null): void
  /** Servers that restarted and are not ready yet. */
  waiting(): string[]
  dispose(): void
}

/** The restart rule (pure over its deps): one relaunch per new instance id,
 *  only once that server is ready. */
export function createZenRestartWatch(deps: ZenRestartWatchDeps): ZenRestartWatch {
  const seen = new Map<string, string>()
  const waits = new Map<string, { attempt: number; timer: unknown | null }>()
  let disposed = false

  const check = (hostKey: string): void => {
    const w = waits.get(hostKey)
    if (!w || disposed) return
    w.timer = null
    void deps
      .boot(hostKey)
      .catch((err: unknown) => {
        console.warn(`[zen] ${hostKey}: /boot-status after a restart failed:`, err)
        return null
      })
      .then((answer) => {
        if (disposed || waits.get(hostKey) !== w) return
        if (answer?.ready) {
          if (answer.instanceId) seen.set(hostKey, answer.instanceId)
          waits.delete(hostKey)
          deps.relaunch(hostKey)
          return
        }
        const delay = ZEN_RESTART_READY_DELAYS_MS[Math.min(w.attempt, ZEN_RESTART_READY_DELAYS_MS.length - 1)]
        w.attempt += 1
        w.timer = deps.setTimer(() => check(hostKey), delay)
      })
  }

  return {
    note(hostKey, instanceId, prev) {
      if (disposed || !instanceId) return
      const last = seen.get(hostKey) ?? (prev || undefined)
      seen.set(hostKey, instanceId)
      // The first id is the one the widgets run against; the same id again
      // is a socket blip, not a restart.
      if (last === undefined || last === instanceId) return
      // Already waiting for this server: the same restart (or a newer one
      // before it was ready) relaunches once.
      if (waits.has(hostKey)) return
      waits.set(hostKey, { attempt: 0, timer: null })
      check(hostKey)
    },
    waiting() {
      return [...waits.keys()]
    },
    dispose() {
      disposed = true
      for (const w of waits.values()) if (w.timer !== null) deps.clearTimer(w.timer)
      waits.clear()
    },
  }
}

/** Caps that put a widget on the Garden's agents (and so on their servers). */
function readsAgents(w: Pick<ZenCustomWidgetPayload, 'caps'>): boolean {
  return w.caps.some((c) => c === 'agents:read' || c.startsWith('thread:'))
}

/** The servers (host keys) one custom widget reaches besides the one Zen
 *  reads its Gardens from: the Garden's reach when it reads agents or
 *  Threads, and the server of each data source it binds. */
export function zenWidgetServers(
  w: Pick<ZenCustomWidgetPayload, 'caps' | 'sourceAddresses'>,
  reachServers: () => readonly string[] = () => zenWidgetReachRows().servers,
): Set<string> {
  const out = new Set<string>()
  if (readsAgents(w)) for (const s of reachServers()) out.add(s)
  for (const a of w.sourceAddresses ?? []) {
    const host = parseSourceAddress(a)?.host
    if (host) out.add(canonicalHostKey(host))
  }
  return out
}

function customPlacements(page: ZenResolvedPage | null): Array<{ id: string; custom: ZenCustomWidgetPayload }> {
  if (!page) return []
  const out: Array<{ id: string; custom: ZenCustomWidgetPayload }> = []
  for (const w of page.widgets) if (w.kind === ZEN_CUSTOM_KIND && w.custom) out.push({ id: w.id, custom: w.custom })
  return out
}

export interface ZenRelaunchOpts {
  /** The server Zen reads its Gardens from (`zenLocalScope().hostKey`). */
  zenHostKey: string
  /** Re-read the Garden list and the page (ZenRoot's `zen_changed` path). */
  reloadPage(): Promise<void>
  /** The servers a widget reaches (tests inject it). */
  serversOf?: (w: ZenCustomWidgetPayload) => Set<string>
}

/**
 * Relaunch what `hostKey`'s restart touched (see the file comment). Returns
 * the placement keys whose frames were relaunched.
 */
export async function relaunchZenAfterRestart(hostKey: string, opts: ZenRelaunchOpts): Promise<string[]> {
  const before = useZenConfigStore.getState()
  const zenHost = hostKey === opts.zenHostKey
  if (zenHost) await opts.reloadPage()
  const { gardenId, page } = useZenConfigStore.getState()
  const keys: string[] = []
  if (gardenId) {
    // A placement whose hash the new page changed was remounted by it (the
    // frame is keyed by hash): clear its stop, don't remount it again.
    const oldHash = new Map(
      before.gardenId === gardenId ? customPlacements(before.page).map((p) => [p.id, p.custom.hash] as const) : [],
    )
    const serversOf = opts.serversOf ?? ((w: ZenCustomWidgetPayload) => zenWidgetServers(w))
    const items: Array<{ key: string; remount: boolean }> = []
    for (const p of customPlacements(page)) {
      if (!zenHost && !serversOf(p.custom).has(hostKey)) continue
      const key = zenPlacementKey(gardenId, p.id)
      keys.push(key)
      items.push({ key, remount: !oldHash.has(p.id) || oldHash.get(p.id) === p.custom.hash })
    }
    // Zen's own server: every stop on the Garden goes (a placement the page
    // no longer lists, or a page still loading after safe mode).
    relaunchZenWidgets(items, zenHost ? { clearGarden: gardenId } : {})
  }
  const feeds = restartZenFeedsOn(hostKey)
  console.info(`[zen] ${hostKey} restarted: relaunched ${keys.length} widget(s), ${feeds} Thread feed(s)`)
  return keys
}

/** ZenRoot, while Zen shows: watch Zen's server, the window's server and
 *  every server the page's widgets reach, and relaunch on a restart.
 *  Returns the uninstall. */
export function installZenRelaunchOnRestart(opts: { reloadPage(): Promise<void> }): () => void {
  const zenScope = zenLocalScope()
  const watch = createZenRestartWatch({
    boot: async (hostKey) => {
      const e = await hostPool.check(hostKey, { bootOnly: true })
      return e.boot ? { ready: e.boot.ready, instanceId: e.boot.instanceId } : null
    },
    relaunch: (hostKey) => {
      void relaunchZenAfterRestart(hostKey, { zenHostKey: zenScope.hostKey, reloadPage: opts.reloadPage }).catch((err: unknown) =>
        console.error(`[zen] relaunching widgets after ${hostKey} restarted failed:`, err),
      )
    },
    setTimer: (fn, ms) => setTimeout(fn, ms),
    clearTimer: (t) => clearTimeout(t as ReturnType<typeof setTimeout>),
  })
  const offs: Array<() => void> = [
    onAppHello(zenScope, (h) => watch.note(zenScope.hostKey, h.instance_id)),
    // The window's server (Zen never resolves it for its config: Z8).
    onAppHello(primaryRoom().scope, (h) => watch.note(primaryRoom().scope.hostKey, h.instance_id)),
    hostPool.onRestart((hostKey, prev, next) => watch.note(hostKey, next, prev)),
  ]
  // Remote rooms' own sockets (a room open on that server) say hello too.
  const remote = new Map<string, () => void>()
  const syncRemote = (): void => {
    const want = new Set<string>()
    for (const p of customPlacements(useZenConfigStore.getState().page)) {
      for (const s of zenWidgetServers(p.custom)) if (s !== zenScope.hostKey) want.add(s)
    }
    for (const [key, off] of remote) {
      if (want.has(key)) continue
      off()
      remote.delete(key)
    }
    for (const key of want) {
      if (remote.has(key)) continue
      remote.set(key, onAppHello(scopeForHost(key), (h) => watch.note(key, h.instance_id)))
    }
  }
  syncRemote()
  const offPage = useZenConfigStore.subscribe((s, prev) => {
    if (s.page !== prev.page) syncRemote()
  })
  return () => {
    offPage()
    for (const off of offs) off()
    for (const off of remote.values()) off()
    remote.clear()
    watch.dispose()
  }
}
