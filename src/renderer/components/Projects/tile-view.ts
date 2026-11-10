// prd-projects-threads-v1 §3.2: which view an agent tile shows.
//
// A tile shows its project's default view (`defaultView` on the project,
// daemon-owned) unless the person switched it. A switch is in memory only
// (`useProjectGroupsStore.tileViews`), remembers the default it was made
// against, and is dropped as soon as the default moves: changing the project
// default resets every tile, and a personal switch never fights it. Tiles
// never read the Agents page's view memory (`useSessionViewTab`), so a saved
// Split can't reach them.

/** What a project can open its tiles in. */
export type ProjectDefaultView = 'terminal' | 'thread' | 'chatter'

/** What a tile can show: a default view, or Heartbeats (Rosson, 2026-11-10:
 *  a fourth picker option, the agent's recently fired heartbeats). Heartbeats
 *  is only ever a personal switch, never a project default. There is no Split
 *  in a tile. */
export type TileView = ProjectDefaultView | 'heartbeats'

/** The picker's segments, in order. */
export const TILE_VIEWS: readonly TileView[] = ['terminal', 'thread', 'chatter', 'heartbeats']

/** A project with no `defaultView` (a daemon from before it) opens in Thread. */
export const TILE_DEFAULT_VIEW: ProjectDefaultView = 'thread'

export function isProjectDefaultView(raw: unknown): raw is ProjectDefaultView {
  return raw === 'terminal' || raw === 'thread' || raw === 'chatter'
}

/** The project's default, read defensively: absent or anything it doesn't
 *  know (Heartbeats included) is Thread. */
export function projectDefaultView(raw: unknown): ProjectDefaultView {
  return isProjectDefaultView(raw) ? raw : TILE_DEFAULT_VIEW
}

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
