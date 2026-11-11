// This computer's Thread sends that the server hasn't confirmed yet: the
// outbox (research-remote-thread-load-v1 Fix 3, the "sending" bubble; made
// durable by thread-send-resilience 2026-11-11, after a send typed on wake
// from sleep was lost on a remote Home room).
//
// Enter puts the message here FIRST, keyed by a `clientMsgId` (32 hex, made
// here), and saves it on this computer (IndexedDB, the Thread view cache's
// database, `threadViewCache.ts`). It shows in the Thread at once as the
// person's own bubble, at the bottom, until the server confirms it:
//
//   - "Sending…": on its way.
//   - "Waiting to send…": the server was out of reach (no answer, a time
//     out, a 5xx/408/429, the host recovering). It goes again by itself,
//     with backoff, and at once on a reconnect (the Thread's socket opening,
//     this computer's daemon coming back), on window focus and on `online`.
//   - "Not sent" with Retry / Delete: a refusal on a later try, a reused id
//     (409 `client_msg_id_reused`), another login, or still out of reach
//     after OUTBOX_AUTO_RETRY_MAX_AGE_SEC (the daemon keeps an id ≥ 24 h;
//     past that a retry could post twice, so the person decides).
//
// The post always carries the id; a server with `post-idempotency-v1`
// (crates/k2-daemon/src/client_msg_id.rs) never posts the same id twice and
// answers a repeat with `duplicate: true` (sent). An older server ignores
// the field. A message leaves the outbox only when the server's copy shows
// (its answer, a socket frame or a catch-up read, matched by
// `doc.clientMsgId`; on an older server by text, newer than the Thread was
// at send time), or when the person deletes it. A catch-up never drops
// one: the bubbles are drawn after whatever the list holds.
//
// Sends to one Thread go oldest first: a send made while older ones wait
// waits behind them.
//
// What is saved has no scope (a scope is a live object): an entry loaded
// after a restart waits until a view of its Thread binds one
// (`outboxBind`, from `useOverlayThread`), then goes.

import { useMemo, useSyncExternalStore } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { LOCAL_HOME_HOST } from '@/lib/host-key'
import { onDaemonConnected } from '@/lib/daemon-reconnect'
// A cycle (overlayThread imports this module): only used at call time.
import { sendOutboxEntry, type OverlayThreadItem } from './overlayThread'
import { deletePendingSend, readPendingSends, savePendingSend, type PendingSendRow } from './threadViewCache'

/** The `/boot-status` feature that takes `clientMsgId` on posts. */
export const POST_IDEMPOTENCY_FEATURE = 'post-idempotency-v1'

/** `sending`: on its way. `waiting`: out of reach, goes again by itself.
 *  `failed`: "Not sent" (Retry / Delete). */
export type OutboxState = 'sending' | 'waiting' | 'failed'

/** Saved rows of another version are ignored. */
export const OUTBOX_ROW_VERSION = 1

/** Automatic retries stop after this (the daemon keeps an id at least 24 h). */
export const OUTBOX_AUTO_RETRY_MAX_AGE_SEC = 20 * 3600

const BACKOFF_CAP_MS = 60_000
let backoffBaseMs = 2_000
let autoTimers = true

export interface OutboxEntry {
  clientMsgId: string
  /** `<hostKey>|<addr>`: the Thread it was sent to. */
  key: string
  /** The server it was written for (canonical host key). */
  hostKey: string
  /** How to reach that server now (memory only; null until a view of the
   *  Thread binds one after a restart). */
  scope: ServerScope | null
  addr: string
  /** The words as posted (the bubble's text). */
  text: string
  /** The POST body, `clientMsgId` included; every try sends it unchanged. */
  body: Record<string, unknown>
  /** The server was known to take the id at send time (else an older
   *  server's copy is matched by text too). */
  keyed: boolean
  /** Send time, unix seconds. */
  createdAtSec: number
  state: OutboxState
  error?: string
  /** Tries that failed on the way. */
  attempts: number
  /** Not before this (ms) for an automatic retry; a reconnect ignores it. */
  nextAttemptAtMs: number
  /** The login it was written under (`''` for this computer's daemon). */
  login: string
  /** The newest seq the Thread held at send time, when known: only a newer
   *  item can be an older server's copy of it. */
  afterSeq?: number
}

