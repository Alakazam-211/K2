// The sheet's left-button gestures when the drawing can be selected (the
// model path): click picks, Shift-click adds or removes, a press on empty
// sheet drags a selection box (left to right a window, right to left a
// crossing), and a press on what's selected drags it: a ghost follows the
// pointer, Shift locks it to one axis, and on release the move is tried.
// A refused move snaps the ghost back. Panning stays on the other buttons,
// or Space with the left. The ghost and the box are drawn straight to the
// DOM, never through React, so a drag costs no renders.

import type { ModelRow, XY } from '../model/types'
import { boxMode, boxSelect, pickAt, shiftLock, snapDelta, toggle } from './selection'

export type SheetCtx = {
  /** Screen point → drawing point (y up). */
  toDrawing: (cx: number, cy: number) => XY
  /** Drawing units per screen pixel now. */
  unitsPerPx: () => number
}

export interface SheetTool {
  /** A left press: true when the tool takes it (the sheet doesn't pan). */
  down(e: PointerEvent, c: SheetCtx): boolean
  move(e: PointerEvent, c: SheetCtx): void
  up(e: PointerEvent, c: SheetCtx): void
  /** Esc mid-drag: put everything back. True when there was a drag to stop. */
  cancel(): boolean
}

export interface Ghost {
  show(handles: readonly string[]): void
  at(dx: number, dy: number): void
  /** Back to where it started, then gone. */
  snapBack(): void
  clear(): void
}

export interface SelectBox {
  show(a: { x: number; y: number }, b: { x: number; y: number }, mode: 'window' | 'crossing'): void
  clear(): void
}

export type ToolState = {
  rows: readonly ModelRow[]
  selection: readonly string[]
  select: (handles: readonly string[]) => void
  canMove: (handles: readonly string[]) => boolean
  move: (dx: number, dy: number) => Promise<boolean>
  ghost: Ghost
  box: SelectBox
}

/** Screen pixels a press may travel and still be a click. */
const SLOP_PX = 3
/** How near (screen pixels) a click must be to pick an entity. */
export const PICK_PX = 5

/** The handle of the drawn element under the pointer, when it's one that can be picked. */
function targetHandle(target: EventTarget | null, rows: readonly ModelRow[]): string | null {
  let el = target instanceof Element ? target : null
  const ok = new Set(rows.map((r) => r.h))
  while (el && !(el instanceof SVGSVGElement)) {
    const h = el.getAttribute('data-h')
    if (h && ok.has(h) && !el.closest('defs')) return h
    el = el.parentElement
  }
  return null
}

type Drag =
  | { kind: 'press'; hit: string; sel: readonly string[]; at: XY; client: XY; moving: boolean; d: XY }
  | { kind: 'box'; at: XY; client: XY; shift: boolean; showing: boolean }
  | { kind: 'none' }

export function editTool(get: () => ToolState): SheetTool {
  let drag: Drag | null = null
  const far = (e: PointerEvent, client: XY) => Math.abs(e.clientX - client[0]) + Math.abs(e.clientY - client[1]) >= SLOP_PX
  return {
    down(e, c) {
      const s = get()
      const p = c.toDrawing(e.clientX, e.clientY)
      const hit = targetHandle(e.target, s.rows) ?? pickAt(s.rows, p, PICK_PX * c.unitsPerPx())
      if (hit && e.shiftKey) {
        s.select(toggle(s.selection, hit))
        drag = { kind: 'none' }
        return true
      }
      if (hit) {
        const sel = s.selection.includes(hit) ? s.selection : [hit]
        if (sel !== s.selection) s.select(sel)
        drag = { kind: 'press', hit, sel, at: p, client: [e.clientX, e.clientY], moving: false, d: [0, 0] }
        return true
      }
      drag = { kind: 'box', at: p, client: [e.clientX, e.clientY], shift: e.shiftKey, showing: false }
      return true
    },
    move(e, c) {
      const s = get()
      if (!drag || drag.kind === 'none') return
      if (drag.kind === 'box') {
        if (!drag.showing && !far(e, drag.client)) return
        drag.showing = true
        const p = c.toDrawing(e.clientX, e.clientY)
        s.box.show({ x: drag.client[0], y: drag.client[1] }, { x: e.clientX, y: e.clientY }, boxMode(drag.at, p))
        return
      }
      if (!drag.moving) {
        if (!far(e, drag.client) || !s.canMove(drag.sel)) return
        drag.moving = true
        s.ghost.show(drag.sel)
      }
      const p = c.toDrawing(e.clientX, e.clientY)
      const k = c.unitsPerPx()
      let [dx, dy] = [p[0] - drag.at[0], p[1] - drag.at[1]]
      if (e.shiftKey) [dx, dy] = shiftLock(dx, dy)
      drag.d = [snapDelta(dx, k), snapDelta(dy, k)]
      s.ghost.at(drag.d[0], drag.d[1])
    },
    up(e, c) {
      const s = get()
      const d = drag
      drag = null
      if (!d || d.kind === 'none') return
      if (d.kind === 'box') {
        if (!d.showing) {
          if (!d.shift) s.select([])
          return
        }
        s.box.clear()
        const got = boxSelect(s.rows, d.at, c.toDrawing(e.clientX, e.clientY))
        s.select(d.shift ? [...new Set([...s.selection, ...got])] : got)
        return
      }
      if (!d.moving) {
        // A click on one of several selected: just that one.
        if (s.selection.length > 1) s.select([d.hit])
        return
      }
      const [dx, dy] = d.d
      if (!dx && !dy) return s.ghost.clear()
      void s.move(dx, dy).then((ok) => {
        // A landed move keeps its ghost until the drawing is read again.
        if (!ok) s.ghost.snapBack()
      })
    },
    cancel() {
      const s = get()
      const had = !!drag && drag.kind !== 'none' && (drag.kind === 'box' ? drag.showing : drag.moving)
      if (drag?.kind === 'box') s.box.clear()
      if (drag?.kind === 'press' && drag.moving) s.ghost.snapBack()
      drag = null
      return had
    },
  }
}
