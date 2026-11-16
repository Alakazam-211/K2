// What draws the monitor's picture (build-live-timeline.md §4). The
// monitor works out *what* to show (monitor-view.ts: a frame, a tile, the
// poster, a soft frame while seeking, the poster dimmed while the first
// frame loads, or nothing behind a card); a MonitorSource draws it.
//
// - `SampleFootage` (sample-footage.ts): the mock's drawn sources, sample
//   only, in the sample's lazy chunk. Never on a real file.
// - `NO_FOOTAGE`: a real file before K2 Render: no picture, so the monitor
//   says "Preparing video" or shows its cards; never sample footage.
// - `RenderStills` and `RenderPlayer` (T8) follow the design section.

import type { MediaInfo } from './picture'

/** How the monitor plays: cuts only, stills (no player here), or preparing (proxies and tiles not made yet). */
export type MonitorMode = 'cuts' | 'stills' | 'preparing'

export interface MonitorPaint {
  /** frame: the exact frame; tile: the nearest filmstrip tile (or the head); poster: the source's poster; soft: a small picture while seeking; dim: the poster at 60% while the first frame loads; none: black (a card or the gap says why). */
  how: 'frame' | 'tile' | 'poster' | 'soft' | 'dim' | 'none'
  mi: MediaInfo | null
  /** The source frame to draw (the tile's own frame for a tile). */
  srcF: number | null
  /** The clip's in-point in the source (the poster is drawn from here). */
  inF: number | null
}

export interface MonitorSource {
  /** Which source this is (details and tests read it). */
  readonly kind: 'sample' | 'none' | 'stills' | 'player'
  /** Sample data: the monitor labels its picture "Sample data". */
  readonly sample: boolean
  /** Draw `p` into a w × h context (CSS pixels; the caller scales for the screen). Black where there's nothing. */
  paint(g: CanvasRenderingContext2D, w: number, h: number, p: MonitorPaint): void
}

/** A real file before K2 Render: nothing to draw. */
export const NO_FOOTAGE: MonitorSource = {
  kind: 'none',
  sample: false,
  paint(g, w, h) {
    g.fillStyle = '#000'
    g.fillRect(0, 0, w, h)
  },
}
