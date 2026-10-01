// Room context (Home M3, prd-home-multi-server-client MS2/MS15).
//
// Room components read their room — its tabs store, scope, project list and
// activity sink — from here. There is NO fallback: a room component rendered
// without a `RoomProvider` throws, so a missed provider is a crash in
// development and in tests, never a silent request to the window's server.
// The Agents shell, Home on the connected server, the Focus window and the
// pinned-chat retainer all pass `primaryRoom()` explicitly.

import { createContext, useContext, useEffect, type ReactNode } from 'react'
import { useStore } from 'zustand'
import type { Room } from '@/stores/room'
import type { TabsState } from '@/stores/tabs'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { roomHidden, roomShown } from '@/stores/window-room'
import { useServerSupports, type FeatureKey } from '@/lib/server-capabilities'

const RoomContext = createContext<Room | null>(null)

export class MissingRoomError extends Error {
  constructor(hook: string) {
    super(
      `${hook}: no RoomProvider above this room component. Room code never falls back to the window's server (MS2); wrap the room in <RoomProvider room={…}>.`,
    )
    this.name = 'MissingRoomError'
  }
}

/** Hand `room` to everything below. `shown` registers the room with the
 *  window's focused-room pointer while mounted (the room in the main area);
 *  providers around drawers or portals of the same room leave it off. */
export function RoomProvider({
  room,
  shown = false,
  children,
}: {
  room: Room
  shown?: boolean
  children: ReactNode
}): React.JSX.Element {
  useEffect(() => {
    if (!shown) return
    roomShown(room)
    return () => roomHidden(room)
  }, [room, shown])
  return <RoomContext.Provider value={room}>{children}</RoomContext.Provider>
}

/** This component's room. Throws with no provider (MS2). */
export function useRoom(): Room {
  const room = useContext(RoomContext)
  if (!room) throw new MissingRoomError('useRoom')
  return room
}

/** Select from this room's tabs store (the room-scoped `useTabsStore`). */
export function useRoomTabs<T>(selector: (state: TabsState) => T): T {
  const room = useContext(RoomContext)
  if (!room) throw new MissingRoomError('useRoomTabs')
  return useStore(room.tabs, selector)
}

/** Select from this room's own project list (MS3). */
export function useRoomProjects<T>(selector: (projects: ProjectWithWorkspaces[]) => T): T {
  const room = useContext(RoomContext)
  if (!room) throw new MissingRoomError('useRoomProjects')
  return useStore(room.projects, (s) => selector(s.projects))
}

/** Does THIS room's server support `feature` (MS11/MS80)? The primary room
 *  keeps the window's live `serverVersion` subscription; a pinned room
 *  reads its own server's version. */
export function useRoomSupports(feature: FeatureKey): boolean {
  const room = useContext(RoomContext)
  if (!room) throw new MissingRoomError('useRoomSupports')
  const windowSupports = useServerSupports(feature)
  return room.isPrimary ? windowSupports : room.scope.serverSupports(feature)
}
