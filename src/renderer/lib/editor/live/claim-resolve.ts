// Placing a hold on the text by the version its place was computed at (K2
// Cores: the kit anchors every claim to the text; a claim_started's `lines`
// are at its `version`). The editor's text moves on, so:
//   - at the text's own version: the lines (or a byte range, UTF-8, when the
//     kit sends one) are read in the text as it is;
//   - from an older version: read in the text as it was then (rebuilt from
//     the ring's inverse changes) and mapped forward through the changes since;
//   - from a newer version than the text: held until the text gets there;
//   - from a version fallen out of the ring: the lines in the text as it is
//     now, with a debug note.
// Once placed, the hold maps through later changes with CodeMirror's own
// mapping (hold-field.ts); it is never placed again by line numbers.

import { ChangeSet, type ChangeDesc, type Text } from '@codemirror/state'

interface Entry {
  /** The text's version before and after this change (equal for your own unacknowledged edit). */
  before: number
  after: number
  changes: ChangeSet
  inverse: ChangeSet
}

/** The last `size` changes to the editor's text, keyed by the version they moved it from and to. */
export class RevRing {
  private entries: Entry[] = []
  /** The oldest version the ring can rebuild the text at. */
  private floor: number
  private current: number

  constructor(
    private readonly size: number,
    version: number,
  ) {
    this.floor = version
    this.current = version
  }

  /** The text's version now. */
  get rev(): number {
    return this.current
  }

  /** The text moved on without a change to it (an acknowledgement of your own edit). */
  setRev(version: number): void {
    this.current = Math.max(this.current, version)
  }

  /** A change to the text: from version `before` to `after`, with its inverse (against the text before). */
  record(before: number, after: number, changes: ChangeSet, inverse: ChangeSet): void {
    this.entries.push({ before, after, changes, inverse })
    this.current = Math.max(this.current, after)
    while (this.entries.length > this.size) this.floor = this.entries.shift()!.after
  }

  /** A fresh text (a snapshot reload): nothing to map from. */
  reset(version: number): void {
    this.entries = []
    this.floor = version
    this.current = version
  }

  /** The text as it was at `version`, and the changes from then to now; null when it's out of reach. */
  since(doc: Text, version: number): { then: Text; forward: ChangeDesc } | null {
    if (version < this.floor) return null
    // The changes made once the text was at `version` (yours made earlier aren't in it).
    const after = this.entries.filter((e) => e.before >= version || e.after > version)
    let then = doc
    for (let i = after.length - 1; i >= 0; i--) then = after[i].inverse.apply(then)
    const forward = after.reduce<ChangeDesc>((acc, e) => acc.composeDesc(e.changes.desc), ChangeSet.empty(then.length).desc)
    return { then, forward }
  }
}

/** A UTF-8 byte offset of `doc`'s text as a position (UTF-16 units), on a character boundary. */
export function utf8ToPos(doc: Text, byte: number): number {
  const enc = new TextEncoder()
  let bytes = 0
  let pos = 0
  for (const ch of doc.sliceString(0)) {
    if (bytes >= byte) break
    bytes += enc.encode(ch).length
    pos += ch.length
  }
  return pos
}

export interface ClaimPlace {
  /** The version its place was computed at (the event's `version`). */
  at: number
  /** Its lines then (1-based). */
  first: number
  last: number
  /** Its UTF-8 byte range then, when the kit sends one. */
  bytes?: { start: number; end: number }
}

const linesRange = (doc: Text, first: number, last: number): { from: number; to: number } | null => {
  if (first > doc.lines) return null
  return { from: doc.line(Math.max(1, first)).from, to: doc.line(Math.min(Math.max(first, last), doc.lines)).to }
}

const placeIn = (doc: Text, c: ClaimPlace): { from: number; to: number } | null =>
  c.bytes ? { from: utf8ToPos(doc, c.bytes.start), to: utf8ToPos(doc, Math.max(c.bytes.start, c.bytes.end)) } : linesRange(doc, c.first, c.last)

/** Where a claim sits in `doc` now; 'hold' while the text hasn't reached its version. */
export function resolveClaim(c: ClaimPlace, doc: Text, ring: RevRing): { from: number; to: number } | 'hold' | null {
  if (c.at > ring.rev) return 'hold'
  if (c.at === ring.rev) return placeIn(doc, c)
  const s = ring.since(doc, c.at)
  if (!s) {
    console.debug(`[live-doc] a claim at version ${c.at} fell out of the last changes kept: placed by its lines`)
    return linesRange(doc, c.first, c.last)
  }
  const then = placeIn(s.then, c)
  if (!then) return null
  const from = s.forward.mapPos(then.from, -1)
  return { from, to: Math.max(from, s.forward.mapPos(then.to, 1)) }
}
