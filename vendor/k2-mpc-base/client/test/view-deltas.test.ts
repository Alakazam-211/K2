// The keyed watch's view deltas (WIRE.md, "View deltas"): a `KeyedWatch` keeps its view index
// (the snapshot's view and each container's units) from each change frame's `units` and
// `containers_changed`, and after any trace it equals a fresh snapshot's. The kit's own traces
// (`test/vectors/view_deltas.json`, written by `crates/base/tests/view_deltas.rs`, on a deck
// whose slides are named by an id field and on one named by key) replay here, and the view is
// compared with the kit's view index at each checkpoint.
//
// Plant caught by name: `ClientViewDrift` (`withContainerDeltaSkipped`). And a kit that sends
// no deltas (an older one) leaves the view as it was and says so (`viewCurrent`).

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import type { ClientFrame, OpsChangeFrame, ServerFrame, Transport, Welcome } from "../src/wire";
import { withContainerDeltaSkipped } from "./plants";

const HERE = dirname(fileURLToPath(import.meta.url));

type Expect = { view: unknown; units: Record<string, unknown[]> };
type Case = { seed: number; by_key: boolean; frames: ServerFrame[]; checks: { after_seq: number; expect: Expect }[] };
const VECTORS = JSON.parse(readFileSync(join(HERE, "vectors", "view_deltas.json"), "utf8")) as { cases: Case[] };

function wire() {
  const ls = new Set<(f: ServerFrame) => void>();
  const sent: ClientFrame[] = [];
  const t: Transport = {
    send: (f) => void sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  return { t, sent, deliver: (f: ServerFrame) => [...ls].forEach((l) => l(f)) };
}

const welcome: Welcome = { root: "slides", version: 1, client: 7, self: "ada", rights: "read", features: [KEYED_WATCH_FEATURE] };

/** The watch's view as the vectors write it. */
function viewOf(w: KeyedWatch): Expect {
  const units: Record<string, unknown[]> = {};
  for (const c of w.view?.containers ?? []) units[c.key] = [...w.units(c.key)];
  return { view: w.view, units };
}

/** Replay a case's frames; at each checkpoint, the first place the view differs from the kit's
 * (`null`: it never does). Throws if the watch asked for a snapshot. */
function replay(c: Case, frames = c.frames): { w: KeyedWatch; drift: string | null } {
  const { t, deliver } = wire();
  const w = new KeyedWatch(t, welcome);
  const asked: string[] = [];
  w.onResync((why, detail) => void (why !== "start" && asked.push(`${why}: ${detail}`)));
  let drift: string | null = null;
  for (const f of frames) {
    deliver(f);
    if (asked.length) throw new Error(`seed ${c.seed}: the watch asked for a snapshot: ${asked[0]}`);
    if (drift || f.type !== "ops_change") continue;
    for (const k of c.checks.filter((x) => x.after_seq === f.seq)) {
      try {
        expect(viewOf(w)).toEqual(k.expect);
      } catch (e) {
        drift = `seed ${c.seed} by_key ${c.by_key} after seq ${f.seq}: ${(e as Error).message.slice(0, 600)}`;
      }
    }
  }
  return { w, drift };
}

describe("the kit's view traces (test/vectors/view_deltas.json)", () => {
  it("are there: both namings, checkpoints, deltas of every kind", () => {
    expect(VECTORS.cases.length).toBeGreaterThanOrEqual(8);
    expect(new Set(VECTORS.cases.map((c) => c.by_key))).toEqual(new Set([true, false]));
    const changes = VECTORS.cases.flatMap((c) => c.frames.filter((f): f is OpsChangeFrame => f.type === "ops_change"));
    const unitOps = new Set(changes.flatMap((f) => (f.units ?? []).flatMap((u) => u.ops)));
    const contOps = new Set(changes.flatMap((f) => (f.containers_changed ?? []).map((d) => d.op)));
    for (const o of ["add", "delete", "restore", "reorder", "move_resize", "rotate", "flip", "text", "style", "hide_show", "props"]) expect(unitOps).toContain(o);
    for (const o of ["add", "delete", "restore", "reorder", "hide_show", "props"]) expect(contOps).toContain(o);
    for (const c of VECTORS.cases) expect(c.checks.length).toBeGreaterThanOrEqual(3);
  });

  for (const c of VECTORS.cases) {
    it(`seed ${c.seed} by_key ${c.by_key}: the client's view equals the kit's at every checkpoint`, () => {
      const { w, drift } = replay(c);
      expect(drift).toBeNull();
      expect(w.viewCurrent).toBe(true);
      // the last checkpoint is after the last change
      const last = c.frames.filter((f) => f.type === "ops_change").at(-1) as OpsChangeFrame;
      expect(c.checks.at(-1)!.after_seq).toBe(last.seq);
    });
  }

  it("plant ClientViewDrift (a skipped container delta) is caught", () => {
    const drifts = withContainerDeltaSkipped(() =>
      VECTORS.cases.map((c) => {
        try {
          return replay(c).drift;
        } catch (e) {
          // a unit delta into a container the view never got doesn't fit: the watch asks again
          return (e as Error).message;
        }
      }),
    );
    expect(drifts.filter((d) => d !== null).length).toBeGreaterThan(0);
  });

  it("frames without deltas (an older kit) leave the view as it was, and say so", () => {
    const c = VECTORS.cases[0];
    const old = c.frames.map((f) => {
      if (f.type !== "ops_change") return f;
      const g: Partial<OpsChangeFrame> = { ...f };
      delete g.units;
      delete g.containers_changed;
      delete g.containers;
      return g as ServerFrame;
    });
    const head = c.frames.findIndex((f) => f.type === "ops_change");
    const before = JSON.stringify(viewOf(replay({ ...c, checks: [] }, old.slice(0, head)).w));
    const { w } = replay({ ...c, checks: [] }, old);
    expect(w.ready).toBe(true);
    expect(w.viewCurrent).toBe(false);
    expect(JSON.stringify(viewOf(w))).toBe(before);
  });
});