/** On a display item: this is an unconfirmed send. */
export interface PendingMark {
  clientMsgId: string
  state: OutboxState
  error?: string
}

/** What one try of a send came to (the sender classifies the answer). */
export type OutboxSendOutcome =
  | { kind: 'sent'; item: OverlayThreadItem | null; resp?: Record<string, unknown> }
  /** Out of reach: try again. */
  | { kind: 'transient'; error: string; code?: string }
  /** Kept as "Not sent", never tried again by itself (a reused id). */
  | { kind: 'final'; error: string; code?: string }
  /** The server refused it. The first try returns the words to the
   *  message box; a later one keeps it as "Not sent". */
  | { kind: 'refused'; error: string; code?: string; addressMiss?: boolean }

const entries = new Map<string, OutboxEntry>()
const listeners = new Set<() => void>()
/** Server ids that replaced a pending bubble (drawn without the arrival
 *  animation: the bubble was already on screen). Bounded. */
const replacedIds: string[] = []
let version = 0
const inFlight = new Set<string>()
const draining = new Set<string>()
const drainAgain = new Map<string, boolean>()
let timer: ReturnType<typeof setTimeout> | null = null
let loading: Promise<void> | null = null
/** Newest seq each Thread view holds (for `afterSeq`). */
const newestSeqs = new Map<string, number>()

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

/** The Thread an entry belongs to: the server (canonical host key, so the
 *  window's own view and a Home room of the same server share it, and a
 *  re-added login keeps it) and the address. */
export function outboxKey(scope: Pick<ServerScope, 'hostKey'>, addr: string): string {
  return `${scope.hostKey}|${addr.trim()}`
}

/** The login a scope posts as right now (`''` for this computer), or null
 *  when its server is no longer saved here. */
export function scopeLogin(scope: ServerScope): string | null {
  if (!scope.isRemote) return ''
  const host = scope.connectHost()
  if (!host) return null
  return (host.username ?? '').trim().toLowerCase()
}

function toRow(e: OutboxEntry): PendingSendRow {
  return {
    clientMsgId: e.clientMsgId,
    hostKey: e.hostKey,
    addr: e.addr,
    text: e.text,
    body: e.body,
    keyed: e.keyed,
    createdAtSec: e.createdAtSec,
    state: e.state,
    ...(e.error !== undefined ? { error: e.error } : {}),
    attempts: e.attempts,
    login: e.login,
    ...(e.afterSeq !== undefined ? { afterSeq: e.afterSeq } : {}),
    v: OUTBOX_ROW_VERSION,
  }
}

function fromRow(r: PendingSendRow): OutboxEntry | null {
  if (!r || r.v !== OUTBOX_ROW_VERSION) return null
  if (typeof r.clientMsgId !== 'string' || typeof r.hostKey !== 'string' || typeof r.addr !== 'string') return null
  if (typeof r.text !== 'string' || !r.body || typeof r.body !== 'object') return null
  return {
    clientMsgId: r.clientMsgId,
    key: `${r.hostKey}|${r.addr.trim()}`,
    hostKey: r.hostKey,
    scope: null,
    addr: r.addr,
    text: r.text,
    body: r.body,
    keyed: r.keyed === true,
    createdAtSec: typeof r.createdAtSec === 'number' ? r.createdAtSec : 0,
    // Whatever was on its way when this window went is unknown: try again.
    state: r.state === 'failed' ? 'failed' : 'waiting',
    ...(typeof r.error === 'string' ? { error: r.error } : {}),
    attempts: typeof r.attempts === 'number' ? r.attempts : 0,
    nextAttemptAtMs: 0,
    login: typeof r.login === 'string' ? r.login : '',
    ...(typeof r.afterSeq === 'number' ? { afterSeq: r.afterSeq } : {}),
  }
}

