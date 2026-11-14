// When the catch-up runs, and for which Threads (speed pass P3b).
//
// On launch and whenever the window comes back to the front (focus, the
// tab visible again), every server with Threads to catch up gets ONE
// `POST /cli/thread/catchup` (`chatCatchup.ts`):
//   - every Thread with a copy in the view cache (this window's memory and
//     this computer's IndexedDB), asking from its newest server seq;
//   - the light prefetch: the top Home rows and the Home rows with unread
//     (`homePrefetchTargets`), asking the newest page when not cached.
// One run at a time per window (a burst of focus events costs one run),
// and not more often than CATCHUP_MIN_GAP_MS. A server without
// `inbox-catchup-v1` (an older daemon) is skipped: its Threads re-sync
// when opened, as before. Never throws.

import { scopeForHost, type ServerScope } from '@/kessel/server-scope'
import { homePrefetchTargets } from '@/lib/home-thread-prefetch'
import { catchUpServer, type CatchupRunResult } from './chatCatchup'
import { catchupSupported } from './threadCatchup'
import { listThreadViews } from './threadViewCache'

/** Runs at most this often (a focus burst, quick window switches). */
export const CATCHUP_MIN_GAP_MS = 15_000

export interface ChatCatchupTarget {
  hostKey: string
  addr: string
}

export interface ChatCatchupDeps {
  targets?: () => Promise<ChatCatchupTarget[]>
  scopeFor?: (hostKey: string) => ServerScope
  catchUp?: (scope: ServerScope, addrs: readonly string[]) => Promise<CatchupRunResult>
  now?: () => number
}

/** Cached Threads first (newest save first), then the prefetch targets. */
async function defaultTargets(): Promise<ChatCatchupTarget[]> {
  const cached = (await listThreadViews()).map((m) => ({ hostKey: m.hostKey, addr: m.addr }))
  return [...cached, ...homePrefetchTargets()]
}

let running: Promise<void> | null = null
let lastRunAt = Number.NEGATIVE_INFINITY

/** One catch-up of every server. Joins a run already going; skipped when
 *  the last one started less than CATCHUP_MIN_GAP_MS ago (`force` skips
 *  the gap). */
export function runChatCatchup(deps: ChatCatchupDeps = {}, force = false): Promise<void> {
  if (running) return running
  const now = deps.now ?? Date.now
  if (!force && now() - lastRunAt < CATCHUP_MIN_GAP_MS) return Promise.resolve()
  lastRunAt = now()
  const p = (async () => {
    const byHost = new Map<string, string[]>()
    for (const t of await (deps.targets ?? defaultTargets)()) {
      const list = byHost.get(t.hostKey) ?? []
      if (!list.includes(t.addr)) list.push(t.addr)
      byHost.set(t.hostKey, list)
    }
    const scopeFor = deps.scopeFor ?? ((h: string) => scopeForHost(h))
    const catchUp = deps.catchUp ?? ((s: ServerScope, a: readonly string[]) => catchUpServer(s, a))
    await Promise.all(
      [...byHost].map(async ([hostKey, addrs]) => {
        let scope: ServerScope
        try {
          scope = scopeFor(hostKey)
        } catch {
          return // the server was removed from this computer
        }
        if (!catchupSupported(scope)) return
        try {
          await catchUp(scope, addrs)
        } catch (e) {
          console.debug('[thread] catch-up failed:', hostKey, e)
        }
      }),
    )
  })().finally(() => {
    running = null
  })
  running = p
  return p
}

/** Run on launch and when the window comes back to the front. Returns the
 *  uninstall. */
export function startChatCatchup(deps: ChatCatchupDeps = {}): () => void {
  void runChatCatchup(deps, true)
  if (typeof window === 'undefined') return () => {}
  const onFocus = (): void => void runChatCatchup(deps)
  const onVisible = (): void => {
    if (document.visibilityState === 'visible') void runChatCatchup(deps)
  }
  window.addEventListener('focus', onFocus)
  document.addEventListener('visibilitychange', onVisible)
  return () => {
    window.removeEventListener('focus', onFocus)
    document.removeEventListener('visibilitychange', onVisible)
  }
}

/** Tests only. */
export function resetChatCatchupRunForTests(): void {
  running = null
  lastRunAt = Number.NEGATIVE_INFINITY
}
