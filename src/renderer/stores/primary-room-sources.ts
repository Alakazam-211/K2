// Home M4 — the window's own presence roster and Active set, as the primary
// room reads them. `presence.ts` and `active.ts` register their stores here
// at module load. This module imports nothing at run time, so registering
// never pulls the room / tabs graph into those small stores (and never
// forms an import cycle through them).

import type { StoreApi } from 'zustand'
import type { ActiveViewStore, PresenceViewStore } from '@/stores/server-view'

let presence: PresenceViewStore | null = null
let activeSet: ActiveViewStore | null = null

/** presence.ts registers the window's roster store (R7). */
export function registerPrimaryRoomPresence(store: PresenceViewStore): void {
  presence = store
}

/** active.ts registers the window's Active-set store. */
export function registerPrimaryRoomActiveSet(store: ActiveViewStore): void {
  activeSet = store
}

type ReadStore<T> = Pick<StoreApi<T>, 'getState' | 'getInitialState' | 'subscribe'>

/** A read-only facade over a store registered later. Reading it before
 *  registration throws (a missing store is a crash, never an empty list). */
function lateStore<T>(get: () => ReadStore<T> | null, what: string): ReadStore<T> {
  const req = (): ReadStore<T> => {
    const s = get()
    if (!s) throw new Error(`primary room: the ${what} store is not loaded`)
    return s
  }
  return {
    getState: () => req().getState(),
    getInitialState: () => req().getInitialState(),
    subscribe: (listener) => req().subscribe(listener),
  }
}

export const PRIMARY_PRESENCE: PresenceViewStore = lateStore(() => presence, 'presence')
export const PRIMARY_ACTIVE_SET: ActiveViewStore = lateStore(() => activeSet, 'active')