function persist(e: OutboxEntry): Promise<boolean> {
  return savePendingSend(toRow(e))
}

/** Load what this computer saved (once per window). */
export function loadOutbox(): Promise<void> {
  if (loading) return loading
  loading = (async () => {
    const rows = await readPendingSends()
    let added = false
    for (const r of rows) {
      const e = fromRow(r)
      if (!e || entries.has(e.clientMsgId)) continue
      entries.set(e.clientMsgId, e)
      added = true
    }
    if (added) changed()
  })()
  return loading
}

/** Add a send, saved on this computer. Resolves true once it is on disk. */
export function outboxAdd(entry: OutboxEntry): Promise<boolean> {
  entries.set(entry.clientMsgId, entry)
  changed()
  installTriggers()
  return persist(entry)
}

export function outboxGet(clientMsgId: string): OutboxEntry | null {
  return entries.get(clientMsgId) ?? null
}

function update(clientMsgId: string, patch: Partial<OutboxEntry>): OutboxEntry | null {
  const e = entries.get(clientMsgId)
  if (!e) return null
  const next: OutboxEntry = { ...e, ...patch }
  if ('error' in patch && patch.error === undefined) delete next.error
  entries.set(clientMsgId, next)
  changed()
  void persist(next)
  return next
}

export function outboxMark(clientMsgId: string, state: OutboxState, error?: string): void {
  update(clientMsgId, { state, error })
}

/** The server confirmed it (its answer or its copy showed). */
export function outboxSettle(clientMsgId: string, serverId?: string | null): void {
  if (serverId) {
    replacedIds.push(serverId)
    if (replacedIds.length > 100) replacedIds.splice(0, replacedIds.length - 100)
  }
  const e = entries.get(clientMsgId)
  if (!e) return
  entries.delete(clientMsgId)
  void deletePendingSend(clientMsgId)
  changed()
  // Whatever waited behind it may go now.
  if (hasQueued(e.key)) void drain(e.key, true)
}

/** Drop it (the person's Delete, or a first-try refusal whose words go
 *  back to the message box). */
export function outboxDrop(clientMsgId: string): void {
  if (!entries.delete(clientMsgId)) return
  void deletePendingSend(clientMsgId)
  changed()
}

/** Was this server id a pending bubble a moment ago? */
export function wasPendingBubble(id: string): boolean {
  return replacedIds.includes(id)
}

export function outboxEntriesFor(key: string | readonly string[]): OutboxEntry[] {
  const keys = typeof key === 'string' ? [key] : key
  return [...entries.values()].filter((e) => keys.includes(e.key))
}

/** A view of this Thread is up: entries for it go through `scope` (an
 *  entry loaded after a restart has none until now). Only for the server
 *  the scope names right now. */
export function outboxBind(scope: ServerScope, addrs: readonly string[]): void {
  const hostKey = scope.hostKey
  const keys = new Set(addrs.filter((a) => a.trim()).map((a) => `${hostKey}|${a.trim()}`))
  let any = false
  for (const e of entries.values()) {
    if (!keys.has(e.key) || e.scope === scope) continue
    entries.set(e.clientMsgId, { ...e, scope })
    any = true
  }
  if (any) changed()
}

/** What a view of a Thread holds, newest seq (for `afterSeq`). */
export function noteThreadNewestSeq(key: string, seq: number): void {
  if (seq > 0) newestSeqs.set(key, seq)
}

export function threadNewestSeq(key: string): number | undefined {
  return newestSeqs.get(key)
}

// ── Sending ────────────────────────────────────────────────────────────────

function queued(key: string): OutboxEntry[] {
  return [...entries.values()]
    .filter((e) => e.key === key && e.state !== 'failed')
    .sort((a, b) => a.createdAtSec - b.createdAtSec || 0)
}

function hasQueued(key: string): boolean {
  for (const e of entries.values()) if (e.key === key && e.state !== 'failed') return true
  return false
}

