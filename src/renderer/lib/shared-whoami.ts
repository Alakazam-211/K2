// One `GET /cli/auth/whoami` per server login, shared
// (research-remote-thread-load-v1 Fix 1, item E). Opening a remote Thread
// asked it twice (the Thread's viewer and the clone-proposals banner), each
// a full tunnel round trip in the same 4-slot queue as the Thread read.
//
// The answer is the server's (who this login is and its role); it is only
// remembered here for a short window, never stored. Keyed by the scope's
// connection and its saved login token, so a re-login, a sign-out or a
// server switch asks again. A failure is not remembered.

import { daemonCliGet } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'

/** `GET /cli/auth/whoami`'s answer (fields a caller reads). */
export interface WhoamiAnswer {
  owner?: unknown
  username?: unknown
  role?: unknown
  [k: string]: unknown
}

/** How long one answer is reused. */
export const WHOAMI_TTL_MS = 5 * 60_000

const memo = new Map<string, { at: number; value: WhoamiAnswer }>()
const inflight = new Map<string, Promise<WhoamiAnswer>>()

function keyOf(scope: ServerScope): string {
  let login = ''
  try {
    const host = scope.connectHost()
    login = host ? `${host.username ?? ''}:${host.token ?? ''}` : ''
  } catch {
    login = ''
  }
  return `${scope.connectionKey}|${login}`
}

/** This login's whoami on `scope`'s server: one request per login per
 *  {@link WHOAMI_TTL_MS}, callers in the same window share it. Throws what
 *  the request threw (nothing is remembered then). */
export function loadWhoami(scope: ServerScope, now: () => number = Date.now): Promise<WhoamiAnswer> {
  const key = keyOf(scope)
  const hit = memo.get(key)
  if (hit && now() - hit.at < WHOAMI_TTL_MS) return Promise.resolve(hit.value)
  const running = inflight.get(key)
  if (running) return running
  const p = daemonCliGet<WhoamiAnswer>(scope, 'auth/whoami')
    .then((value) => {
      const v = value && typeof value === 'object' ? value : ({} as WhoamiAnswer)
      memo.set(key, { at: now(), value: v })
      return v
    })
    .finally(() => {
      inflight.delete(key)
    })
  inflight.set(key, p)
  return p
}

/** Forget every remembered answer (sign-out, a server removed). */
export function forgetWhoami(): void {
  memo.clear()
}

/** Tests: forget everything. */
export function resetSharedWhoamiForTests(): void {
  memo.clear()
  inflight.clear()
}
