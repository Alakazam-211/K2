// Why a message from the say row didn't go (wire-shapes-k2-2026-11-15, "Say
// something"): the route's refusals in plain words, for the say row's line
// "Couldn’t send: <reason>. Your message is kept here." Pure.

import { failureReason } from '@/lib/editor/save-model/write'

export interface SayFailure {
  reason: string
  /** Rate limited: how long until a send can go. */
  retryAfterMs?: number
}

export function sayFailure(err: unknown, to: string, nameOf: (id: string) => string | null): SayFailure {
  const e = err as { status?: number; code?: string | null; body?: { retryAfterMs?: unknown } | null }
  if (e?.status === 429 || e?.code === 'rate_limited') {
    const ms = typeof e.body?.retryAfterMs === 'number' && e.body.retryAfterMs > 0 ? e.body.retryAfterMs : null
    const wait = ms === null ? 'in a few seconds' : `in ${Math.max(1, Math.ceil(ms / 1000))} s`
    return { reason: `you sent several at once; you can send again ${wait}`, ...(ms !== null ? { retryAfterMs: ms } : {}) }
  }
  if (e?.code === 'not_in_room') return { reason: 'you’re not in this file’s live session now; reopen the file' }
  if (e?.code === 'no_live_room') return { reason: 'this file has no live session to send in' }
  if (e?.status === 404) return { reason: `${nameOf(to) ?? 'they'} isn’t in this file now` }
  return { reason: failureReason(err) }
}

/** An address as words when this text can't name it (lib/editor/live/addr.ts resolves it first):
 *  `@h:getting-started` → "getting-started", `L12` → "line 12". */
export function addressLabel(address: string): string {
  if (address.startsWith('@h:')) return address.slice(3)
  const l = /^L([1-9]\d*)$/.exec(address)
  return l ? `line ${l[1]}` : address
}
