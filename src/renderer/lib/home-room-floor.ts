// Home 0.43.2 (prd-home-seamless-0432 Z5, Z42; Rosson 2026-10-03 Q5) —
// what a Home row on another server does, by THAT server's version.
//
// Decided per row from the server's own `/boot-status` in the pool, never
// from the window's server:
//
//   | that server                         | the row                          |
//   |-------------------------------------|----------------------------------|
//   | not checked yet                     | opens a room; the floor is       |
//   |                                     | checked when the pool knows (Z42)|
//   | no version, or below the floor      | switches the window, one toast   |
//   |                                     | per server per session           |
//   | floor ≤ version < 0.43.0            | a view-only room with a          |
//   |                                     | "Switch to {server}" button      |
//   | 0.43.0 or newer                     | a usable room                    |
//
// The usable / view-only split in a room that is open comes from the layout
// revision probe (`stores/home-rooms.ts`), not from the version. The
// version split here is for copy (the row's tooltip) and for the floor.

import { FEATURES, gte } from '@/lib/server-capabilities'
import type { PoolBoot } from '@/lib/host-pool'

/** The oldest K2 that opens in place (Q5). Below it the row switches the
 *  window. Same number as the room's own `version-too-old` banner. */
export const HOME_ROOM_FLOOR: string = FEATURES['home-room']

/** The first K2 with the layout revision check (P0, `2a12ec7b`): its rooms
 *  are usable. Older ones at or above the floor are view only. */
export const HOME_ROOM_USABLE_FROM = '0.43.0'

export type HomeRoomVerdict = 'unknown' | 'switch' | 'view-only' | 'use'

/** What a row on a server with this `/boot-status` does. */
export function homeRoomVerdict(boot: PoolBoot | null | undefined): HomeRoomVerdict {
  if (!boot) return 'unknown'
  if (boot.version === null) return 'switch'
  if (!gte(boot.version, HOME_ROOM_FLOOR)) return 'switch'
  if (!gte(boot.version, HOME_ROOM_USABLE_FROM)) return 'view-only'
  return 'use'
}

/** The toast for a server below the floor. */
export function oldServerToastText(serverLabel: string, version: string | null): string {
  const runs = version === null ? 'an older K2' : `K2 ${version}`
  return `${serverLabel} runs ${runs}. It opens by switching this window. Update it to open it here.`
}
