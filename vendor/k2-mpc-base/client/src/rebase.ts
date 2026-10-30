// The live-edit rebase (amendment E-LIVE): a line-for-line port of the kit's
// `session/rebase.rs` (`map_point`, `map_range`, `rebase_ours`, `rebase_incoming`), without
// its planted mutants. The server (`Session::edit_at_base`) and this client apply the same
// rules, so both compute the same text; the shared vectors (`test/vectors/live_rebase.json`,
// written by the Rust kit) pin it.
//
// - `rebaseOurs` (the server's role, and the client's for its own pending change): ours was
//   made later, so at a tie ours goes after theirs; competing text refuses (rule 4 (b)).
// - `rebaseIncoming` (the client mirror): an incoming server change carried through the
//   client's own pending change; at a tie the incoming one goes before. Never refuses.

import { coalesce, utf8Len, type Hunk } from "./hunk";

/** Where a rebase found competing text (rule 4 (b)), as the kit's `LiveConflict`. */
export interface LiveConflict {
  /** Which of ours: its index among our hunks as carried to that step. */
  ours: number;
  /** Our hunk's deleted range (or insertion point) where it met theirs, before theirs. */
  start: number;
  end: number;
  insert: string;
  /** Theirs: the step (index of the change, from 0) and the hunk's range. */
  step: number;
  theirsStart: number;
  theirsEnd: number;
  why: "replacement" | "insert_inside_deleted";
}

export type RebaseResult = { ok: Hunk[] } | { conflict: LiveConflict };

interface Piece {
  at: number;
  /** r2.3 S3: the insertion point's other tie-break placement (left bias at every tie), so
   * an insertion conflicts as "strictly inside" only when it is under both placements. */
  lo: number;
  insert: string;
  dels: Array<[number, number]>;
  replacement: boolean;
  index: number;
}

/**
 * Where an insertion point `at` (of the text before `theirs`) goes when `theirs` (canonical)
 * is applied. `after`: at a tie, ours goes after theirs' inserted text; else before it. A
 * point strictly inside a range theirs deleted goes after theirs' inserted text there.
 */
export function mapPoint(at: number, theirs: readonly Hunk[], after: boolean): number {
  let delta = 0;
  for (const t of theirs) {
    const ts = t.start;
    const te = t.end;
    const j = utf8Len(t.insert);
    if (at < ts) break;
    if (ts === te) {
      if (at === ts && !after) break;
      delta += j;
      continue;
    }
    if (at === ts) return after ? ts + delta + j : ts + delta;
    if (at < te) return ts + delta + j;
    delta += j - (te - ts);
  }
  return at + delta;
}

/**
 * What survives of the deleted range `[a, b)` (of the text before `theirs`) once `theirs` is
 * applied: the characters theirs didn't delete, as ranges of the text after it, split around
 * every insertion of theirs, which they never cover.
 */
export function mapRange(a: number, b: number, theirs: readonly Hunk[]): Array<[number, number]> {
  const out: Array<[number, number]> = [];
  let delta = 0;
  let cur = a;
  for (const t of theirs) {
    const ts = t.start;
    const te = t.end;
    const grow = utf8Len(t.insert) - (te - ts);
    if (te <= cur && (ts < cur || ts === te)) {
      delta += grow;
      continue;
    }
    if (ts >= b) break;
    const start = cur + delta;
    if (ts > cur) out.push([start, Math.max(ts, cur) + delta]);
    delta += grow;
    cur = Math.max(cur, te);
  }
  if (cur < b) out.push([cur + delta, Math.max(b, cur) + delta]);
  const merged: Array<[number, number]> = [];
  for (const [s, e] of out) {
    if (s >= e) continue;
    const l = merged[merged.length - 1];
    if (l && l[1] === s) l[1] = e;
    else merged.push([s, e]);
  }
  return merged;
}

function insideDeleted(x: number, theirs: readonly Hunk[]): boolean {
  return theirs.some((t) => t.start < x && x < t.end);
}

function pieces(hunks: readonly Hunk[]): Piece[] {
  return hunks.map((h, index) => ({
    at: h.start,
    lo: h.start,
    insert: h.insert,
    dels: h.start < h.end ? [[h.start, h.end]] : [],
    replacement: h.start < h.end && h.insert.length > 0,
    index,
  }));
}

function hunksOf(ps: Piece[]): Hunk[] {
  const out: Hunk[] = [];
  for (const p of ps) {
    let k = 0;
    if (p.insert.length > 0) {
      let end = p.at;
      if (p.dels.length > 0 && p.dels[0][0] === p.at) {
        end = p.dels[0][1];
        k = 1;
      }
      out.push({ start: p.at, end, insert: p.insert });
    }
    for (; k < p.dels.length; k++) out.push({ start: p.dels[k][0], end: p.dels[k][1], insert: "" });
  }
  // stable: ties keep their order (as Rust's sort_by_key)
  const keyed = out.map((h, i) => ({ h, i }));
  keyed.sort((x, y) => {
    if (x.h.start !== y.h.start) return x.h.start - y.h.start;
    const dx = x.h.end > x.h.start ? 1 : 0;
    const dy = y.h.end > y.h.start ? 1 : 0;
    if (dx !== dy) return dx - dy;
    return x.i - y.i;
  });
  return coalesce(keyed.map((k) => k.h));
}

