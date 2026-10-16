// Clone menu permissions (prd-clone-carries-threads-and-vault-v1 §4.2).
//
// `GET /cli/auth/whoami` on the active server says, for the caller:
//   - `canClone`: the owner, or a login with "Can clone" (Admin Access);
//   - `clonePersonalComputers`: this server lets the caller take a clone
//     to their own computer (Settings → General master switch; always true
//     for the owner).
// The workspace context menus hide "Clone to This Computer" when either is
// false, and "Clone to <host>" when `canClone` is false. Hiding is only
// courtesy: the daemon's clone gate refuses the routes anyway. An older
// daemon (no fields) or a failed lookup reads as allowed, so the menu never
// hides an action the server would accept.

import type { ServerScope } from '@/kessel/server-scope'

export interface ClonePerms {
  canClone: boolean
  personal: boolean
}

export function clonePermsFromWhoami(w: unknown): ClonePerms {
  const o = (w ?? {}) as { canClone?: unknown; clonePersonalComputers?: unknown }
  return { canClone: o.canClone !== false, personal: o.clonePersonalComputers !== false }
}

/** "Clone to This Computer" (pull) is offered. */
export function pullAllowed(p: ClonePerms): boolean {
  return p.canClone && p.personal
}

/** "Clone to <host>" (push) is offered. */
export function pushAllowed(p: ClonePerms): boolean {
  return p.canClone
}

type Get = <T = unknown>(scope: ServerScope, route: string) => Promise<T>

const CACHE_MS = 30_000
const cache = new Map<string, { at: number; perms: ClonePerms }>()

/** Test hook. */
export function clearClonePermsCache(): void {
  cache.clear()
}

/** Whoami-derived perms for `scope`, cached briefly per server key; a slow
 *  or failed lookup (over `timeoutMs`) reads as allowed. */
export async function fetchClonePerms(scope: ServerScope, key: string, get?: Get, timeoutMs = 1500): Promise<ClonePerms> {
  const hit = cache.get(key)
  if (hit && Date.now() - hit.at < CACHE_MS) return hit.perms
  const doGet: Get =
    get ??
    (async <T,>(s: ServerScope, route: string): Promise<T> => {
      const { daemonCliGet } = await import('./daemon-cli')
      return daemonCliGet<T>(s, route)
    })
  try {
    const w = await Promise.race([
      doGet(scope, 'auth/whoami'),
      new Promise<never>((_, reject) => setTimeout(() => reject(new Error('timeout')), timeoutMs)),
    ])
    const perms = clonePermsFromWhoami(w)
    cache.set(key, { at: Date.now(), perms })
    return perms
  } catch {
    return { canClone: true, personal: true }
  }
}
