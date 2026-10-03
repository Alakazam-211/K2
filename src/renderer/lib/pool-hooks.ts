// Home M2 — small registries that let the request layer and the event
// sockets reach the connection pool without importing it. The pool
// (`lib/host-pool-instance.ts`) registers here; `lib/daemon-cli.ts` and
// `stores/session-events.ts` only call through. Kept free of runtime
// imports so it never changes module order (tests mock the request layer).

import type { ServerScope } from '@/kessel/server-scope'

/** What the connection pool answers when a pinned scope's request was
 *  refused as unauthorized. Only 'revived' replays the request. */
export type PinnedAuthOutcome = 'revived' | 'not-revived'

type PinnedAuthRecovery = (scope: ServerScope) => Promise<PinnedAuthOutcome>
let pinnedAuthRecovery: PinnedAuthRecovery | null = null

/** Home M2 (MS10, MS77): how a server that is NOT the window's own
 *  recovers a refused login: one revive through the pool's login lease,
 *  never a keychain delete, never the full-screen overlay. Returns the
 *  unregister. */
export function setPinnedAuthRecovery(fn: PinnedAuthRecovery | null): () => void {
  pinnedAuthRecovery = fn
  return () => {
    if (pinnedAuthRecovery === fn) pinnedAuthRecovery = null
  }
}

export function getPinnedAuthRecovery(): PinnedAuthRecovery | null {
  return pinnedAuthRecovery
}

type SocketCloseSink = (hostKey: string, code: number) => void
let socketCloseSink: SocketCloseSink | null = null

/** MS71: the pool hears every daemon socket close, to tell a kick (4001)
 *  from a network drop. */
export function setPoolSocketCloseSink(fn: SocketCloseSink | null): void {
  socketCloseSink = fn
}

export function notePoolSocketClose(hostKey: string, code: number): void {
  if (socketCloseSink) socketCloseSink(hostKey, code)
}

/** What the top bar reads about one server from the pool (0.43.2 Z19, Q3):
 *  whether it answers, whether its login is good, and the login's role. */
export interface PoolHostStatus {
  readonly reach: string
  readonly auth: string
  readonly role: string | null
  readonly checkedAt: number | null
}

/** The pool's entries as a store the top bar subscribes to. */
export interface PoolStatusSource {
  getState(): { entries: Readonly<Record<string, PoolHostStatus>> }
  subscribe(listener: () => void): () => void
}

let poolStatusSource: PoolStatusSource | null = null

/** 0.43.2 Z19: the pool's entries, for the top bar of a focused Home room
 *  (offline, sign in, role). Returns the unregister. */
export function setPoolStatusSource(src: PoolStatusSource | null): () => void {
  poolStatusSource = src
  return () => {
    if (poolStatusSource === src) poolStatusSource = null
  }
}

export function getPoolStatusSource(): PoolStatusSource | null {
  return poolStatusSource
}
