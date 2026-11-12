// prd-projects-threads-v1 §3.2: which view an agent tile shows.
//
// A tile shows its agent's default view unless the person switched it: the
// agent's own (`ownDefaultView`, 0302) else the project's (`defaultView`),
// both daemon-owned (`memberDefaultView`). A switch is in memory only
// (`useProjectGroupsStore.tileViews`), remembers the default it was made
// against, and is dropped as soon as that default moves: changing the
// project's or the agent's view resets the tiles it reaches, and a personal
// switch never fights it. Tiles
// never read the Agents page's view memory (`useSessionViewTab`), so a saved
// Split can't reach them.

import type { ProjectDefaultView } from './projects-api'

/** What a tile can show: a default view, or Heartbeats (Rosson, 2026-11-10:
 *  a fourth picker option, the agent's recently fired heartbeats). Heartbeats
 *  is only ever a personal switch, never a project default. There is no Split
 *  in a tile. */
export type TileView = ProjectDefaultView | 'heartbeats'

/** The picker's segments, in order. */
export const TILE_VIEWS: readonly TileView[] = ['terminal', 'thread', 'chatter', 'heartbeats']

/** One personal switch: the view, and the default it was made against. */
export interface TileViewEntry {
  view: TileView
  against: ProjectDefaultView
}

/** One tile's memory key: per server, per project, per agent. */
export function tileViewKey(hostKey: string, groupId: string, workspaceId: string): string {
  return `${hostKey}|${groupId}|${workspaceId}`
}

/** The view to show, and whether the entry is stale (made against another
 *  default) and should be dropped. */
export function resolveTileView(
  entry: TileViewEntry | undefined,
  projectDefault: ProjectDefaultView,
): { view: TileView; stale: boolean } {
  if (!entry) return { view: projectDefault, stale: false }
  if (entry.against !== projectDefault) return { view: projectDefault, stale: true }
  return { view: entry.view, stale: false }
}

/** The entry a pick stores; null when the pick is the default itself (no
 *  entry needed). */
export function pickTileView(next: TileView, projectDefault: ProjectDefaultView): TileViewEntry | null {
  return next === projectDefault ? null : { view: next, against: projectDefault }
}
