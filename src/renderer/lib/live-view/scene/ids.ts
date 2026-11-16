// What is under a point of the picture: K2 Render's object-id image
// (`layer=ids`, a PNG at the still's size, 8-bit RGB, each pixel
// `r << 16 | g << 8 | b`). 0 is the background; any other value v is the
// object at index v − 1, which the still's `ids.objects` table names, and
// whose container's locator is `o<v − 1>` (crates/k2-daemon/src/docs/
// preview/extras.rs). Index 0 is a real object (value 1): never read 0 as
// "transparent" or as index 0. The still's alpha is the transparency.
// S4 uses it for hover outlines; click-to-pick is S9.

/** One object in the id table: its index in the pixels, and its id. */
export interface IdEntry {
  index: number
  id: string
}

/** A decoded id image and its table. */
export interface IdLayer {
  width: number
  height: number
  /** One value per pixel, row by row: 0 background, else index + 1. */
  values: ArrayLike<number>
  /** The objects in the image, ascending by index (may be cut short). */
  objects: readonly IdEntry[]
}

/** A container's locator for an index (K2 Render's `o<index>`). */
export const locatorOf = (index: number): string => `o${index}`

/** The index of the object at pixel (x, y) of the still, or null for the background or outside it. */
export function idIndexAt(layer: IdLayer, x: number, y: number): number | null {
  const px = Math.floor(x)
  const py = Math.floor(y)
  if (!(px >= 0 && py >= 0 && px < layer.width && py < layer.height)) return null
  const v = layer.values[py * layer.width + px]
  return v > 0 ? v - 1 : null
}

/** The table's entry for an index (binary search: the table is ascending). */
export function entryOf(layer: IdLayer, index: number): IdEntry | null {
  const t = layer.objects
  let lo = 0
  let hi = t.length - 1
  while (lo <= hi) {
    const mid = (lo + hi) >> 1
    if (t[mid].index === index) return t[mid]
    if (t[mid].index < index) lo = mid + 1
    else hi = mid - 1
  }
  return null
}

/** The id of the object at pixel (x, y), or null (background, outside, or an index the table left out). */
export function idAt(layer: IdLayer, x: number, y: number): string | null {
  const i = idIndexAt(layer, x, y)
  return i === null ? null : (entryOf(layer, i)?.id ?? null)
}

/** The same, from a point of the picture as shown: fractions of its width and height. */
export const idAtFraction = (layer: IdLayer, fx: number, fy: number): string | null => idAt(layer, fx * layer.width, fy * layer.height)

/** RGBA pixels of the id PNG (as a canvas reads them back) to values. Alpha is ignored. */
export function decodeIdPixels(rgba: ArrayLike<number>, width: number, height: number): Uint32Array {
  const out = new Uint32Array(width * height)
  for (let i = 0; i < out.length; i++) out[i] = (rgba[i * 4] << 16) | (rgba[i * 4 + 1] << 8) | rgba[i * 4 + 2]
  return out
}

/** The box (in still pixels) an object covers, or null when it isn't drawn. */
export function extentOf(layer: IdLayer, index: number): { x0: number; y0: number; x1: number; y1: number } | null {
  const want = index + 1
  let x0 = layer.width
  let y0 = layer.height
  let x1 = -1
  let y1 = -1
  for (let y = 0; y < layer.height; y++)
    for (let x = 0; x < layer.width; x++) {
      if (layer.values[y * layer.width + x] !== want) continue
      if (x < x0) x0 = x
      if (x > x1) x1 = x
      if (y < y0) y0 = y
      if (y > y1) y1 = y
    }
  return x1 < 0 ? null : { x0, y0, x1, y1 }
}
