// Where each top-level entity and each layer sits in the drawing, by handle
// and by name, in drawing coordinates: what a view address frames, and where
// a changed block's placements are outlined. Built alongside the drawing.

import type { Bounds } from '../dxfModel'

export type DrawingIndex = {
  /** Upper-case handle (no `@h:`) → the drawn entity's box. */
  handles: Record<string, Bounds>
  /** Layer name → the box of what it draws. */
  layers: Record<string, Bounds>
  /** Layer name → whether it is frozen or off now. */
  layerState: Record<string, { frozen: boolean; off: boolean }>
}

const empty = (): Bounds => ({ minX: Infinity, minY: Infinity, maxX: -Infinity, maxY: -Infinity })
const grow = (b: Bounds, o: Bounds): void => {
  b.minX = Math.min(b.minX, o.minX)
  b.minY = Math.min(b.minY, o.minY)
  b.maxX = Math.max(b.maxX, o.maxX)
  b.maxY = Math.max(b.maxY, o.maxY)
}

/** Collects one top-level entity's points at a time. */
export class IndexBuilder {
  readonly index: DrawingIndex = { handles: {}, layers: {}, layerState: {} }
  private cur: Bounds | null = null

  begin(): void {
    this.cur = empty()
  }

  add(x: number, y: number): void {
    const c = this.cur
    if (!c) return
    if (x < c.minX) c.minX = x
    if (x > c.maxX) c.maxX = x
    if (y < c.minY) c.minY = y
    if (y > c.maxY) c.maxY = y
  }

  /** The entity drew: file its box under its handle and its layer. */
  end(handle: unknown, layer: string, drew: boolean): void {
    const c = this.cur
    this.cur = null
    if (!c || !drew || !Number.isFinite(c.minX) || !Number.isFinite(c.minY)) return
    if (typeof handle === 'string' && handle) this.index.handles[handle.toUpperCase()] = c
    grow((this.index.layers[layer] ??= empty()), c)
  }

  layer(name: string, frozen: boolean, off: boolean): void {
    this.index.layerState[name] = { frozen, off }
  }
}