/** Older unconfirmed sends to this Thread (a new one waits behind them). */
export function outboxHasEarlier(key: string, clientMsgId: string): boolean {
  for (const e of entries.values()) {
    if (e.key === key && e.clientMsgId !== clientMsgId && e.state !== 'failed') return true
  }
  return false
}

function backoffMs(attempts: number): number {
  return Math.min(BACKOFF_CAP_MS, backoffBaseMs * 2 ** Math.max(0, attempts - 1))
}

function nowSec(): number {
  return Math.floor(Date.now() / 1000)
}

/**
 * One try of one entry. `first`: the send the person just made (a refusal
 * then drops it: the words go back to the box). Null when it could not be
 * tried now (no scope, the scope names another server now, the
 * server is not saved, already on its way).
 */
export async function outboxAttempt(clientMsgId: string, first = false): Promise<OutboxSendOutcome | null> {
  const e = entries.get(clientMsgId)
  if (!e || !e.scope || inFlight.has(clientMsgId)) return null
  // A primary scope follows the server switcher: never post to another server.
  if (e.scope.hostKey !== e.hostKey) {
    entries.set(clientMsgId, { ...e, scope: null })
    return null
  }
  const login = scopeLogin(e.scope)
  if (login === null) return null
  if (login !== e.login) {
    const error = 'Written while signed in as someone else on this server: not sent.'
    update(clientMsgId, { state: 'failed', error })
    return { kind: 'final', error }
  }
  inFlight.add(clientMsgId)
  update(clientMsgId, { state: 'sending', error: undefined })
  let out: OutboxSendOutcome
  try {
    out = await sendOutboxEntry(entries.get(clientMsgId) ?? e)
  } catch (err) {
    out = { kind: 'transient', error: err instanceof Error ? err.message : String(err) }
  } finally {
    inFlight.delete(clientMsgId)
  }
  const cur = entries.get(clientMsgId)
  // Its copy showed meanwhile (a frame or a catch-up), or it was deleted.
  if (!cur) return out.kind === 'sent' || out.kind === 'transient' ? { kind: 'sent', item: null } : out
  switch (out.kind) {
    case 'sent':
      outboxSettle(clientMsgId, out.item?.id ?? null)
      break
    case 'transient': {
      const attempts = cur.attempts + 1
      if (nowSec() - cur.createdAtSec > OUTBOX_AUTO_RETRY_MAX_AGE_SEC) {
        update(clientMsgId, { state: 'failed', error: out.error, attempts })
      } else {
        update(clientMsgId, { state: 'waiting', error: out.error, attempts, nextAttemptAtMs: Date.now() + backoffMs(attempts) })
        scheduleTimer()
      }
      break
    }
    case 'final':
      update(clientMsgId, { state: 'failed', error: out.error })
      break
    case 'refused':
      if (first) outboxDrop(clientMsgId)
      else update(clientMsgId, { state: 'failed', error: out.error })
      break
  }
  return out
}

/** Send what waits for one Thread, oldest first; stop at the first one
 *  still out of reach (order). `force`: ignore the backoff (a reconnect). */
async function drain(key: string, force: boolean): Promise<void> {
  if (draining.has(key)) {
    drainAgain.set(key, (drainAgain.get(key) ?? false) || force)
    return
  }
  draining.add(key)
  try {
    for (;;) {
      const next = queued(key)[0]
      if (!next || inFlight.has(next.clientMsgId) || !next.scope) break
      if (!force && next.nextAttemptAtMs > Date.now()) break
      const out = await outboxAttempt(next.clientMsgId)
      if (!out || out.kind === 'transient') break
      // It reached the server: the rest go now.
      force = true
    }
  } finally {
    draining.delete(key)
    const again = drainAgain.get(key)
    if (again !== undefined) {
      drainAgain.delete(key)
      void drain(key, again)
    }
  }
}

/** Try what waits now: every Thread, or one server's. `force` ignores the
 *  backoff (a reconnect, focus, `online`). */
