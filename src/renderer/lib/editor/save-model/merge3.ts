// The plain-file three-way merge (build-editors.md §3; notes v3.1): a save
// outside K2 landed while you had unsaved words. base = the text K2 last read,
// theirs = the file now, yours = the buffer. The kit's rule for live text,
// line by line:
//   - a change only one side made lands;
//   - where both sides changed the same lines (and differently), BOTH are
//     kept, whole, yours first, for you to tidy. Nothing is dropped.
// It reports each kept pair as line ranges in the merged text, so the editor
// can mark them ("Kept both · yours" / "Kept both · saved outside K2").
//
// Lines are compared whole: each distinct line becomes one character and
// @codemirror/merge's `diff` (Myers, already in the bundle) diffs those.

import { diff } from '@codemirror/merge'

/** A half-open range of 0-based line numbers in the merged text. */
export interface LineRange {
  from: number
  to: number
}

export interface KeptPair {
  yours: LineRange
  theirs: LineRange
}

export interface Merge3Result {
  text: string
  /** Where both changed the same lines: both kept, yours then theirs. */
  kept: KeptPair[]
}

interface Hunk {
  /** Base lines [bFrom, bTo) became side lines [sFrom, sTo). */
  bFrom: number
  bTo: number
  sFrom: number
  sTo: number
}

const splitLines = (s: string): string[] => s.split('\n')

/** Skip the surrogate range so every line is one UTF-16 unit `diff` won't split. */
function codeFor(i: number): string {
  const c = 0x100 + i
  return String.fromCharCode(c >= 0xd800 ? c + 0x800 : c)
}

/** Line-level hunks from `a` to `b`, in order. */
function lineHunks(a: string[], b: string[], codes: Map<string, string>): Hunk[] {
  const enc = (lines: string[]): string =>
    lines
      .map((l) => {
        let c = codes.get(l)
        if (c === undefined) {
          c = codeFor(codes.size)
          codes.set(l, c)
        }
        return c
      })
      .join('')
  return diff(enc(a), enc(b)).map((ch) => ({ bFrom: ch.fromA, bTo: ch.toA, sFrom: ch.fromB, sTo: ch.toB }))
}

/** Too many distinct lines for one code each: one hunk for the whole file. */
const MAX_DISTINCT = 0xffff - 0x100 - 0x800

function wholeHunk(a: string[], b: string[]): Hunk[] {
  const same = a.length === b.length && a.every((l, i) => l === b[i])
  return same ? [] : [{ bFrom: 0, bTo: a.length, sFrom: 0, sTo: b.length }]
}

/** Do two hunks (in base lines) touch the same lines? An insertion at the
 *  edge of the other's range doesn't; two insertions at one place do. */
function overlaps(x: Hunk, y: Hunk): boolean {
  const xe = x.bFrom === x.bTo
  const ye = y.bFrom === y.bTo
  if (xe && ye) return x.bFrom === y.bFrom
  if (xe) return x.bFrom > y.bFrom && x.bFrom < y.bTo
  if (ye) return y.bFrom > x.bFrom && y.bFrom < x.bTo
  return Math.max(x.bFrom, y.bFrom) < Math.min(x.bTo, y.bTo)
}

/** The side's lines for base [from, to), given its hunks inside that span. */
function sideText(base: string[], side: string[], hunks: Hunk[], from: number, to: number): string[] {
  const out: string[] = []
  let at = from
  for (const h of hunks) {
    out.push(...base.slice(at, h.bFrom), ...side.slice(h.sFrom, h.sTo))
    at = h.bTo
  }
  out.push(...base.slice(at, to))
  return out
}

const sameLines = (a: string[], b: string[]): boolean => a.length === b.length && a.every((l, i) => l === b[i])

export function merge3(base: string, theirs: string, yours: string): Merge3Result {
  if (yours === base || yours === theirs) return { text: theirs, kept: [] }
  if (theirs === base) return { text: yours, kept: [] }
  const B = splitLines(base)
  const T = splitLines(theirs)
  const Y = splitLines(yours)
  const codes = new Map<string, string>()
  const distinct = new Set([...B, ...T, ...Y]).size
  const hy = distinct > MAX_DISTINCT ? wholeHunk(B, Y) : lineHunks(B, Y, codes)
  const ht = distinct > MAX_DISTINCT ? wholeHunk(B, T) : lineHunks(B, T, codes)

  const out: string[] = []
  const kept: KeptPair[] = []
  let at = 0 // the next base line not yet written
  let i = 0
  let j = 0
  // Base order; at one place an insertion comes before a change.
  const before = (x: Hunk, y: Hunk): boolean => x.bFrom < y.bFrom || (x.bFrom === y.bFrom && x.bFrom === x.bTo && y.bFrom !== y.bTo)
  while (i < hy.length || j < ht.length) {
    const takeYours = j >= ht.length || (i < hy.length && !before(ht[j], hy[i]))
    // A group: the next hunk and every hunk, from either side, that overlaps it.
    const gy: Hunk[] = []
    const gt: Hunk[] = []
    const span: Hunk = { ...(takeYours ? hy[i] : ht[j]) }
    if (takeYours) gy.push(hy[i++])
    else gt.push(ht[j++])
    const grow = (list: Hunk[], k: number, into: Hunk[]): number => {
      while (k < list.length && overlaps(list[k], span)) {
        into.push(list[k])
        span.bFrom = Math.min(span.bFrom, list[k].bFrom)
        span.bTo = Math.max(span.bTo, list[k].bTo)
        k++
      }
      return k
    }
    for (;;) {
      const i0 = i
      const j0 = j
      i = grow(hy, i, gy)
      j = grow(ht, j, gt)
      if (i === i0 && j === j0) break
    }
    const from = span.bFrom
    const to = span.bTo
    out.push(...B.slice(at, from))
    at = to
    if (gy.length === 0) out.push(...sideText(B, T, gt, from, to))
    else if (gt.length === 0) out.push(...sideText(B, Y, gy, from, to))
    else {
      const ys = sideText(B, Y, gy, from, to)
      const ts = sideText(B, T, gt, from, to)
      if (sameLines(ys, ts)) out.push(...ys)
      else {
        const y0 = out.length
        out.push(...ys)
        const t0 = out.length
        out.push(...ts)
        kept.push({ yours: { from: y0, to: t0 }, theirs: { from: t0, to: out.length } })
      }
    }
  }
  out.push(...B.slice(at))
  return { text: out.join('\n'), kept }
}
