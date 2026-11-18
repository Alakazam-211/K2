// Where the floating call tile rests in this window (CSS px), while it is
// up: the corner's spot it is anchored to and its size, and whether that is
// a bottom corner. A message list it sits over reads it to make room under
// its newest message (useCallTileListInset). Per-window view state, never
// the daemon's.

import { create } from 'zustand'
import type { TileRest } from '@/lib/voice/mini-snap'

/** Between the tile and what it keeps clear (px): the window's edges, a
 *  message box, the newest message of a list it rests over. */
export const CALL_TILE_GAP = 8

export const useCallTilePlace = create<{ rest: TileRest | null }>(() => ({ rest: null }))

const same = (a: TileRest | null, b: TileRest): boolean =>
  a !== null && a.left === b.left && a.top === b.top && a.width === b.width && a.height === b.height && a.bottom === b.bottom

/** The tile came to rest at `rest` (it anchored to a corner's spot). */
export function setCallTileRest(rest: TileRest): void {
  if (!same(useCallTilePlace.getState().rest, rest)) useCallTilePlace.setState({ rest })
}

/** The tile went away. */
export function clearCallTileRest(): void {
  if (useCallTilePlace.getState().rest !== null) useCallTilePlace.setState({ rest: null })
}
