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
