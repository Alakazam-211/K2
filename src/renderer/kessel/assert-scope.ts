// Home M1 / G9 — the runtime half of "a scope is required".
//
// The typechecker already refuses a call without a ServerScope. This guard
// catches untyped or cast callers at run time with a clear message, and the
// request layer never falls back to the window's server.

import type { ServerScope } from '@/kessel/server-scope'

/** Home M1 / G9: fail loudly when a caller passes something that is not a
 *  ServerScope (an untyped JS caller, or a cast). Never fall back to the
 *  window's server. */
export function assertServerScope(scope: unknown, fn: string): asserts scope is ServerScope {
  const s = scope as Partial<ServerScope> | null | undefined
  if (
    !s ||
    typeof s !== 'object' ||
    typeof s.id !== 'string' ||
    typeof s.creds !== 'function' ||
    typeof s.isWindowHost !== 'function'
  ) {
    throw new TypeError(`${fn}: a ServerScope is required as the first argument (got ${typeof scope})`)
  }
}

