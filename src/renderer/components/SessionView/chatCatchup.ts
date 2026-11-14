// Catch-up (K2 0.46.3 `inbox-catchup-v1`): one `POST /cli/thread/catchup`
// per server brings every cached Thread up to date, instead of one read per
// Thread, so by the time a Thread is opened its copy is usually current.
//
// The logic is K2 Companion's `src/zen/chatCatchup.ts` (MIT, Alakazam
// Labs), adapted onto the desktop's view cache (`threadViewCache.ts`) and
// `daemonCliPost` scopes:
//   - a cached Thread asks `afterSeq` (its newest server seq: pushed rows
//     never count, `threadCacheMerge.newestSeq`); an uncached prefetch
//     target (a top Home row, an unread one) asks `newest`;
//   - each served entry is what the per-Thread read would have returned,
//     so it merges the same way (`applyThreadDelta`) into the cache;
//   - deferred entries and deltas with more to come ask again, up to
//     CATCHUP_MAX_ROUNDS calls (what came in by then lands: the copy goes
//     on from there next time); a Thread the call can't settle (it moved,
//     was renamed or reset, is another conversation now, or its access is
//     gone) loses its copy, so opening it reads the newest page;
//   - a Thread open in this window is left to its own view (`markThreadOpen`).
// Project chats and the `catchup-unchanged-v1` short-cut stay on the phone.

import type { ServerScope } from '@/kessel/server-scope'
import { catchupBody, postCatchup, type CatchupResult, type CatchupThreadAsk } from './threadCatchup'
import { applyThreadDelta, goneStatus } from './threadDelta'
import { newestSeq } from './threadCacheMerge'
import { forgetThreadView, peekThreadView, readThreadView, saveThreadView, type CachedThreadView } from './threadViewCache'
import type { OverlayThreadItem } from './overlayThread'

/** Calls per catch-up (deferred entries and long deltas ask again). */
export const CATCHUP_MAX_ROUNDS = 4

// ── Threads open in this window ────────────────────────────────────────────

const openThreads = new Map<string, number>()

function openKey(hostKey: string, addr: string): string {
  return `${hostKey}|${addr.trim()}`
}

/** A view shows this Thread: the catch-up leaves it alone until the
 *  returned function is called. */
export function markThreadOpen(hostKey: string, addr: string): () => void {
  const key = openKey(hostKey, addr)
  openThreads.set(key, (openThreads.get(key) ?? 0) + 1)
  let done = false
  return () => {
    if (done) return
    done = true
    const n = (openThreads.get(key) ?? 1) - 1
    if (n <= 0) openThreads.delete(key)
    else openThreads.set(key, n)
  }
}

export function isThreadOpen(hostKey: string, addr: string): boolean {
  return openThreads.has(openKey(hostKey, addr))
}

// ── One server ─────────────────────────────────────────────────────────────

interface Job {
  addr: string
  base: CachedThreadView | null
  ask: CatchupThreadAsk
  steps: Array<{ items: OverlayThreadItem[]; changed: OverlayThreadItem[]; removed: Array<{ id: string }> }>
  /** Newest page (a `newest` ask). */
  page: OverlayThreadItem[] | null
  hasMore: boolean
  conversationId: string
  done: boolean
  /** The copy can't be caught up: forget it (the open reads the page). */
  drop: boolean
}

export interface CatchupRunResult {
  /** Threads asked about. */
  asked: number
  /** Threads whose copy was written. */
  saved: string[]
  /** Threads whose copy was forgotten. */
  dropped: string[]
}

export interface CatchupServerDeps {
  post?: (scope: ServerScope, body: ReturnType<typeof catchupBody>) => Promise<CatchupResult>
  now?: () => number
}

/**
 * Catch up `addrs` (Threads of one server) and write what came back into
 * the view cache. Never throws.
 */
export async function catchUpServer(
  scope: ServerScope,
  addrs: readonly string[],
  deps: CatchupServerDeps = {},
): Promise<CatchupRunResult> {
  const post = deps.post ?? ((s, b) => postCatchup(s, b))
  const now = deps.now ?? Date.now
  const host = scope.hostKey
  const jobs: Job[] = []
  const seen = new Set<string>()
  for (const raw of addrs) {
    const addr = raw.trim()
    if (!addr || seen.has(addr) || isThreadOpen(host, addr)) continue
    seen.add(addr)
    const base = peekThreadView(host, addr) ?? (await readThreadView(host, addr))
    const cursor = base && base.conversationId ? newestSeq(base.items) : null
    jobs.push({
      addr,
      base: cursor === null ? null : base,
      ask: cursor === null ? { addr, newest: true } : { addr, afterSeq: cursor },
      steps: [],
      page: null,
      hasMore: base?.hasMore ?? false,
      conversationId: base?.conversationId ?? '',
      done: false,
      drop: false,
    })
  }

  for (let round = 0; round < CATCHUP_MAX_ROUNDS; round++) {
    const live = jobs.filter((j) => !j.done)
    if (!live.length) break
    const body = catchupBody(live.map((j) => j.ask))
    // Over the cap: what did not fit waits for the next round.
    const sent = body.threads.map((a) => live.find((j) => j.ask.addr === a.addr) as Job)
    const r = await post(scope, body)
    if (!r.ok) break
    let progress = false
    r.data.threads.forEach((e, i) => {
      const j = sent[i]
      if (!j || e.kind === 'deferred') return
      progress = true
      if (e.kind === 'error') {
        j.done = true
        j.drop = goneStatus(e.error.status)
        return
      }
      if (e.movedFrom || e.resync) {
        j.done = true
        j.drop = true
        return
      }
      const ask = j.ask
      if ('newest' in ask) {
        j.page = e.items
        j.hasMore = e.hasMore
        j.conversationId = e.conversationId ?? ''
        j.done = true
        return
      }
      if (e.conversationId && j.conversationId && e.conversationId !== j.conversationId) {
        // The conversation moved (seqs renumbered): the copy is another one.
        j.steps = []
        j.done = true
        j.drop = true
        return
      }
      j.steps.push({ items: e.items, changed: e.changed, removed: e.removed })
      const last = e.items.length ? e.items[e.items.length - 1].seq : null
      if (e.hasMore && typeof last === 'number' && last > ask.afterSeq) {
        j.ask = { addr: ask.addr, afterSeq: last }
      } else {
        j.done = true
        // More to come but nothing past the cursor: it can't go on.
        if (e.hasMore) {
          j.steps = []
          j.drop = true
        }
      }
    })
    if (!progress) break
  }

  const saved: string[] = []
  const dropped: string[] = []
  for (const j of jobs) {
    if (isThreadOpen(host, j.addr)) continue
    if (j.drop) {
      if (j.base) {
        dropped.push(j.addr)
        await forgetThreadView(host, j.addr)
      }
      continue
    }
    // Unsettled after the rounds (still more to come, or never served):
    // what did come lands; the next catch-up, or the open, goes on from it.
    if (j.page) {
      saveThreadView(host, j.addr, j.conversationId, j.page, now(), { hasMore: j.hasMore })
      saved.push(j.addr)
      continue
    }
    if (!j.base || !j.steps.length) continue
    const items = j.steps.reduce((acc, s) => applyThreadDelta(acc, s, j.hasMore), j.base.items)
    if (items === j.base.items) continue
    saveThreadView(host, j.addr, j.conversationId, items, now(), { hasMore: j.hasMore })
    saved.push(j.addr)
  }
  return { asked: jobs.length, saved, dropped }
}

/** Tests only. */
export function resetChatCatchupForTests(): void {
  openThreads.clear()
}
