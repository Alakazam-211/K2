// Name flags on a live text (editors-notes.md §8 "Spacing: nothing covers
// text", v3.2b): each other person's caret gets a flag with their name (and
// K2's agent glyph for an agent) in the right margin, at the caret's line,
// with a dashed hairline back to the caret. A flag never covers a glyph:
//
//   - it starts GAP px right of the reading column's edge (Write and Read), or
//     right of the last glyph on the lines its own height spans (Source and
//     code, which have no column);
//   - two flags at one height sit side by side, never on each other;
//   - when the margin can't hold the name, the flag shrinks to the initial;
//   - when it can't hold that either (the Thread's 820px column in a narrow
//     tab), it falls back to the shared editors' name tag: a small chip just
//     above the caret line, shown for TAG_MS after that caret moves or types
//     and while the pointer is over the caret or its selection (TagClock),
//     never taking the pointer. Margin flags win whenever they fit;
//   - a caret half out of view gets no flag until its line is in view.
//
// Pure: the layer measures (positions in the layer's coordinates, widths of
// the words) and draws what this returns.

/** A flag's height, and its gap from the column or the text. */
export const FLAG_H = 20
export const FLAG_GAP = 12
/** Room kept between a flag and the edge that would clip it. */
export const FLAG_EDGE = 6
/** The initial's flag: a square. */
export const FLAG_MINI_W = 20
/** The fallback caret tag's height, and how long it stays after its caret moves. */
export const TAG_H = 18
export const TAG_MS = 2000

export interface FlagCaret {
  id: string
  name: string
  agent: boolean
  /** The caret's left edge and its line box, in the layer's coordinates. */
  x: number
  top: number
  bottom: number
}

export interface FlagRoom {
  /** The reading column's right edge (Write and Read); null: no column (Source, code). */
  column: number | null
  /** The visible box flags must stay wholly inside. */
  clip: { left: number; right: number; top: number; bottom: number }
  /** The rightmost glyph edge on the text between `top` and `bottom` (else -Infinity). */
  textRight: (top: number, bottom: number) => number
  /** A label's width as drawn (padding and glyph included). */
  width: (label: string, agent: boolean) => number
}

export interface Flag {
  id: string
  /** The full name, or its initial when the margin is tight. */
  label: string
  name: string
  agent: boolean
  mini: boolean
  /** The fallback: a passing name tag over the caret line, not a margin flag. */
  tag: boolean
  left: number
  top: number
  width: number
  /** The hairline from the caret to the flag, along the line's top (none for a tag). */
  line: { x0: number; x1: number; y: number } | null
}

export function placeFlags(carets: readonly FlagCaret[], room: FlagRoom): Flag[] {
  const out: Flag[] = []
  const sorted = [...carets].sort((a, b) => a.top - b.top || a.x - b.x || (a.id < b.id ? -1 : 1))
  for (const c of sorted) {
    const mid = (c.top + c.bottom) / 2
    const top = Math.round(mid - FLAG_H / 2)
    // A line half out of view: no flag until it's in view.
    if (c.top < room.clip.top || c.bottom > room.clip.bottom || top < room.clip.top || top + FLAG_H > room.clip.bottom) continue
    let edge = Math.max(room.column ?? -Infinity, room.textRight(top, top + FLAG_H), c.x)
    // A flag already at this height: sit after it.
    for (const f of out) if (f.top < top + FLAG_H + 2 && top < f.top + FLAG_H + 2) edge = Math.max(edge, f.left + f.width + 4 - FLAG_GAP)
    const left = Math.round(edge + FLAG_GAP)
    const limit = room.clip.right - FLAG_EDGE
    const full = room.width(c.name, c.agent)
    const initial = (c.name.trim()[0] ?? '?').toUpperCase()
    let flag: Pick<Flag, 'label' | 'mini' | 'width'> | null = null
    if (left + full <= limit) flag = { label: c.name, mini: false, width: full }
    else if (left + FLAG_MINI_W <= limit) flag = { label: initial, mini: true, width: FLAG_MINI_W }
    if (flag) {
      out.push({ id: c.id, name: c.name, agent: c.agent, left, top, tag: false, ...flag, line: { x0: Math.round(c.x + 1), x1: left, y: Math.round(c.top) } })
      continue
    }
    // No room even for the initial: the name tag just above the caret line (under it on the top line), kept whole in view.
    const w = Math.min(full, Math.max(FLAG_MINI_W, limit - room.clip.left))
    const tagLeft = Math.round(Math.max(room.clip.left, Math.min(c.x, limit - w)))
    const above = Math.round(c.top - TAG_H - 1)
    const tagTop = above >= room.clip.top ? above : Math.round(c.bottom + 1)
    out.push({ id: c.id, name: c.name, agent: c.agent, left: tagLeft, top: tagTop, tag: true, label: c.name, mini: false, width: w, line: null })
  }
  return out
}

/** Two boxes share a point (a flag on a glyph). */
export const touches = (a: { left: number; top: number; right: number; bottom: number }, b: { left: number; top: number; right: number; bottom: number }): boolean =>
  a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom

/** When each caret last moved, and which one the pointer is over: whether its tag shows. */
export class TagClock {
  private readonly at = new Map<string, { pos: number; moved: number }>()
  private hovered: string | null = null

  /** The caret is at `pos` now; a new place (a move or typing) restarts its time. */
  observe(id: string, pos: number, now: number): void {
    const prev = this.at.get(id)
    if (!prev || prev.pos !== pos) this.at.set(id, { pos, moved: now })
  }

  /** The pointer is over this person's caret or selection (null: none). True when it changed. */
  hover(id: string | null): boolean {
    if (this.hovered === id) return false
    this.hovered = id
    return true
  }

  visible(id: string, now: number): boolean {
    if (this.hovered === id) return true
    const a = this.at.get(id)
    return !!a && now - a.moved < TAG_MS
  }

  /** The next time a shown tag goes (to redraw then), or null. */
  nextExpiry(now: number): number | null {
    let next: number | null = null
    for (const a of this.at.values()) {
      const end = a.moved + TAG_MS
      if (end > now && (next === null || end < next)) next = end
    }
    return next
  }

  forget(present: ReadonlySet<string>): void {
    for (const id of [...this.at.keys()]) if (!present.has(id)) this.at.delete(id)
  }
}
