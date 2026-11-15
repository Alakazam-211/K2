// The view schema's nesting (WIRE.md, "Nesting"): which unit kinds sit inside which. The kit's
// snapshot of a deck that declares it (`test/golden/keyed_snapshot_nesting.json`, written by
// `crates/base/tests/keyed_watch.rs`) reads to the kit's state and units, with the schema's
// `nesting` as the pack declared it; the kit's older snapshot golden (a schema with none) has no
// `nesting` at all.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import type { ClientFrame, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
type Golden = { features: string[]; frames: ServerFrame[]; final_json: string; units: Record<string, { key: string }[]> };
const load = (name: string) => JSON.parse(readFileSync(join(HERE, "golden", name), "utf8")) as Golden;

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

const welcome: Welcome = { root: "items", version: 1, client: 7, self: "ada", rights: "read", features: [KEYED_WATCH_FEATURE] };

function read(g: Golden) {
  const w = wire();
  const k = new KeyedWatch(w.t, welcome);
  for (const f of g.frames) w.deliver(f);
  return k;
}

describe("the view schema's nesting", () => {
  it("the kit's snapshot of a deck that declares it reads to the kit's state and units, with the nesting in order", () => {
    const g = load("keyed_snapshot_nesting.json");
    const k = read(g);
    expect(k.ready).toBe(true);
    expect(k.stateJson()).toBe(g.final_json);
    expect(k.view?.schema.nesting).toEqual([
      { kind: "box", parent: "group" },
      { kind: "group", parent: "group" },
    ]);
    const units = Object.fromEntries((k.view?.containers ?? []).map((c) => [c.key, k.units(c.key).map((u) => u.key)]));
    expect(units).toEqual(Object.fromEntries(Object.entries(g.units).map(([c, us]) => [c, us.map((u) => u.key)])));
  });

  it("a schema with none has no nesting on the wire", () => {
    const g = JSON.parse(readFileSync(join(HERE, "golden", "keyed_snapshot.json"), "utf8")) as { view: Golden };
    const k = read(g.view);
    expect(k.ready).toBe(true);
    expect(k.view?.schema).toBeDefined();
    expect("nesting" in (k.view?.schema ?? {})).toBe(false);
  });
});
