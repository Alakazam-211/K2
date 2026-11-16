// Off-screen people on a live text (build-editors.md E4b; editors-notes.md v3
// Spacing): someone whose caret is past the top or the bottom of the view gets
// a small muted chip in a 34px band at that edge, centred over the column
// (where the timeline puts "More lanes below"). The band takes its own space,
// so a chip never sits on text. Clicking a chip jumps to that person. You,
// away people and the person you follow get none (the view is on them).
// Pure: the editor measures the lines; this sorts.

/** The band's height. */
export const BAND_H = 34
/** A caret this close past an edge still counts as in view. */
const ALLOW = 8

export interface PlacedPerson {
  id: string
  /** Their caret line's top and bottom in the document's coordinates; null: no caret. */
  top: number | null
  bottom: number | null
  self?: boolean
  away?: boolean
}

export interface Offscreen {
  above: string[]
  below: string[]
}

export const NO_OFFSCREEN: Offscreen = { above: [], below: [] }

export function offscreen(people: readonly PlacedPerson[], view: { top: number; bottom: number }, o: { following: string | null }): Offscreen {
  const above: string[] = []
  const below: string[] = []
  for (const p of people) {
    if (p.self || p.away || p.id === o.following || p.top === null || p.bottom === null) continue
    if (p.bottom < view.top + ALLOW) above.push(p.id)
    else if (p.top > view.bottom - ALLOW) below.push(p.id)
  }
  return { above, below }
}

export const sameOffscreen = (a: Offscreen, b: Offscreen): boolean => a.above.join('\n') === b.above.join('\n') && a.below.join('\n') === b.below.join('\n')

/** Where the band centres its chips: over the column, else the editor's middle. */
export const bandCentre = (column: { left: number; right: number } | null, width: number): number => Math.round(column ? (column.left + column.right) / 2 : width / 2)

/** The scroll top that puts a line a third of the way down the view. */
export const jumpTop = (lineTop: number, viewHeight: number): number => Math.max(0, Math.round(lineTop - viewHeight / 3))
