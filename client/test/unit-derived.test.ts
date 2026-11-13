// A unit's derived values (the view schema's `unit_derived`, WIRE.md "Derived values per
// unit"): `KeyedWatch.unitDerived` and `KeyedWatch.unit` read what a snapshot's units pages
// carried and each change's `derived.units` set or removed. The kit's own frames
// (`test/golden/keyed_frames_unit_derived.json`, written by `crates/base/tests/view_slots.rs`)
// replay to the kit's state and to every unit's derived values at the end, byte for byte; a
// frame from an older kit (no `unit_derived` in the schema) leaves them `null`.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import type { ClientFrame, OpsChangeFrame, ServerFrame, Transport, ViewSchemaJson, ViewUnit, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
type Golden = { features: string[]; frames: ServerFrame[]; final_json: string; final_rev: number; final_derived: Record<string, Record<string, string>> };
const GOLDEN = JSON.parse(readFileSync(join(HERE, "golden", "keyed_frames_unit_derived.json"), "utf8")) as Golden;

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

const SCHEMA: ViewSchemaJson = {
  containers: "groups",
  units: "objs",
  unit_container_field: "group",
  container_id_field: null,
  native_unit: "scene-unit",
  page: null,
  container_fields: { title: "name", hidden: null, layout: null },
  unit_fields: { kind: { list_value: true }, name: "name", alt: null, text: null, hidden: null, editable_text: null, placeholder: null },
  geometry: null,
  field_class: { xform: "geometry", name: "text" },
  derived: { containers: [], units: [] },
  unit_derived: ["transform", "label"],
};

const unit = (key: string, derived?: Record<string, string>): ViewUnit => ({
  key,
  container: "@grp:1",
  parent: null,
  live: true,
  kind: "mesh",
  name: key,
  name_is_generic: null,
  alt: null,
  z: 0,
  bbox: null,
  bbox_inherited: false,
  rot: 0,
  flip: { h: false, v: false },
  approx_box: false,
  text: null,
  fields: [],
  hidden: false,
  placeholder: null,
  editable_text: null,
  ...(derived ? { derived } : {}),
});

/** A two-unit scene's snapshot (the schema given), then change frames. */
function snapshot(schema: ViewSchemaJson): ServerFrame[] {
  return [
    {
      type: "keyed_snapshot",
      seq: 1,
      upto: 3,
      schema: {
        name: "scene-lite",
        roots: [
          { name: "groups", kind: "list" },
          { name: "group_f", kind: "map" },
          { name: "objs", kind: "list" },
          { name: "obj_f", kind: "map" },
        ],
        composite: [
          { map: "group_f", list: "groups" },
          { map: "obj_f", list: "objs" },
        ],
      },
      view: {
        schema,
        tier: "live",
        rev: 3,
        version: null,
        page: null,
        fonts: [],
        layouts: [],
        containers: [{ key: "@grp:1", position: 1, title: "Props", hidden: false, layout: null, render_hash: null, fonts: [], has_fields: [], unit_count: 2 }],
      },
      pages: 4,
    },
    { type: "keyed_page", seq: 2, upto: 3, page: 1, pages: 4, root: "groups", live: [["@grp:1", "group"]] },
    { type: "keyed_page", seq: 3, upto: 3, page: 2, pages: 4, root: "objs", live: [["@obj:1", "mesh"], ["@obj:2", "mesh"]] },
    {
      type: "keyed_page",
      seq: 4,
      upto: 3,
      page: 3,
      pages: 4,
      root: "obj_f",
      entries: [
        ["@obj:1", "group", "g1"],
        ["@obj:1", "name", "@obj:1"],
        ["@obj:2", "group", "g1"],
        ["@obj:2", "name", "@obj:2"],
      ],
    },
    {
      type: "keyed_page",
      seq: 5,
      upto: 3,
      page: 4,
      pages: 4,
      root: "objs",
      units: { container: "@grp:1", from: 0, units: [unit("@obj:1", { transform: "xform 0,0,0", label: ">Cube" }), unit("@obj:2")], next: null, total: 2 },
    },
  ];
}

