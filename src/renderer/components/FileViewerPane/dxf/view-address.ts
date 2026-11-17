// Opening a drawing at a view address: the box to frame for
//   - `@model:fit`: the whole drawing;
//   - `@h:<HANDLE>`: one drawn entity (an INSERT is one entity);
//   - `@layer:<name>`: what a layer draws (the name percent-encoded where it
//     has spaces, `/` or `%`);
//   - a set: several of these, as a list or separated by spaces or commas.
// An address the drawing has no box for adds nothing; with nothing found the
// caller fits the whole drawing.

import type { Bounds, DxfDrawing } from '../dxfModel'

function decode(s: string): string {
  try {
    return decodeURIComponent(s)
  } catch {
    return s
  }
}

function one(address: string, d: Pick<DxfDrawing, 'bounds' | 'index'>): Bounds | null {
  const m = /^@([a-z][a-z0-9_-]*):(.+)$/i.exec(address.trim())
  if (!m) return null
  const kind = m[1].toLowerCase()
  if (kind === 'model') return d.bounds
  if (kind === 'h') return d.index.handles[m[2].toUpperCase()] ?? null
  if (kind === 'layer') return d.index.layers[decode(m[2])] ?? null
  return null
}

/** A box worth framing: one smaller than 2% of the drawing (a point, a dot) grows to that, about its centre. */
export function framed(b: Bounds, whole: Bounds | null): Bounds {
  const min = whole ? 0.02 * Math.max(whole.maxX - whole.minX, whole.maxY - whole.minY) : 0
  if (Math.max(b.maxX - b.minX, b.maxY - b.minY) >= min) return b
  const cx = (b.minX + b.maxX) / 2
  const cy = (b.minY + b.maxY) / 2
  return { minX: cx - min / 2, minY: cy - min / 2, maxX: cx + min / 2, maxY: cy + min / 2 }
}

export function viewBounds(view: string | readonly string[] | null | undefined, d: Pick<DxfDrawing, 'bounds' | 'index'>): Bounds | null {
  if (!view) return null
  const addresses = (typeof view === 'string' ? view.split(/[\s,]+/) : view).filter(Boolean)
  let out: Bounds | null = null
  for (const a of addresses) {
    const b = one(a, d)
    if (!b) continue
    out = out
      ? { minX: Math.min(out.minX, b.minX), minY: Math.min(out.minY, b.minY), maxX: Math.max(out.maxX, b.maxX), maxY: Math.max(out.maxY, b.maxY) }
      : { ...b }
  }
  return out
}
