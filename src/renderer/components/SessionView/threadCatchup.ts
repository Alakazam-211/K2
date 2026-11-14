// `POST /cli/thread/catchup` (K2 0.46.3 `inbox-catchup-v1`): many Threads'
// "what's new" in one call.
//
// The parse half is ported from K2 Companion's `src/api/threadCatchup.ts`
// (MIT, Alakazam Labs); the call goes through the desktop's
// `daemonCliPost` and its scopes (this computer, or another server over
// Connect with the saved login) instead of the phone's `scopedFetch`.
// Project chats (`groups`) stay on the phone: the desktop does not cache
// them.
//
// Each served Thread entry is exactly the plain `GET /cli/thread` body for
// that chat (a delta for `afterSeq`, the newest page for `newest`), minus
// `ok`. Entries come back in request order (after de-duplication) and are
// matched by position. A failing chat is an error on its entry only; the
// 4 MiB byte budget cuts one entry (`has_more`) and defers the rest
// (`deferred`, top-level `has_more`). One call in flight per token: a
// second is 429 `{retryAfterMs}`.

import { daemonCliPost } from '@/lib/daemon-cli'
import { DaemonHttpError } from '@/lib/daemon-http-error'
import type { ServerScope } from '@/kessel/server-scope'
import { THREAD_CARDS_CONTRACT, PERMISSION_CARDS_FEATURE, threadItemsFromSnapshot, type OverlayThreadItem } from './overlayThread'

export const FEATURE_INBOX_CATCHUP = 'inbox-catchup-v1' as const
export const CATCHUP_MAX_THREADS = 200
export const CATCHUP_LIMIT = 50
/** 429 retries before giving up on this round. */
const MAX_RATE_RETRIES = 2

export function catchupSupported(scope: Pick<ServerScope, 'serverSupports'>): boolean {
  return scope.serverSupports(FEATURE_INBOX_CATCHUP)
}

export type CatchupThreadAsk = { addr: string; afterSeq: number } | { addr: string; newest: true }

export interface CatchupError {
  status: number | null
  code: string
  hint: string
}

export type CatchupThreadEntry =
  | {
      kind: 'served'
      /** The address the chat answers at now (after a rename, `movedFrom`
       *  holds what was sent). */
      addr: string
      movedFrom: string | null
      conversationId: string | null
      /** Present on an `afterSeq` (delta) answer. */
      afterSeq: number | null
      items: OverlayThreadItem[]
      changed: OverlayThreadItem[]
      removed: { id: string }[]
      /** Delta: newer remain. Newest page: older exist. */
      hasMore: boolean
      /** Reserved: drop the copy, read the newest page. */
      resync: boolean
    }
  | { kind: 'error'; addr: string; error: CatchupError }
  | { kind: 'deferred'; addr: string }

export interface CatchupAnswer {
  hasMore: boolean
  threads: CatchupThreadEntry[]
}

export type CatchupResult = { ok: true; data: CatchupAnswer } | { ok: false; error: CatchupError }

function str(v: unknown): string {
  return typeof v === 'string' ? v.trim() : ''
}

function arr<T>(v: unknown): T[] {
  return Array.isArray(v) ? (v as T[]) : []
}

function removedOf(v: unknown): { id: string }[] {
  return arr<unknown>(v)
    .map((r) => (r && typeof r === 'object' ? str((r as Record<string, unknown>).id) : ''))
    .filter(Boolean)
    .map((id) => ({ id }))
}

function errorOf(o: Record<string, unknown>): CatchupError {
  const e = o.error
  const status = typeof o.status === 'number' ? o.status : null
  if (e && typeof e === 'object') {
    const x = e as Record<string, unknown>
    return { status, code: str(x.code), hint: str(x.hint) || str(x.code) }
  }
  return { status, code: str(e), hint: str(e) }
}

/** Thread items as the hook keeps them (the plain read's coercion). */
function items(v: unknown): OverlayThreadItem[] {
  return threadItemsFromSnapshot({ items: arr<unknown>(v) }).items
}

