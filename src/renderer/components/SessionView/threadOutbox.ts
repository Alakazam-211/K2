// This window's Thread sends that the server hasn't confirmed yet
// (research-remote-thread-load-v1 Fix 3, the "sending" bubble).
//
// Enter shows the message at once as a pending bubble, keyed by a
// `clientMsgId` (32 hex, made here). The post carries that id when the
// server lists `post-idempotency-v1` (crates/k2-daemon/src/client_msg_id.rs):
// a retry of the same id never posts twice, and a repeat answers the stored
// post with `duplicate: true` (sent). A key already used in another chat is
// a 409 `client_msg_id_reused`: the bubble fails and is never re-keyed.
//
// The bubble goes away when the server's copy shows (its answer, a socket
// frame or a catch-up read, whichever comes first): matched by
// `doc.clientMsgId`; on a server without the feature, by text and time
// (best effort; the server's answer still settles it). A transport failure
// (or a 5xx) keeps it as "Not sent" with Retry (the SAME id) and Discard.
//
// View state only: nothing here is canonical, nothing is persisted (a
// reload drops unsent bubbles; the draft path covers refusals).

import { useMemo, useSyncExternalStore } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayThreadItem } from './overlayThread'

/** The `/boot-status` feature that takes `clientMsgId` on posts. */
export const POST_IDEMPOTENCY_FEATURE = 'post-idempotency-v1'

export type OutboxState = 'sending' | 'failed'

export interface OutboxEntry {
  clientMsgId: string
  /** `<connectionKey>|<addr>`: the Thread it was sent to. */
  key: string
  scope: ServerScope
  addr: string
  /** The words as posted (the bubble's text). */
  text: string
  /** The POST body, `clientMsgId` included when the server takes it; a
   *  retry sends it unchanged. */
  body: Record<string, unknown>
  /** The server takes the id (else the match is by text and time). */
  keyed: boolean
  /** Send time, unix seconds. */
  createdAtSec: number
  state: OutboxState
  error?: string
}

/** On a display item: this is an unconfirmed send. */
export interface PendingMark {
  clientMsgId: string
  state: OutboxState
  error?: string
}

const entries = new Map<string, OutboxEntry>()
const listeners = new Set<() => void>()
/** Server ids that replaced a pending bubble (drawn without the arrival
 *  animation: the bubble was already on screen). Bounded. */
const replacedIds: string[] = []
let version = 0

function changed(): void {
  version += 1
  for (const fn of [...listeners]) fn()
}

/** 32 lowercase hex characters (128 random bits). */
export function newClientMsgId(): string {
  const bytes = new Uint8Array(16)
  const c = (globalThis as { crypto?: Crypto }).crypto
  if (c?.getRandomValues) c.getRandomValues(bytes)
  else for (let i = 0; i < bytes.length; i++) bytes[i] = Math.floor(Math.random() * 256)
  return Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('')
}

export function outboxKey(scope: Pick<ServerScope, 'connectionKey'>, addr: string): string {
  return `${scope.connectionKey}|${addr.trim()}`
}

export function outboxAdd(entry: OutboxEntry): void {
  entries.set(entry.clientMsgId, entry)
  changed()
}

export function outboxGet(clientMsgId: string): OutboxEntry | null {
  return entries.get(clientMsgId) ?? null
}

export function outboxMark(clientMsgId: string, state: OutboxState, error?: string): void {
  const e = entries.get(clientMsgId)
  if (!e) return
  entries.set(clientMsgId, { ...e, state, ...(error !== undefined ? { error } : { error: undefined }) })
  changed()
}

/** The server confirmed it (its answer or its copy showed). */
export function outboxSettle(clientMsgId: string, serverId?: string | null): void {
  if (serverId) {
    replacedIds.push(serverId)
    if (replacedIds.length > 100) replacedIds.splice(0, replacedIds.length - 100)
  }
  if (!entries.delete(clientMsgId)) return
  changed()
}

/** Drop it (a refusal that returns the draft, or the person's Discard). */
export function outboxDrop(clientMsgId: string): void {
  if (!entries.delete(clientMsgId)) return
  changed()
}

/** Was this server id a pending bubble a moment ago? */
export function wasPendingBubble(id: string): boolean {
  return replacedIds.includes(id)
}

export function outboxEntriesFor(key: string): OutboxEntry[] {
  return [...entries.values()].filter((e) => e.key === key)
}

function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

const getVersion = (): number => version

/** The Thread's unsent bubbles, kept current (one array per change). */
export function useOutbox(key: string): OutboxEntry[] {
  const v = useSyncExternalStore(subscribe, getVersion, getVersion)
  // eslint-disable-next-line react-hooks/exhaustive-deps
  return useMemo(() => outboxEntriesFor(key), [key, v])
}

/** A server item is this entry's post. */
function matches(entry: OutboxEntry, item: OverlayThreadItem, baseline: ReadonlySet<string> | undefined): boolean {
  const cid = item.doc.clientMsgId
  if (cid) return cid === entry.clientMsgId
  if (entry.keyed) return false
  // No id on either side (a server without the feature): a person's post
  // with the same words, new since the send, sent around that time.
  if (baseline?.has(item.id)) return false
  if (item.doc.via !== 'compose') return false
  if ((item.doc.body ?? '') !== entry.text) return false
  const at = item.doc.created_at
  return typeof at !== 'number' || Math.abs(at - entry.createdAtSec) <= 120
}

/**
 * Split `entries` against what the list holds: the ones the server's copy
 * already answers (`matched`), and the bubbles still to draw (`pending`,
 * as display items, oldest first). Each server item answers one entry.
 * `baselines` holds, per entry, the ids the list had when it first saw the
 * entry (so an older post with the same words never answers it).
 */
export function reconcileOutbox(
  outbox: readonly OutboxEntry[],
  items: readonly OverlayThreadItem[],
  baselines: ReadonlyMap<string, ReadonlySet<string>> = new Map(),
): { matched: Array<{ entry: OutboxEntry; id: string }>; pending: OverlayThreadItem[] } {
  const used = new Set<string>()
  const matched: Array<{ entry: OutboxEntry; id: string }> = []
  const pending: OverlayThreadItem[] = []
  const sorted = [...outbox].sort((a, b) => a.createdAtSec - b.createdAtSec)
  for (const e of sorted) {
    const hit = items.find((it) => !used.has(it.id) && matches(e, it, baselines.get(e.clientMsgId)))
    if (hit) {
      used.add(hit.id)
      matched.push({ entry: e, id: hit.id })
      continue
    }
    pending.push(pendingItem(e))
  }
  return { matched, pending }
}

/** The bubble for an entry: the person's own post, marked pending. */
export function pendingItem(e: OutboxEntry): OverlayThreadItem {
  const id = `pending:${e.clientMsgId}`
  return {
    collection: 'thread',
    // After everything the server has; ids keep pending ones apart.
    seq: Number.MAX_SAFE_INTEGER,
    id,
    localTime: true,
    pending: { clientMsgId: e.clientMsgId, state: e.state, ...(e.error ? { error: e.error } : {}) },
    doc: {
      id,
      kind: 'text',
      from: '',
      body: e.text,
      via: 'compose',
      created_at: e.createdAtSec,
      ...(e.keyed ? { clientMsgId: e.clientMsgId } : {}),
    },
  }
}

/** Tests: forget everything. */
export function resetOutboxForTests(): void {
  entries.clear()
  replacedIds.length = 0
  changed()
}
