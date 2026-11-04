// References to list elements and their state, the kit's `crate::refs` (Rust) for clients: a
// reference is never moved or deleted silently. Its state is computed from the doc, never
// stored, and the same on every replica. Checked against the kit's vectors
// (test/vectors/refs.json).

/** A reference to element `key` of list `root`; `hint` is a pack value to draw an orphan
 * with (the kit never reads it). */
export interface Ref {
  root: string;
  key: string;
  hint?: string | null;
}

/** A reference's state: attached; suspect (its element changed kind, its container is gone,
 * or it closes a cycle of parents); orphaned (its element is deleted or unknown). */
export type RefState = "attached" | "kind_changed" | "container_gone" | "cycle" | "orphaned";

/** A doc's lists as the helper reads them: each list root's live elements, in order. A root
 * that isn't here isn't a list. */
export type Lists = Readonly<Record<string, ReadonlyArray<readonly [string, string | null]>>>;

type KindOk = (value: string | null) => boolean;

const stateOf = (v: string | null, kindOk: KindOk): RefState => (kindOk(v) ? "attached" : "kind_changed");

/** The state of `r`. */
export function resolveRef(lists: Lists, r: Ref, kindOk: KindOk = () => true): RefState {
  const live = Object.prototype.hasOwnProperty.call(lists, r.root) ? lists[r.root] : undefined;
  if (!live) return "container_gone";
  const e = live.find(([k]) => k === r.key);
  return e ? stateOf(e[1], kindOk) : "orphaned";
}

/** Every reference with its state, in order: one entry per reference, orphans included. */
export function resolveRefs(lists: Lists, refs: readonly Ref[], kindOk: KindOk = () => true): [Ref, RefState][] {
  const index = new Map<string, Map<string, string | null> | null>();
  return refs.map((r) => {
    let l = index.get(r.root);
    if (l === undefined) {
      const live = Object.prototype.hasOwnProperty.call(lists, r.root) ? lists[r.root] : undefined;
      l = live ? new Map(live.map(([k, v]) => [k, v] as const)) : null;
      index.set(r.root, l);
    }
    if (l === null) return [r, "container_gone"];
    return [r, l.has(r.key) ? stateOf(l.get(r.key) ?? null, kindOk) : "orphaned"];
  });
}

/** The parent references of `root`'s live elements, for each that `parentOf` gives a parent:
 * attached (the parent is live), container_gone (it isn't), or cycle (the element closes a
 * cycle of live parents; each cycle is cut at its smallest key, in any list order). */
export function resolveParents(lists: Lists, root: string, parentOf: (key: string) => string | null): Map<string, RefState> {
  const out = new Map<string, RefState>();
  const live = Object.prototype.hasOwnProperty.call(lists, root) ? lists[root] : undefined;
  if (!live) return out;
  const pos = new Map(live.map(([k], i) => [k, i] as const));
  const next: (number | null)[] = live.map(() => null);
  live.forEach(([k], i) => {
    const p = parentOf(k);
    if (p === null) return;
    const j = pos.get(p);
    if (j === undefined) out.set(k, "container_gone");
    else {
      next[i] = j;
      out.set(k, "attached");
    }
  });
  const colour = live.map(() => 0);
  for (let start = 0; start < live.length; start++) {
    if (colour[start] !== 0) continue;
    const walk = start + 1;
    let at = start;
    for (;;) {
      colour[at] = walk;
      const j = next[at];
      if (j === null) break;
      if (colour[j] === 0) {
        at = j;
        continue;
      }
      if (colour[j] === walk) {
        let cut = j;
        for (let c = next[j]!; c !== j; c = next[c]!) if (live[c][0] < live[cut][0]) cut = c;
        out.set(live[cut][0], "cycle");
      }
      break;
    }
  }
  return out;
}
