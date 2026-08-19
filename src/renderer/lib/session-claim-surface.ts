// Last claim surface we (this client) sent for a daemon session.
// Used by the passive "viewing at C×R" pill to say "project viewer"
// when we last drove size from a Projects dashboard. Other users'
// project claims need a daemon stamp (not on this host yet).

import { useSyncExternalStore } from 'react'

export type ClaimSurface = 'agent' | 'project'

const surfaces = new Map<string, ClaimSurface>()
const listeners = new Set<() => void>()

export function noteSessionClaimSurface(sessionId: string, surface: ClaimSurface): void {
  if (!sessionId) return
  if (surfaces.get(sessionId) === surface) return
  surfaces.set(sessionId, surface)
  for (const l of listeners) l()
}

export function sessionClaimSurface(sessionId: string | undefined | null): ClaimSurface | null {
  if (!sessionId) return null
  return surfaces.get(sessionId) ?? null
}

export function useSessionClaimSurface(sessionId: string | undefined | null): ClaimSurface | null {
  return useSyncExternalStore(
    (onStoreChange) => {
      listeners.add(onStoreChange)
      return () => {
        listeners.delete(onStoreChange)
      }
    },
    () => sessionClaimSurface(sessionId),
    () => sessionClaimSurface(sessionId),
  )
}
