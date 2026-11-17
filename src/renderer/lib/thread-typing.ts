// Who else is typing in an agent Thread, and this window's own "I'm
// typing" signal. Both ride the Thread's overlay socket
// (`WS /cli/overlay/events`, opened by useOverlayThread), on a server that
// reports `thread-typing-v1` in `/boot-status`.
//
// In: the daemon sends `{collection: "typing", seq: 0, id: <thread addr>,
// member: <principal id>, name: <display name>, kind: "person" |
// "app_guest", at: <ms>}` to the Thread's other readers only (never the
// typist's own sockets), at most one per member every 2 s.
// `parseThreadTypingFrame` is the one place a frame becomes typing; a
// match lands in this module's store by server and Thread address. A name
// holds THREAD_TYPING_HOLD_MS after its last frame, and goes at once when
// that member's post arrives (`clearThreadTypist`). The viewer's own
// member is never shown.
//
// Out: the composer calls `noteTyping(text)` (useThreadTypingSignal) as the
// person types; while the box has text it sends `{"t": "typing", "thread":
// <thread addr>}` on that Thread's socket at most once every
// THREAD_TYPING_SEND_EVERY_MS, and nothing at all to a server without
// `thread-typing-v1`. There is no stop frame: the name runs out on the
// other side, or goes when the post lands.

import { useCallback, useMemo, useRef, useSyncExternalStore } from 'react'
import type { ServerScope } from '@/kessel/server-scope'
import { useThreadViewer, type ThreadViewer } from '@/lib/thread-viewer'

/** The boot-status feature that says the server speaks typing. */
export const THREAD_TYPING_FEATURE = 'thread-typing-v1'
/** A name stays this long after its member's last frame (ms). */
export const THREAD_TYPING_HOLD_MS = 10_000
/** The most often this window says it is typing in one Thread (ms). */
export const THREAD_TYPING_SEND_EVERY_MS = 8_000

/** One typing frame. */
export interface ThreadTypingFrame {
  /** The Thread's address (the frame's `id`). */
  thread: string
  /** The typist's principal id. */
  member: string
  /** The name to show (the member's id when the frame has none). */
  name: string
  /** `person` or `app_guest`; a later kind still parses. */
  kind: string
  /** When the server saw it (ms); 0 when absent. */
  at: number
}

/** A socket frame as typing, or null when it isn't one. */
export function parseThreadTypingFrame(raw: unknown): ThreadTypingFrame | null {
  if (!raw || typeof raw !== 'object' || Array.isArray(raw)) return null
  const f = raw as Record<string, unknown>
  if (f.collection !== 'typing') return null
  const thread = typeof f.id === 'string' ? f.id.trim() : ''
  const member = typeof f.member === 'string' ? f.member.trim() : ''
  if (!thread || !member) return null
  const name = typeof f.name === 'string' && f.name.trim() ? f.name.trim() : member
  const kind = typeof f.kind === 'string' ? f.kind : ''
  const at = typeof f.at === 'number' && Number.isFinite(f.at) ? f.at : 0
  return { thread, member, name, kind, at }
}

/** The words under the composer: "Mira is typing…", "Mira and Ben are
 *  typing…", "Several people are typing…"; empty for nobody. */
export function threadTypingWords(names: readonly string[]): string {
  if (names.length === 0) return ''
  if (names.length === 1) return `${names[0]} is typing…`
  if (names.length === 2) return `${names[0]} and ${names[1]} are typing…`
  return 'Several people are typing…'
}

/** The outgoing frame for `thread`. */
export function threadTypingSignalFrame(thread: string): string {
  return JSON.stringify({ t: 'typing', thread })
}

// ── Who is typing, per server and Thread ───────────────────────────────

/** One typist as the store keeps them: one entry per member (all their
 *  devices), kept in the order they started, oldest first. */
export interface ThreadTypist {
  member: string
  name: string
}

type Held = ThreadTypist & { until: number }

const held = new Map<string, Map<string, Held>>()
const snapshots = new Map<string, readonly ThreadTypist[]>()
const listeners = new Set<() => void>()
const NOBODY: readonly ThreadTypist[] = []
let sweep: ReturnType<typeof setTimeout> | null = null

function keyOf(hostKey: string, thread: string): string {
  return `${hostKey}\u0000${thread}`
}

/** Drop what ran out, refresh the changed snapshots, tell the listeners,
 *  and wake again when the next name runs out. */
