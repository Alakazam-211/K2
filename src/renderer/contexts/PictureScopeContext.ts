// Which server a person chip reads its picture from (0.46.8, per-server
// pictures, Rosson 2026-11-04). `RoomProvider` sets it to its room's server,
// so a chip in a Thread, a room's rows or a pinned room shows the picture
// that server keeps for the person. Outside any room (Settings, the
// profile modal) it is null and the chip reads the window's own server.
// A caller that knows better passes `pictureScope` to the chip itself
// (the top bar following a room, a Home row on another server).

import { createContext, useContext } from 'react'
import type { ServerScope } from '@/kessel/server-scope'

export const PictureScopeContext = createContext<ServerScope | null>(null)

/** The enclosing room's server, or null outside a room. */
export function usePictureScopeContext(): ServerScope | null {
  return useContext(PictureScopeContext)
}
