// E-LIVE r2.3: a change is carried as sent through every step. Along random histories the
// client's view (incoming changes mirrored through the carried change's gap form) and the
// server's (the change carried through each, then applied) agree at every step, and refusals
// match the all-at-once rebase (as the kit's carried_client_and_server_agree_along_histories).

import { describe, expect, it } from "vitest";
import { apply, coalesce, utf8Len, type Hunk } from "../src/hunk";
import { Carried, rebaseOurs } from "../src/rebase";
import { rng } from "./fake-server";
import { runrec } from "./runrec";

function randomHunks(r: ReturnType<typeof rng>, t: string, max: number): Hunk[] {
  const bits = ["a", "b", "\n", "é", "日", "XY", ""];
  const b = [0];
  let n = 0;
  for (const ch of t) b.push((n += utf8Len(ch)));
  const k = 1 + r.int(max);
  const cuts = Array.from({ length: 2 * k }, () => r.pick(b)).sort((x, y) => x - y);
  const out: Hunk[] = [];
  for (let i = 0; i < cuts.length; i += 2) {
    const s = cuts[i];
    const e = r.next() < 0.4 ? s : cuts[i + 1];
    const last = out[out.length - 1];
    if (last && s < last.end) continue;
    let ins = "";
    for (let j = r.int(3); j > 0; j--) ins += r.pick(bits);
    if (s === e && !ins) continue;
    out.push({ start: s, end: e, insert: ins });
  }
  return coalesce(out);
}

describe("carried as sent (r2.3)", () => {
  it("client and server agree along random histories", () => {
    const r = rng(11);
    let steps = 0;
    let refused = 0;
    for (let i = 0; i < 20000; i++) {
      let base = "";
      for (let n = r.int(8); n > 0; n--) base += r.pick(["a", "b", "c", "\n", "é", "日"]);
      const ours = randomHunks(r, base, 3);
      let c = Carried.of(ours);
      let server = base;
      let client = apply(base, ours);
      const since: Hunk[][] = [];
      for (let step = 0, k = 1 + r.int(3); step < k; step++) {
        const theirs = randomHunks(r, server, 3);
        const shown = Carried.of(theirs).mirrored(c.gaps()).hunks();
        const next = c.through(theirs, step);
        server = apply(server, theirs);
        since.push(theirs);
        if ("ok" in next) {
          client = apply(client, shown);
          c = next.ok;
          steps++;
          const ctx = JSON.stringify({ base, ours, since });
          expect(apply(server, c.hunks()), ctx).toBe(client);
          expect(apply(server, c.gaps()), ctx).toBe(client);
          expect(rebaseOurs(ours, since), ctx).toEqual({ ok: c.hunks() });
        } else {
          refused++;
          expect(rebaseOurs(ours, since)).toEqual(next);
          break;
        }
      }
    }
    expect(steps).toBeGreaterThan(20000);
    expect(refused).toBeGreaterThan(100);
    runrec("client_carried_histories", { histories: 20000, steps, refused });
  });

  it("the model's counterexamples: P2 and P2c", () => {
    const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
    // P2: an insertion touching our own deletion stays an insertion (no false refusal)
    const since = [[h(1, 2, "A")], [h(2, 3, "B")]];
    const r = rebaseOurs([h(1, 1, "C"), h(2, 3)], since);
    expect("ok" in r).toBe(true);
    if ("ok" in r) expect(apply(apply(apply("ab ", since[0]), since[1]), r.ok)).toBe("aABC");
    // P2c: a split replacement still competes
    const e = rebaseOurs([h(0, 2, "B")], [[h(1, 1, "C")], [h(2, 3, "D")]]);
    expect("conflict" in e && [e.conflict.ours, e.conflict.step, e.conflict.why]).toEqual([0, 1, "replacement"]);
  });

  it("S3: an insertion is strictly inside a deleted range only under both placements", () => {
    const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
    // ours `A` at 0; `B` at 0 first (ours after it); then `Ba` deleted: ours' left placement
    // is at the deleted range's edge
    expect(rebaseOurs([h(0, 0, "A")], [[h(0, 0, "B")], [h(0, 2)]])).toEqual({ ok: [h(0, 0, "A")] });
    // both sides of both placements deleted: refused at step 1
    const e = rebaseOurs([h(1, 1, "X")], [[h(1, 1, "Y")], [h(0, 3)]]);
    expect("conflict" in e && [e.conflict.why, e.conflict.step]).toEqual(["insert_inside_deleted", 1]);
    // one side kept under either placement: applied
    expect("ok" in rebaseOurs([h(1, 1, "X")], [[h(1, 1, "Y")], [h(0, 2)]])).toBe(true);
    expect("ok" in rebaseOurs([h(1, 1, "X")], [[h(1, 1, "Y")], [h(1, 3)]])).toBe(true);
  });
});

