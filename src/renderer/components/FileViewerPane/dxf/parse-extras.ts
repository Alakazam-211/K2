// Group codes dxf-parser reads past without keeping. Each entity handler is
// wrapped so it sees the groups go by and records, on the entity:
//   - `trueColor`: group 420 was present (dxf-parser folds it into `color`,
//     so "420 with no 62" and "62 only" look the same afterwards);
//   - `weights`: a SPLINE's group 41s (rational splines; dropped entirely).

import type DxfParser from 'dxf-parser'
import type { IEntity } from 'dxf-parser'

type Group = { code: number; value: unknown }
type Scanner = { next(): Group; _pointer?: number }
type Handler = { ForEntityName: string; parseEntity(scanner: Scanner, curr: Group): IEntity }

export type EntityExtras = {
  /** Group 420, 0xRRGGBB. */
  trueColor?: number
  /** SPLINE group 41, one per control point. */
  weights?: number[]
}

const kept = (code: number, entity: string): boolean => code === 420 || (code === 41 && entity === 'SPLINE')

export function readExtras(parser: DxfParser): void {
  const handlers = (parser as unknown as { _entityHandlers?: Record<string, Handler> })._entityHandlers
  if (!handlers) return
  for (const [name, inner] of Object.entries(handlers)) {
    handlers[name] = {
      ForEntityName: inner.ForEntityName,
      parseEntity(scanner, curr) {
        // Keyed by position: parsePoint rewinds and re-reads a group.
        const seen = new Map<number, Group>()
        const own = scanner.next
        let tick = 0
        let embedded = false
        scanner.next = function next(this: Scanner) {
          const at = typeof this._pointer === 'number' ? this._pointer : -++tick
          const g = own.call(this)
          // 101: an embedded object runs to the entity's end; its codes mean other things.
          if (g.code === 101) embedded = true
          if (!embedded && kept(g.code, name)) seen.set(at, g)
          return g
        }
        let e: IEntity & EntityExtras
        try {
          e = inner.parseEntity(scanner, curr)
        } finally {
          scanner.next = own
        }
        const weights: number[] = []
        for (const [, g] of [...seen.entries()].sort((a, b) => a[0] - b[0])) {
          if (typeof g.value !== 'number' || !Number.isFinite(g.value)) continue
          if (g.code === 420) e.trueColor = g.value & 0xffffff
          else weights.push(g.value)
        }
        if (weights.length) e.weights = weights
        return e
      },
    }
  }
}