function publish(now: number): void {
  let next = Infinity
  const keys = new Set([...snapshots.keys(), ...held.keys()])
  for (const [key, who] of held) {
    for (const [member, t] of who) {
      if (t.until <= now) who.delete(member)
      else next = Math.min(next, t.until)
    }
    if (who.size === 0) held.delete(key)
  }
  let changed = false
  for (const key of keys) {
    const prev = snapshots.get(key) ?? NOBODY
    const cur = [...(held.get(key)?.values() ?? [])].map(({ member, name }) => ({ member, name }))
    if (prev.length === cur.length && prev.every((p, i) => p.member === cur[i].member && p.name === cur[i].name)) continue
    changed = true
    if (cur.length) snapshots.set(key, cur)
    else snapshots.delete(key)
  }
  if (sweep) clearTimeout(sweep)
  sweep = Number.isFinite(next) ? setTimeout(() => publish(Date.now()), next - now + 10) : null
  if (changed) for (const l of listeners) l()
}

/** A typing frame from the server `hostKey`: its member types in its
 *  Thread for the next THREAD_TYPING_HOLD_MS. */
export function noteThreadTyping(hostKey: string, frame: ThreadTypingFrame, now = Date.now()): void {
  const key = keyOf(hostKey, frame.thread)
  const who = held.get(key) ?? new Map<string, Held>()
  who.set(frame.member, { member: frame.member, name: frame.name, until: now + THREAD_TYPING_HOLD_MS })
  held.set(key, who)
  publish(now)
}

/** `member` posted in the Thread at `thread`: they stopped typing. */
export function clearThreadTypist(hostKey: string, thread: string, member: string, now = Date.now()): void {
  const who = held.get(keyOf(hostKey, thread))
  if (!who?.delete(member)) return
  publish(now)
}

/** Forget every typist (tests). */
export function resetThreadTypingForTests(): void {
  held.clear()
  snapshots.clear()
  sockets.clear()
  if (sweep) clearTimeout(sweep)
  sweep = null
  for (const l of listeners) l()
}

function subscribe(l: () => void): () => void {
  listeners.add(l)
  return () => listeners.delete(l)
}

const fold = (s: string): string => s.trim().toLowerCase()

/** Is `member` the person looking (their principal, or a login that is
 *  the owner too)? */
export function isViewerMember(member: string, viewer: ThreadViewer | null): boolean {
  if (!viewer) return false
  const m = fold(member)
  return m === fold(viewer.principal) || viewer.aliases.some((a) => fold(a) === m)
}

/** The names of who else is typing in the Thread at `threadAddr`, oldest
 *  first; empty when nobody is (or the server sends no typing). */
export function useThreadTyping(scope: ServerScope, threadAddr: string): readonly string[] {
  const key = threadAddr ? keyOf(scope.hostKey, threadAddr) : null
  const typists = useSyncExternalStore(
    subscribe,
    () => (key ? snapshots.get(key) ?? NOBODY : NOBODY),
    () => NOBODY,
  )
  const viewer = useThreadViewer(scope)
  return useMemo(() => typists.filter((t) => !isViewerMember(t.member, viewer)).map((t) => t.name), [typists, viewer])
}

// ── This window's own typing ───────────────────────────────────────────

/** An open overlay socket that can carry the signal: its server, the
 *  Thread it reads now, and a send that says whether the frame went. */
interface TypingSocket {
  hostKey: string
  thread: () => string
  send: (frame: string) => boolean
}

const sockets = new Set<TypingSocket>()

/** An overlay socket offers itself for the signal; returns the release. */
export function registerThreadTypingSocket(
  hostKey: string,
  thread: () => string,
  send: (frame: string) => boolean,
): () => void {
  const s: TypingSocket = { hostKey, thread, send }
  sockets.add(s)
  return () => {
    sockets.delete(s)
  }
}

/** Say this person is typing in `threadAddr`, on that Thread's socket.
 *  False when it didn't go: the server lacks `thread-typing-v1`, or no
 *  socket reads that Thread now. */
export function sendThreadTyping(scope: ServerScope, threadAddr: string): boolean {
  if (!threadAddr || !scope.serverSupports(THREAD_TYPING_FEATURE)) return false
  for (const s of sockets) {
    if (s.hostKey === scope.hostKey && s.thread() === threadAddr && s.send(threadTypingSignalFrame(threadAddr))) return true
  }
  return false
}

/** `noteTyping(text)` for the composer to call with the box's text as the
 *  person types: one signal at most every THREAD_TYPING_SEND_EVERY_MS for
 *  this Thread, and none while the box is empty. */
export function useThreadTypingSignal(scope: ServerScope, threadAddr: string): (text: string) => void {
  const last = useRef<{ thread: string; at: number }>({ thread: '', at: -Infinity })
  return useCallback((text: string) => {
    if (!threadAddr || text.length === 0) return
    const now = Date.now()
    if (last.current.thread === threadAddr && now - last.current.at < THREAD_TYPING_SEND_EVERY_MS) return
    if (sendThreadTyping(scope, threadAddr)) last.current = { thread: threadAddr, at: now }
  }, [scope, threadAddr])
}
