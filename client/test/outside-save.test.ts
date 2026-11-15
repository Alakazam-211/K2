// The keyed watch's `outside_save` frame (WIRE.md, "Outside saves"): unnumbered, after the save's
// own change. `KeyedWatch` hands each one on (`onOutsideSave`) and changes nothing for it: the
// kit's scripted session (`test/golden/keyed_frames.json`, which ends with an outside save) reads
// to the kit's final state with the frame handed on, and with no gap asked for.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import type { ClientFrame, OutsideSaveFrame, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
type Golden = { frames: ServerFrame[]; final_json: string; final_rev: number };

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

const welcome: Welcome = { root: "shapes", version: 1, client: 7, self: "ada", rights: "read", features: [KEYED_WATCH_FEATURE] };

describe("the outside_save frame", () => {
  it("the kit's scripted session hands on its outside save's outcome and reads to the kit's state", () => {
    const g = JSON.parse(readFileSync(join(HERE, "golden", "keyed_frames.json"), "utf8")) as Golden;
    const want = g.frames.filter((f): f is OutsideSaveFrame => f.type === "outside_save");
    expect(want.length).toBe(1);
    expect(want[0].outcome).toBe("kept_both");
    const w = wire();
    const k = new KeyedWatch(w.t, welcome);
    const got: OutsideSaveFrame[] = [];
    k.onOutsideSave((o) => got.push(o));
    for (const f of g.frames) w.deliver(f);
    expect(got).toEqual(want);
    expect(k.ready).toBe(true);
    expect(k.stateJson()).toBe(g.final_json);
    expect(k.rev).toBe(g.final_rev);
    // the first `watch` only: the unnumbered frame is no gap
    expect(w.sent.filter((f) => f.type === "watch").length).toBe(1);
  });
});