/** One `threads[]` entry; `sent` is what was asked at that position. */
export function parseThreadEntry(raw: unknown, sent: CatchupThreadAsk): CatchupThreadEntry {
  // Missing (a shorter answer than asked): ask again next round.
  if (raw == null || typeof raw !== 'object') return { kind: 'deferred', addr: sent.addr }
  const o = raw as Record<string, unknown>
  const addr = str(o.addr) || sent.addr
  if (o.deferred === true) return { kind: 'deferred', addr: sent.addr }
  if (o.error != null || (typeof o.status === 'number' && o.status >= 400)) {
    return { kind: 'error', addr: sent.addr, error: errorOf(o) }
  }
  return {
    kind: 'served',
    addr,
    movedFrom: str(o.movedFrom) || null,
    conversationId: str(o.conversation_id) || null,
    afterSeq: typeof o.after_seq === 'number' ? o.after_seq : null,
    items: items(o.items),
    changed: items(o.changed),
    removed: removedOf(o.removed),
    hasMore: o.has_more === true,
    resync: o.resync === true,
  }
}

/** The asks as the route takes them: one per address (the first wins), at
 *  most the cap, a bad cursor left out. */
export function catchupBody(
  threads: readonly CatchupThreadAsk[],
  limit = CATCHUP_LIMIT,
): { threads: CatchupThreadAsk[]; limit: number } {
  const seen = new Set<string>()
  const t: CatchupThreadAsk[] = []
  for (const a of threads) {
    if (!a.addr || seen.has(a.addr) || t.length >= CATCHUP_MAX_THREADS) continue
    if ('afterSeq' in a && !(Number.isInteger(a.afterSeq) && a.afterSeq >= 0)) continue
    seen.add(a.addr)
    t.push('afterSeq' in a ? { addr: a.addr, afterSeq: a.afterSeq } : { addr: a.addr, newest: true })
  }
  return { threads: t, limit }
}

/** A whole answer, matched to the body it answered. */
export function parseCatchupAnswer(raw: unknown, body: ReturnType<typeof catchupBody>): CatchupAnswer {
  const j = raw && typeof raw === 'object' ? (raw as Record<string, unknown>) : {}
  const rawT = arr<unknown>(j.threads)
  return {
    hasMore: j.has_more === true,
    threads: body.threads.map((sent, i) => parseThreadEntry(rawT[i], sent)),
  }
}

export interface CatchupDeps {
  post?: (scope: ServerScope, route: string, body: unknown) => Promise<unknown>
  sleep?: (ms: number) => Promise<void>
}

/** One catch-up call (the body from `catchupBody`). A 429 waits its
 *  `retryAfterMs` and tries again (twice at most). Never throws. */
export async function postCatchup(
  scope: ServerScope,
  body: ReturnType<typeof catchupBody>,
  deps: CatchupDeps = {},
): Promise<CatchupResult> {
  const post = deps.post ?? ((s: ServerScope, route: string, b: unknown) => daemonCliPost<unknown>(s, route, b))
  const sleep = deps.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)))
  const payload = {
    threads: body.threads,
    limit: body.limit,
    // Permission cards: the `cards=2` view in every entry, as the live
    // Thread reads it.
    ...(scope.serverSupports(PERMISSION_CARDS_FEATURE) ? { cards: THREAD_CARDS_CONTRACT } : {}),
  }
  for (let attempt = 0; ; attempt++) {
    try {
      const raw = await post(scope, 'thread/catchup', payload)
      return { ok: true, data: parseCatchupAnswer(raw, body) }
    } catch (e) {
      if (e instanceof DaemonHttpError) {
        if (e.status === 429 && attempt < MAX_RATE_RETRIES) {
          const b = e.body && typeof e.body === 'object' ? (e.body as Record<string, unknown>) : {}
          const wait = typeof b.retryAfterMs === 'number' && b.retryAfterMs > 0 ? Math.min(b.retryAfterMs, 10_000) : 1000
          await sleep(wait)
          continue
        }
        return { ok: false, error: { status: e.status, code: e.code ?? `http_${e.status}`, hint: e.message } }
      }
      const msg = e instanceof Error ? e.message : String(e)
      return { ok: false, error: { status: null, code: 'network', hint: msg } }
    }
  }
}
