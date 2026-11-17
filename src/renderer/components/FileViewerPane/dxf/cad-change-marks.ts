// The outlines a change draws, in drawing coordinates:
//   - each changed model-space entity on its box after the change, or before
//     it for a removal, in its author's presence colour;
//   - a move also outlines where it was, quietly, with a line from old to new;
//   - an approximate box (text, splines) is dashed; an entity with no box is
//     counted but not drawn;
//   - a changed block outlines each of its placements, from the drawing;
//     entities inside the block definition are never drawn (their boxes are
//     in the block's own coordinates);
//   - rows paired across an outside save are quiet; a refused pairing draws
//     nothing at all; layer changes draw nothing;
//   - your own change (SELF_PID) is drawn in your own accent, and takes no
//     presence slot.

import { NO_BLUE, presenceSlots, type PresenceSlot } from '@/lib/live-view/shared/presence-slots'
import type { DxfDrawing } from '../dxfModel'
import type { CadChange, CadEntityRow, ChangeBox } from './cad-change'

/** The author of your own edits in this viewer. */
export const SELF_PID = 'k2:you'

export type ChangeMark = {
  address: string
  box: ChangeBox
  slot: PresenceSlot
  /** Yours: drawn in your own accent. */
  self?: true
  dashed: boolean
  quiet: boolean
  /** Where a moved entity was. */
  was: boolean
}

export type ChangeLink = { from: [number, number]; to: [number, number]; slot: PresenceSlot; self?: true }

export type ChangeMarks = {
  marks: ChangeMark[]
  links: ChangeLink[]
  /** Everything changed, drawn or not: entity rows, rows past the page, blocks. */
  counted: number
}

/** Each author's pid, in the order they first appear. */
export function changeAuthors(c: CadChange): string[] {
  const out: string[] = []
  const see = (pid: string | null) => {
    if (pid && !out.includes(pid)) out.push(pid)
  }
  for (const v of c.versions) see(v.by.pid)
  for (const r of c.entities) see(r.by)
  return out
}

/** Each author's presence slot: kept while the drawing is open, never the viewer's own blue. */
export const changeSlots = (prev: ReadonlyMap<string, PresenceSlot>, authors: readonly string[]): Map<string, PresenceSlot> =>
  presenceSlots(
    prev,
    authors.filter((a) => a !== SELF_PID),
    NO_BLUE,
  )

const centre = (b: ChangeBox): [number, number] => [(b[0] + b[2]) / 2, (b[1] + b[3]) / 2]
const same = (a: ChangeBox, b: ChangeBox): boolean => a.every((v, i) => v === b[i])

/** Who changed a block: the last block edit in the range, else the last version's author. */
function blockAuthor(c: CadChange): string | null {
  const block = [...c.versions].reverse().find((v) => v.verb.startsWith('block'))
  return (block ?? c.versions[c.versions.length - 1])?.by.pid ?? null
}

export function changeMarks(c: CadChange, slots: ReadonlyMap<string, PresenceSlot>, drawing: Pick<DxfDrawing, 'index'>): ChangeMarks {
  const marks: ChangeMark[] = []
  const links: ChangeLink[] = []
  if (c.pairing === 'refused') return { marks, links, counted: 0 }
  const slotOf = (pid: string | null): PresenceSlot => (pid ? slots.get(pid) : undefined) ?? NO_BLUE[0]
  const model = c.entities.filter((r) => r.owner === 'model')
  const counted = c.entities.filter((r) => !r.owner.startsWith('@h:')).length + c.entitiesMore + c.blocks.length

  const mark = (r: CadEntityRow, box: ChangeBox, was: boolean): ChangeMark => ({
    address: r.address,
    box,
    slot: slotOf(r.by),
    dashed: r.quality === 'approx',
    quiet: r.doubt,
    was,
    ...(r.by === SELF_PID ? { self: true as const } : {}),
  })
  for (const r of model) {
    if (r.quality === 'none') continue
    if (r.op === 'removed') {
      if (r.before) marks.push(mark(r, r.before, false))
      continue
    }
    if (r.op === 'changed' && r.before && r.after && r.fields.includes('geom') && !same(r.before, r.after)) {
      marks.push(mark(r, r.before, true), mark(r, r.after, false))
      links.push({ from: centre(r.before), to: centre(r.after), slot: slotOf(r.by), ...(r.by === SELF_PID ? { self: true as const } : {}) })
      continue
    }
    const box = r.after ?? r.before
    if (box) marks.push(mark(r, box, false))
  }

  const blockSlot = slotOf(blockAuthor(c))
  for (const b of c.blocks) {
    for (const address of b.inserts) {
      const at = drawing.index.handles[address.replace(/^@h:/i, '').toUpperCase()]
      if (at) marks.push({ address, box: [at.minX, at.minY, at.maxX, at.maxY], slot: blockSlot, dashed: false, quiet: false, was: false })
    }
  }
  return { marks, links, counted }
}

/** Everything the change outlines, as one box (for Show), or null. */
export function marksBox(m: ChangeMarks): ChangeBox | null {
  if (!m.marks.length) return null
  return m.marks.reduce<ChangeBox>(
    (a, k) => [Math.min(a[0], k.box[0]), Math.min(a[1], k.box[1]), Math.max(a[2], k.box[2]), Math.max(a[3], k.box[3])],
    [Infinity, Infinity, -Infinity, -Infinity],
  )
}