export function kickOutbox(opts: { hostKey?: string; force?: boolean } = {}): Promise<void> {
  return loadOutbox().then(async () => {
    const keys = new Set<string>()
    for (const e of entries.values()) {
      if (e.state === 'failed') continue
      if (opts.hostKey !== undefined && e.hostKey !== opts.hostKey) continue
      keys.add(e.key)
    }
    await Promise.all([...keys].map((k) => drain(k, opts.force === true)))
  })
}

/** Send one again now (the person's Retry): the same body, the SAME id. */
export async function outboxRetry(clientMsgId: string): Promise<OutboxSendOutcome | null> {
  const e = entries.get(clientMsgId)
  if (!e) return null
  if (e.state === 'failed') update(clientMsgId, { state: 'waiting', nextAttemptAtMs: 0 })
  const out = await outboxAttempt(clientMsgId)
  if (out && out.kind !== 'transient' && hasQueued(e.key)) void drain(e.key, true)
  return out
}

function scheduleTimer(): void {
  if (!autoTimers) return
  let soonest = Infinity
  for (const e of entries.values()) {
    if (e.state === 'waiting' && e.scope) soonest = Math.min(soonest, e.nextAttemptAtMs)
  }
  if (timer !== null) clearTimeout(timer)
  timer = null
  if (!Number.isFinite(soonest)) return
  timer = setTimeout(() => {
    timer = null
    void kickOutbox()
  }, Math.max(0, soonest - Date.now()))
}

let triggersInstalled = false

/** Focus and `online` try what waits at once (a view's own socket reopening
 *  and this computer's daemon coming back are wired by their owners). */
function installTriggers(): void {
  if (triggersInstalled || !autoTimers || typeof window === 'undefined') return
  triggersInstalled = true
  const now = (): void => {
    void kickOutbox({ force: true })
  }
  window.addEventListener('online', now)
  window.addEventListener('focus', now)
  // This computer's daemon came back (restart, slow boot).
  onDaemonConnected(() => {
    void kickOutbox({ hostKey: LOCAL_HOME_HOST, force: true })
  })
}

// ── The view ───────────────────────────────────────────────────────────────

function subscribe(fn: () => void): () => void {
  listeners.add(fn)
  return () => {
    listeners.delete(fn)
  }
}

const getVersion = (): number => version

/** The Thread's unsent bubbles, kept current (one array per change). */
export function useOutbox(keys: readonly string[]): OutboxEntry[] {
  const v = useSyncExternalStore(subscribe, getVersion, getVersion)
  const id = keys.join('\n')
  // eslint-disable-next-line react-hooks/exhaustive-deps
  return useMemo(() => outboxEntriesFor(id.split('\n')), [id, v])
}

/** A server item is this entry's post. */
function matches(entry: OutboxEntry, item: OverlayThreadItem, baseline: ReadonlySet<string> | undefined): boolean {
  const cid = item.doc.clientMsgId
  if (cid) return cid === entry.clientMsgId
  if (entry.keyed) return false
  // No id on the server's copy (a server without the feature): a person's
  // post with the same words, newer than the Thread at send time, sent
  // around that time.
  if (entry.afterSeq !== undefined && item.seq <= entry.afterSeq) return false
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
      clientMsgId: e.clientMsgId,
    },
  }
}

/** Tests: tune the backoff and turn the timer and window triggers off. */
export function configureOutboxForTests(opts: { backoffBaseMs?: number; autoTimers?: boolean }): void {
  if (opts.backoffBaseMs !== undefined) backoffBaseMs = opts.backoffBaseMs
  if (opts.autoTimers !== undefined) autoTimers = opts.autoTimers
}

/** Tests: forget this window's outbox (`keepDisk`: a "restart", the saved
 *  rows stay; else the caller clears them with `clearPendingSendsForTests`). */
export function resetOutboxForTests(): void {
  entries.clear()
  replacedIds.length = 0
  inFlight.clear()
  draining.clear()
  drainAgain.clear()
  newestSeqs.clear()
  loading = null
  if (timer !== null) clearTimeout(timer)
  timer = null
  changed()
}
