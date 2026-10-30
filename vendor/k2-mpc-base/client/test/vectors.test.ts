// The shared vectors, written by the Rust kit: the client must reproduce every one exactly.
// - transform.json: `position::transform` (crates/base/tests/client_vectors.rs);
// - live_rebase.json: `session::rebase` (`rebase_ours` as the server, `rebase_incoming` as the
//   client mirror; crates/base/tests/live_edit.rs). Where the kit has the file (once the
//   hunk channel is merged), that copy is checked too.

import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { Bias, transform } from "../src/position";
import { rebaseIncoming, rebaseOurs } from "../src/rebase";
import { runrec } from "./runrec";

type W = [number, number, string];
const hunks = (ws: W[]): Hunk[] => ws.map(([start, end, insert]) => ({ start, end, insert }));
const wire = (hs: Hunk[]): W[] => hs.map((h) => [h.start, h.end, h.insert]);
const load = (p: URL) => JSON.parse(readFileSync(p, "utf8"));

describe("position::transform vectors", () => {
  const v = load(new URL("./vectors/transform.json", import.meta.url));
  it(`reproduces all ${v.cases.length} cases, both biases`, () => {
    expect(v.cases.length).toBeGreaterThanOrEqual(600);
    let n = 0;
    for (const c of v.cases) {
      const hs = hunks(c.hunks);
      c.positions.forEach((p: number, i: number) => {
        expect(transform(p, hs, Bias.Before), `${JSON.stringify(c)} before @${p}`).toBe(c.before[i]);
        expect(transform(p, hs, Bias.After), `${JSON.stringify(c)} after @${p}`).toBe(c.after[i]);
        n += 2;
      });
    }
    expect(n).toBeGreaterThan(5000);
    runrec("client_transform_vectors", { cases: v.cases.length, positions: n });
  });
});

const rebaseFiles = [
  new URL("./vectors/live_rebase.json", import.meta.url),
  new URL("../../crates/base/tests/vectors/live_rebase.json", import.meta.url),
].filter((p) => existsSync(p));

describe.each(rebaseFiles.map((p) => [p.pathname.replace(/.*\/(client|crates)\//, "$1/"), p]))(
  "session::rebase vectors (%s)",
  (_name, path) => {
    const v = load(path as URL);
    it(`server and client mirror reproduce all ${v.cases.length} cases`, () => {
      let conflicts = 0;
      for (const c of v.cases) {
        const since: Hunk[][] = c.since.map(hunks);
        const r = rebaseOurs(hunks(c.ours), since);
        if ("ok" in c.server) {
          expect("ok" in r, `server refused ${JSON.stringify(c)}`).toBe(true);
          if ("ok" in r) expect(wire(r.ok), JSON.stringify(c)).toEqual(c.server.ok);
        } else {
          conflicts++;
          expect("conflict" in r, `server accepted ${JSON.stringify(c)}`).toBe(true);
          if ("conflict" in r) {
            const { ours, step, why } = c.server.conflict;
            expect([r.conflict.ours, r.conflict.step, r.conflict.why], JSON.stringify(c)).toEqual([ours, step, why]);
          }
        }
        const mirror = rebaseIncoming(since[0] ?? [], hunks(c.ours));
        expect(wire(mirror), `mirror ${JSON.stringify(c)}`).toEqual(c.mirror);
      }
      expect(conflicts).toBeGreaterThan(0);
      // named after the file: the client's copy, and the kit's when it is there
      const which = (_name as string).startsWith("crates/") ? "kit" : "client";
      runrec(`client_rebase_vectors_${which}_copy`, { cases: v.cases.length, conflicts });
    });
  },
);
