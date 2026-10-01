// Test helpers for Home M1's required ServerScope argument.
//
// Every `daemonCli*` helper and `getDaemonWs` now takes a scope first. Tests
// that mock those helpers with a pre-M1 argument list wrap the mock with
// `primaryOnly`: it fails loudly unless the call site passed
// `primaryScope()`, then forwards the remaining arguments unchanged.

import { primaryScope, type ServerScope } from '@/kessel/server-scope'

/** Throw unless `scope` is the window's primary scope. */
export function expectPrimaryScope(scope: unknown): void {
  if (scope !== primaryScope()) {
    const label =
      scope && typeof scope === 'object' && 'id' in scope
        ? String((scope as ServerScope).id)
        : JSON.stringify(scope)
    throw new Error(`expected primaryScope() as the first argument, got ${label}`)
  }
}

/** Wrap a mock written for the pre-M1 argument list. */
export function primaryOnly<A extends unknown[], R>(
  fn: (...args: A) => R,
): (scope: ServerScope, ...args: A) => R {
  return (scope: ServerScope, ...args: A): R => {
    expectPrimaryScope(scope)
    return fn(...args)
  }
}
