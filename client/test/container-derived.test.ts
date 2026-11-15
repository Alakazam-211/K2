// A container's derived values (the view schema's `container_derived`, WIRE.md "Derived values
// per container"): `KeyedWatch.containerDerived` and the view's containers read what the snapshot
// carried and each change's `derived.containers` set or removed. The kit's own frames
// (`test/golden/keyed_frames_container_derived.json`, written by
// `crates/base/tests/view_containers.rs`) replay to the kit's values after the snapshot and after
// every change; names the schema doesn't declare read `null`.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import type { ClientFrame, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
type Golden = { features: string[]; frames: ServerFrame[]; final_json: string; final_rev: number; derived: Record<string, Record<string, string>>[] };
const GOLDEN = JSON.parse(readFileSync(join(HERE, "golden", "keyed_frames_container_derived.json"), "utf8")) as Golden;

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

const welcome: Welcome = { root: "objs", version: 1, client: 7, self: "ada", rights: "read", features: [KEYED_WATCH_FEATURE] };

/** Each container's derived values as the client holds them (`{}`: none). */
const valuesOf = (k: KeyedWatch) => Object.fromEntries((k.view?.containers ?? []).map((c) => [c.key, c.derived ?? {}]));

describe("derived values per container", () => {
  it("the kit's frames replay to the kit's values after the snapshot and after every change", () => {
    const w = wire();
    const k = new KeyedWatch(w.t, welcome);
    const seen: Record<string, Record<string, string>>[] = [];
    let snapshotDone = false;
    for (const f of GOLDEN.frames) {
      if (f.type === "ops_change" && !snapshotDone) {
        seen.push(valuesOf(k));
        snapshotDone = true;
      }
      w.deliver(f);
      if (f.type === "ops_change") seen.push(valuesOf(k));
    }
    expect(k.ready).toBe(true);
    expect(k.viewCurrent).toBe(true);
    expect(k.stateJson()).toBe(GOLDEN.final_json);
    expect(k.rev).toBe(GOLDEN.final_rev);
    expect(seen).toEqual(GOLDEN.derived);
    // the reader: a declared name, one the container has none of, one never declared
    expect(k.containerDerived("label", "@coll:1")).toBe("WORLD");
    expect(k.containerDerived("kind", "@coll:1")).toBeNull();
    expect(k.containerDerived("kind", "@coll:7")).toBe("rig");
    expect(k.containerDerived("secret", "@coll:1")).toBeNull();
    expect(k.containerDerived("label", "@coll:99")).toBeNull();
    expect(w.sent.filter((f) => f.type === "watch").length).toBe(1);
  });

  it("the schema names them; a container with none has no derived on the wire", () => {
    const head = GOLDEN.frames[0] as unknown as { view: { schema: { container_derived?: string[] }; containers: Record<string, unknown>[] } };
    expect(head.view.schema.container_derived).toEqual(["label", "kind"]);
    const g = JSON.parse(readFileSync(join(HERE, "golden", "keyed_snapshot.json"), "utf8")) as { view: { frames: ServerFrame[] } };
    const old = g.view.frames[0] as unknown as { view: { schema: Record<string, unknown>; containers: Record<string, unknown>[] } };
    expect("container_derived" in old.view.schema).toBe(false);
    expect(old.view.containers.some((c) => "derived" in c)).toBe(false);
  });
});
