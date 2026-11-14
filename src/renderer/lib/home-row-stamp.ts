// Home groups: turn "the person sent to agent X" (`lib/user-send-bus.ts`)
// into a `lastMessagedAt` stamp on X's rows, for "Most recent at the top".
//
// A stamp is a synced row change, so a burst of sends must not become a
// burst of pushes: each row address is stamped at most once per
// STAMP_MIN_GAP_MS. A send inside that window is not lost: the latest one
// is stamped when the window ends (one trailing stamp), so the order still
// ends up right.

import { homeAddress } from '@/lib/home-address'

export const STAMP_MIN_GAP_MS = 30_000

/** The Home row address a send to Thread address `addr` on `hostKey` is
 *  about: the agent's handle (a sidecar's `k2/3` is agent `k2`). */
export function rowAddressForSend(hostKey: string, addr: string): string | null {
  const handle = addr.trim().toLowerCase().split('::')[0].split('/')[0]
  if (!handle || !hostKey.trim()) return null
  return homeAddress(handle, hostKey.trim().toLowerCase())
}

export interface StamperDeps {
  /** Write the stamp (the Homes store's `stampRowsMessaged`). */
  stamp(address: string, at: number): void
  now(): number
  schedule(fn: () => void, ms: number): unknown
}

/** A send listener with the per-row rate limit. */
export function createSendStamper(deps: StamperDeps): (hostKey: string, addr: string) => void {
  const last = new Map<string, number>()
  const pending = new Map<string, number>()
  const flush = (address: string): void => {
    const at = pending.get(address)
    pending.delete(address)
    if (at === undefined) return
    last.set(address, deps.now())
    deps.stamp(address, at)
  }
  return (hostKey, addr) => {
    const address = rowAddressForSend(hostKey, addr)
    if (!address) return
    const now = deps.now()
    const prev = last.get(address)
    if (prev === undefined || now - prev >= STAMP_MIN_GAP_MS) {
      last.set(address, now)
      deps.stamp(address, now)
      return
    }
    const waiting = pending.has(address)
    pending.set(address, now)
    if (!waiting) deps.schedule(() => flush(address), prev + STAMP_MIN_GAP_MS - now)
  }
}
