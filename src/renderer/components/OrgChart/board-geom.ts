// Board's own geometry, used as it is (prd-org-chart-v1 §6.5, vs-live C48/C51):
// the anchors, elbow and straight routes, end caps and boxes come from
// `board-file-1/geom.js`, the file the Board widget runs. It is a plain
// script that sets `window.BoardGeom`; importing it here runs that same file
// in the renderer, so the Org Chart's lines bend exactly like Board's.
// Not a copy: an edit to Board's routing changes these lines too.

import '../../../../crates/k2-core/src/zen/builtin_widgets/board-file-1/geom.js'

export type Side = 'top' | 'right' | 'bottom' | 'left'
export type Point = [number, number]

export interface Box {
  x: number
  y: number
  w: number
  h: number
}

/** A connector as Board stores one (the fields `route` reads). */
export interface BoardConnector {
  from: { id: string; anchor: Side | 'auto' }
  to: { id: string; anchor: Side | 'auto' }
  route: 'elbow' | 'straight' | 'curved'
}

export interface BoardRoute {
  d: string
  pts: Point[]
  start: Point
  end: Point
  missing: boolean
  unplaced: boolean
}

interface BoardGeomApi {
  route(c: BoardConnector, get: (id: string) => Box | null): BoardRoute
  anchorPoint(o: Box, side: Side): Point
  midpoint(pts: Point[]): Point
  bounds(list: Box[]): Box | null
  intersects(a: Box, b: Box): boolean
  inside(a: Box, b: Box): boolean
  cap(kind: string, p: Point, d: Point, sw: number): { d: string; filled: boolean } | null
}

const geom = (globalThis as unknown as { BoardGeom?: BoardGeomApi }).BoardGeom
if (!geom) throw new Error('board-geom: board-file-1/geom.js did not load (window.BoardGeom is missing)')

export const BoardGeom: BoardGeomApi = geom

/** Board's dash patterns (`board-file-1/render.js`, `dash()`), for a stroke
 *  width: the Org Chart's dotted layer is Board's `dotted`. render.js keeps
 *  this inside its renderer, so it is mirrored here; `OrgChart.board.test.ts`
 *  draws a Board connector with render.js itself and checks both agree. */
export function boardDash(kind: 'solid' | 'dashed' | 'dotted', sw = 2): string {
  return kind === 'dashed' ? `${sw * 4} ${sw * 3}` : kind === 'dotted' ? `0.1 ${sw * 2.5}` : ''
}