function change(seq: number, rev: number, derived: OpsChangeFrame["derived"]): OpsChangeFrame {
  return {
    type: "ops_change",
    seq,
    rev,
    at_ms: 1,
    change_kind: "edit",
    actor: { id: "ada", name: "Ada", kind: "person", colour: null },
    client: 1,
    via: "tab",
    on_behalf_of: null,
    turn: null,
    verb: null,
    claim: null,
    ops: [{ root: "obj_f", kind: "map", op: "set", key: "@obj:1", field: "xform", after: "5,5,5" }],
    units: [{ key: "@obj:1", container: "@grp:1", ops: ["move_resize"], geom_before: null, geom_after: null, fields: ["xform"], hidden_after: null, placeholder_type: null }],
    containers_changed: [],
    containers: ["@grp:1"],
    derived,
    render_hashes: null,
  };
}

describe("a unit's derived values", () => {
  it("come with the snapshot's units, and each change's derived.units sets or removes them", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of snapshot(SCHEMA)) deliver(f);
    expect(w.ready).toBe(true);
    expect(w.unitDerived("transform", "@obj:1")).toBe("xform 0,0,0");
    expect(w.unitDerived("label", "@obj:1")).toBe(">Cube");
    expect(w.unitDerived("label", "@obj:2")).toBeNull();
    expect(w.unit("@obj:2")?.key).toBe("@obj:2");
    expect(w.unit("@obj:9")).toBeNull();
    // a value set, another removed, a name the schema doesn't declare left alone, a unit the
    // view doesn't know skipped
    deliver(change(6, 4, { units: { "@obj:1": { transform: "xform 5,5,5", label: null, other: "x" }, "@obj:2": { label: ">Two" }, "@obj:9": { label: "?" } } }));
    expect(w.unitDerived("transform", "@obj:1")).toBe("xform 5,5,5");
    expect(w.unitDerived("label", "@obj:1")).toBeNull();
    expect(w.unitDerived("other", "@obj:1")).toBeNull();
    expect(w.unitDerived("label", "@obj:2")).toBe(">Two");
    expect(w.unitDerived("label", "@obj:9")).toBeNull();
    expect(w.unit("@obj:1")?.derived).toEqual({ transform: "xform 5,5,5" });
    // a change with no derived values changes none
    deliver(change(7, 5, null));
    expect(w.unitDerived("transform", "@obj:1")).toBe("xform 5,5,5");
    expect(w.ready).toBe(true);
  });

  it("from an older kit (no unit_derived in the schema): none, and the watch works as before", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    const old = { ...SCHEMA };
    delete old.unit_derived;
    for (const f of snapshot(old)) deliver(f);
    deliver(change(6, 4, { units: { "@obj:1": { label: ">New" } } }));
    expect(w.ready).toBe(true);
    expect(w.unitDerived("label", "@obj:1")).toBeNull();
    expect(w.unitDerived("transform", "@obj:1")).toBeNull();
  });
});

describe("the kit's frames (test/golden/keyed_frames_unit_derived.json)", () => {
  it("replay to the kit's state and to every unit's derived values", () => {
    expect(GOLDEN.features).toEqual([KEYED_WATCH_FEATURE]);
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of GOLDEN.frames) deliver(f);
    expect(w.ready).toBe(true);
    expect(w.stateJson()).toBe(GOLDEN.final_json);
    expect(w.rev).toBe(GOLDEN.final_rev);
    expect(sent.filter((f) => f.type === "watch").length).toBe(1);
    const head = GOLDEN.frames[0] as { view?: { schema: ViewSchemaJson } };
    const names = head.view?.schema.unit_derived ?? [];
    expect(names).toEqual(["transform", "label"]);
    // every unit the view shows, with exactly the kit's values at the end
    const containers = w.view!.containers.map((c) => c.key);
    const got: Record<string, Record<string, string>> = {};
    for (const c of containers) {
      for (const u of w.units(c)) {
        const d: Record<string, string> = {};
        for (const n of names) {
          const v = w.unitDerived(n, u.key);
          if (v !== null) d[n] = v;
        }
        if (Object.keys(d).length > 0) got[u.key] = d;
      }
    }
    expect(got).toEqual(GOLDEN.final_derived);
    // the frames carried some: a value set, one removed, many at once
    const changes = GOLDEN.frames.filter((f): f is OpsChangeFrame => f.type === "ops_change");
    const values = changes.flatMap((c) => Object.values(c.derived?.units ?? {}).flatMap((d) => Object.values(d as Record<string, unknown>)));
    expect(values.some((v) => v === null)).toBe(true);
    expect(values.filter((v) => typeof v === "string").length).toBeGreaterThan(4);
  });
});