function carry(
  ps: Piece[],
  theirs: readonly Hunk[],
  after: boolean,
  check: number | null,
): { ok: Piece[] } | { conflict: LiveConflict } {
  const out: Piece[] = [];
  for (const p of ps) {
    if (check !== null) {
      for (const t of theirs) {
        const ts = t.start;
        const te = t.end;
        const conflict = (start: number, end: number, why: LiveConflict["why"]) => ({
          conflict: {
            ours: p.index,
            start,
            end,
            insert: p.insert,
            step: check,
            theirsStart: ts,
            theirsEnd: te,
            why,
          },
        });
        const deleting = p.dels.find(([s, e]) => Math.max(s, ts) < Math.min(e, te));
        if (ts < te) {
          if (p.replacement) {
            if (deleting) return conflict(deleting[0], deleting[1], "replacement");
          } else if (
            p.dels.length === 0 &&
            p.insert.length > 0 &&
            ts < p.at &&
            p.at < te &&
            insideDeleted(p.lo, theirs) // r2.3 S3: strictly inside under both placements
          ) {
            return conflict(p.at, p.at, "insert_inside_deleted");
          }
        }
      }
    }
    const at = mapPoint(p.at, theirs, after);
    const lo = Math.min(mapPoint(p.lo, theirs, false), at);
    const dels: Array<[number, number]> = [];
    for (const [s, e] of p.dels) dels.push(...mapRange(s, e, theirs));
    out.push({ ...p, at, lo, dels });
  }
  return { ok: out };
}

/** The gap form of carried pieces (as the kit's `gaps_of`): each insertion at its point and
 * each surviving deleted range as its own hunk, sorted; insertions at one point joined. */
function gapsOf(ps: readonly Piece[]): Hunk[] {
  const out: Hunk[] = [];
  for (const p of ps) {
    if (p.insert.length > 0) out.push({ start: p.at, end: p.at, insert: p.insert });
    for (const [s, e] of p.dels) out.push({ start: s, end: e, insert: "" });
  }
  const keyed = out.map((h, i) => ({ h, i }));
  keyed.sort((x, y) => {
    if (x.h.start !== y.h.start) return x.h.start - y.h.start;
    const dx = x.h.end > x.h.start ? 1 : 0;
    const dy = y.h.end > y.h.start ? 1 : 0;
    if (dx !== dy) return dx - dy;
    return x.i - y.i;
  });
  const joined: Hunk[] = [];
  for (const { h } of keyed) {
    const l = joined[joined.length - 1];
    if (l && l.start === l.end && h.start === h.end && l.start === h.start) l.insert += h.insert;
    else joined.push({ ...h });
  }
  return joined;
}

/**
 * A change being carried (amendment E-LIVE r2.3, the kit's `Carried`): its hunks **as sent**,
 * carried through every later change without re-canonicalizing between steps; only the
 * result (`hunks()`) is canonical. The server carries an edit this way through the changes
 * since its base; the client mirrors it for its own pending change.
 */
export class Carried {
  private constructor(private readonly ps: Piece[]) {}

  /** Start carrying `hunks` (joined where they touch, as sent). */
  static of(hunks: readonly Hunk[]): Carried {
    return new Carried(pieces(coalesce(hunks)));
  }

  /** Carried through `theirs` (applied first by the server): ours after at a tie; competing
   * text refuses (rule 4 (b)), naming our hunk's index as sent and the `step`. */
  through(theirs: readonly Hunk[], step: number): { ok: Carried } | { conflict: LiveConflict } {
    const r = carry(this.ps, theirs, true, step);
    return "conflict" in r ? r : { ok: new Carried(r.ok) };
  }

  /** Carried through the client's own `pending` change (incoming first at a tie). Never
   * refuses. */
  mirrored(pending: readonly Hunk[]): Carried {
    const r = carry(this.ps, pending, false, null);
    if ("conflict" in r) throw new Error("the mirror never refuses");
    return new Carried(r.ok);
  }

  /** The canonical hunks of the change now. */
  hunks(): Hunk[] {
    return hunksOf(this.ps.map((p) => ({ ...p, dels: p.dels.map((d) => [d[0], d[1]] as [number, number]) })));
  }

  /** The gap form: an incoming change is mirrored through this, not through `hunks()`. */
  gaps(): Hunk[] {
    return gapsOf(this.ps);
  }

  isEmpty(): boolean {
    return this.ps.every((p) => p.insert.length === 0 && p.dels.length === 0);
  }
}

/**
 * **The server's rebase** (rules 2–5): carry `ours` (hunks on the base version's text) through
 * `since` (each change applied after the base, in server order, each in its own pre-change
 * coordinates), as sent (r2.3). At a tie ours goes after; competing text refuses. The result
 * is canonical, on the text after the last change.
 */
export function rebaseOurs(ours: readonly Hunk[], since: readonly (readonly Hunk[])[]): RebaseResult {
  let c = Carried.of(ours);
  for (let step = 0; step < since.length; step++) {
    const r = c.through(since[step], step);
    if ("conflict" in r) return r;
    c = r.ok;
  }
  return { ok: c.hunks() };
}

/**
 * **The client mirror** (rule 3): carry an `incoming` change (on the text before the
 * client's own `pending` change) through `pending`: at a tie the incoming one goes before.
 * Never refuses. The result is canonical, on the client's text with `pending` applied. When
 * `pending` is itself being carried, pass its gap form (`Carried.gaps()`).
 */
export function rebaseIncoming(incoming: readonly Hunk[], pending: readonly Hunk[]): Hunk[] {
  return Carried.of(incoming).mirrored(pending).hunks();
}
