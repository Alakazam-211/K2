// Next / previous change across one or many stacked diffs. Pure: the
// viewer turns each file's merge chunks into stops and asks where to go.

/** One place F7 can land: a changed chunk in file `file` (index in the
 *  stacked list) starting at doc position `pos`. `pending` = the file has
 *  not rendered yet; landing on it scrolls the file into view (which loads
 *  it) instead of moving a cursor. */
export interface ChangeStop {
  file: number
  pos: number
  pending?: boolean
}

/** Where the user is: the file holding focus (or last navigated) and the
 *  cursor position inside it. `null` = nowhere yet (start of the list). */
export interface ChangeCursor {
  file: number
  pos: number
}

/** What a file contributes to the stop list. */
export type FileStops =
  | { state: 'chunks'; starts: readonly number[] }
  | { state: 'pending' }
  | { state: 'none' }

/** Flatten per-file chunk starts into ordered stops. Files that can't have
 *  a change to land on (binary, too large, no text change) add nothing. */
export function buildStops(files: readonly FileStops[]): ChangeStop[] {
  const stops: ChangeStop[] = []
  files.forEach((f, file) => {
    if (f.state === 'pending') stops.push({ file, pos: 0, pending: true })
    else if (f.state === 'chunks') {
      const sorted = [...f.starts].sort((a, b) => a - b)
      for (const pos of sorted) stops.push({ file, pos })
    }
  })
  return stops
}

function before(a: ChangeCursor, b: ChangeCursor): boolean {
  return a.file < b.file || (a.file === b.file && a.pos < b.pos)
}

/**
 * The stop to go to from `cursor` in direction `dir` (+1 next, -1 previous),
 * wrapping at either end. Returns the index into `stops`, or -1 when there
 * are no stops.
 */
export function stepChange(stops: readonly ChangeStop[], cursor: ChangeCursor | null, dir: 1 | -1): number {
  if (stops.length === 0) return -1
  if (!cursor) return dir > 0 ? 0 : stops.length - 1
  if (dir > 0) {
    const i = stops.findIndex((s) => before(cursor, s))
    return i === -1 ? 0 : i
  }
  for (let i = stops.length - 1; i >= 0; i--) {
    if (before(stops[i], cursor)) return i
  }
  return stops.length - 1
}

/** 1-based "change N of M" for the toolbar: the last stop at or before the
 *  cursor. 0 when the cursor is before the first stop (or nowhere). */
export function currentChangeNumber(stops: readonly ChangeStop[], cursor: ChangeCursor | null): number {
  if (!cursor) return 0
  let n = 0
  for (let i = 0; i < stops.length; i++) {
    if (before(cursor, stops[i])) break
    n = i + 1
  }
  return n
}
