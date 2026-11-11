// An agent tile's bell and search, in its bar (Rosson, 2026-11-11: "each
// tile's bell and search sit in the tile's bar", not floating over the
// Thread). Thread view has both, Chatter view search only, Terminal and
// Heartbeats neither (threadFloats.tsx, the `tile` places). They are the
// Thread view's own objects, drawn as the bar's buttons: the same quiet look
// as the bar's × (a 24px target, muted at rest, a soft fill under the
// pointer, the shared focus ring; round in Glass, cut in Terminal).
//
// The tile is the search's host (PaneChrome puts `data-thread-floats-host`
// on it), so ⌘F anywhere in the tile searches its Thread, and the field
// opens just under the bar (ThreadFloatSearch `bar`).

import type { JSX } from 'react'
import { ThreadFloatBell } from '@/components/SessionView/ThreadFloatBell'
import { ThreadFloatSearch } from '@/components/SessionView/ThreadFloatSearch'
import { threadFloatShown, type FloatButtonLook, type ThreadFloatsPlace } from '@/components/SessionView/threadFloats'
import type { TileView } from './tile-view'

/** The bar's icon button: the tile's × look (DialogCloseButton, app tone),
 *  a step brighter at rest (the owner's mockup: they are the bar's tools),
 *  pressed (the search field open) as under the pointer. */
export const TILE_BAR_BUTTON: FloatButtonLook = {
  'data-k2-chamfer': 'control',
  'data-tile-button': '',
  className: [
    'k2-focus no-drag flex h-6 w-6 flex-shrink-0 cursor-pointer items-center justify-center transition-colors [--k2-ch-bw:0px]',
    'rounded-[var(--radius-pill)] bg-transparent text-[var(--color-text-secondary)]',
    'hover:bg-[color-mix(in_srgb,var(--color-text-primary)_10%,transparent)] hover:text-[var(--color-text-primary)]',
    'aria-expanded:bg-[color-mix(in_srgb,var(--color-text-primary)_10%,transparent)] aria-expanded:text-[var(--color-text-primary)]',
    'active:bg-[color-mix(in_srgb,var(--color-text-primary)_16%,transparent)]',
    'disabled:pointer-events-none disabled:opacity-50',
  ].join(' '),
}

/** Which of the Thread's objects a tile's view keeps (null: none). */
export function tileToolsPlace(view: TileView): ThreadFloatsPlace | null {
  if (view === 'thread') return 'tile'
  if (view === 'chatter') return 'tileChatter'
  return null
}

/** The bell, then search. Each sits in a slot the bar's fit can measure, and
 *  the bell's can fold away (`data-tile-bell="off"`, tile-bar-fit.ts). A
 *  view switch starts the search afresh (Thread and Chatter are different
 *  lists). */
export function TileBarTools({ view, workspaceId, name }: { view: TileView; workspaceId: string; name: string }): JSX.Element | null {
  const place = tileToolsPlace(view)
  if (!place) return null
  return (
    <>
      {threadFloatShown('bell', place) ? (
        <span className="flex" data-tile-tool="bell">
          <ThreadFloatBell projectId={workspaceId} bar={TILE_BAR_BUTTON} label={`Ring when ${name} finishes`} />
        </span>
      ) : null}
      {threadFloatShown('search', place) ? (
        <span className="flex ml-0.5" data-tile-tool="search">
          <ThreadFloatSearch key={view} bar={TILE_BAR_BUTTON} />
        </span>
      ) : null}
    </>
  )
}
