// Placements in 3D (WIRE.md, "Placements in 3D"): the client's helper on the shared vectors
// (`test/vectors/placement.json`, written by the kit's Rust helper in
// `crates/base/tests/view_place.rs`): every form, order and degenerate case, unknown kinds, and
// chains, broken ones giving `null`; the kit's frames (`test/golden/keyed_frames_placement.json`)
// replay to the kit's state and to every element's world matrix; hand-built frames keep
// placements through `place`, `add` and a removal; a kit with no slot gives none.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch } from "../src/keyed";
import { placementMatrix, worldMatrix, type Mat4, type Precision } from "../src/placement";
import type { ClientFrame, PlacementJson, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
const load = <T>(...p: string[]): T => JSON.parse(readFileSync(join(HERE, ...p), "utf8")) as T;

type Case = { name: string; precision: Precision; placement: PlacementJson; matrix: number[] | null };
type Chain = { name: string; precision: Precision; placements: Record<string, PlacementJson | null>; address: string; world: number[] | null };
const VECTORS = load<{ cases: Case[]; chains: Chain[] }>("vectors", "placement.json");
type Golden = { frames: ServerFrame[]; final_json: string; final_rev: number; final_world: Record<string, number[] | null> };
const GOLDEN = load<Golden>("golden", "keyed_frames_placement.json");

/** Within the stated tolerance: 1e-12 relative to the largest entry, plus 1e-12. */
function near(got: Mat4 | null, want: number[] | null): boolean {
  if (got === null || want === null) return got === want;
  const big = Math.max(...want.map(Math.abs));
  return want.every((w, i) => Math.abs(got[i] - w) <= 1e-12 * big + 1e-12);
}

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

describe("the shared vectors (test/vectors/placement.json)", () => {
  it("are there, with every kind of case", () => {
    expect(VECTORS.cases.length).toBeGreaterThan(20);
    expect(VECTORS.cases.some((c) => c.matrix === null)).toBe(true);
    expect(VECTORS.chains.some((c) => c.world === null)).toBe(true);
    expect(VECTORS.chains.some((c) => c.world !== null)).toBe(true);
  });

  for (const c of VECTORS.cases) {
    it(`placement: ${c.name}`, () => {
      expect(near(placementMatrix(c.placement, c.precision), c.matrix), JSON.stringify(placementMatrix(c.placement, c.precision))).toBe(true);
    });
  }

  for (const c of VECTORS.chains) {
    it(`chain: ${c.name}`, () => {
      const lookup = (a: string) => (a in c.placements ? c.placements[a] : undefined);
      expect(near(worldMatrix(lookup, c.address, c.precision), c.world)).toBe(true);
    });
  }
});

describe("the kit's frames (test/golden/keyed_frames_placement.json)", () => {
  it("replay to the kit's state and to every element's world matrix", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of GOLDEN.frames) deliver(f);
    expect(w.ready).toBe(true);
    expect(w.stateJson()).toBe(GOLDEN.final_json);
    expect(w.rev).toBe(GOLDEN.final_rev);
    expect(w.viewCurrent).toBe(true);
    const entries = Object.entries(GOLDEN.final_world);
    expect(entries.filter(([, m]) => m === null).length).toBeGreaterThan(0);
    expect(entries.filter(([, m]) => m !== null).length).toBeGreaterThan(5);
    for (const [a, m] of entries) expect(near(w.worldMatrix(a), m), a).toBe(true);
    expect(w.space?.length_unit).toBe("mm");
  });
});

const PLACED = (x: number, rel: string | null = null): PlacementJson => ({
  relative_to: rel,
  steps: [{ trs: { loc: [x, 0, 0], rot: { euler: "XYZ", angles: [0, 0, 0] }, scale: [1, 1, 1] } }],
});

/** A two-unit scene's snapshot: `@o:1` placed at x 1, `@o:2` at x 2 relative to it. */
function snapshot(slot: boolean): ServerFrame[] {
  const unit = (key: string, placement?: PlacementJson) => ({
    key,
    container: "@g:1",
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
    ...(placement ? { placement } : {}),
  });
  return [
    {
      type: "keyed_snapshot",
      seq: 1,
      upto: 3,
      schema: {
        name: "scene3d",
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
        schema: {
          containers: "groups",
          units: "objs",
          unit_container_field: "group",
          container_id_field: null,
          native_unit: "scene-unit",
          page: null,
          container_fields: { title: null, hidden: null, layout: null },
          unit_fields: { kind: { list_value: true }, name: null, alt: null, text: null, hidden: null, editable_text: null, placeholder: null },
          geometry: null,
          field_class: {},
          derived: { containers: [], units: [] },
          ...(slot ? { space: { unit_fields: ["xform"], container_fields: [], precision: "f64" as const } } : {}),
        },
        tier: "live",
        rev: 3,
        version: null,
        page: null,
        fonts: [],
        layouts: [],
        containers: [{ key: "@g:1", position: 1, title: null, hidden: false, layout: null, render_hash: null, fonts: [], has_fields: [], unit_count: 2 }],
        ...(slot ? { space: { length_unit: "m", metres_per_unit: 1, up: "z", handed: "right" } } : {}),
      },
      pages: 4,
    },
    { type: "keyed_page", seq: 2, upto: 3, page: 1, pages: 4, root: "groups", live: [["@g:1", "group"]] },
    { type: "keyed_page", seq: 3, upto: 3, page: 2, pages: 4, root: "objs", live: [["@o:1", "mesh"], ["@o:2", "mesh"]] },
    { type: "keyed_page", seq: 4, upto: 3, page: 3, pages: 4, root: "obj_f", entries: [["@o:1", "group", "g1"], ["@o:2", "group", "g1"]] },
    {
      type: "keyed_page",
      seq: 5,
      upto: 3,
      page: 4,
      pages: 4,
      root: "objs",
      units: { container: "@g:1", from: 0, units: [unit("@o:1", slot ? PLACED(1) : undefined), unit("@o:2", slot ? PLACED(2, "@o:1") : undefined)], next: null, total: 2 },
    },
  ];
}

function change(seq: number, rev: number, units: Record<string, unknown>[], ops: unknown[] = []): ServerFrame {
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
    ops: ops as never,
    units: units as never,
    containers_changed: [],
    containers: ["@g:1"],
    derived: null,
    render_hashes: null,
  };
}

const delta = (key: string, ops: string[], placement_after?: PlacementJson | null) => ({
  key,
  container: "@g:1",
  ops,
  geom_before: null,
  geom_after: null,
  fields: [],
  hidden_after: null,
  placeholder_type: null,
  ...(placement_after !== undefined ? { placement_after } : {}),
});

describe("placements kept from the frames", () => {
  it("a place delta moves a unit and what is placed through it; null removes one", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of snapshot(true)) deliver(f);
    expect(w.worldMatrix("@o:2")?.[12]).toBe(3);
    deliver(change(6, 4, [delta("@o:1", ["place"], PLACED(10))]));
    expect(w.worldMatrix("@o:2")?.[12]).toBe(12);
    deliver(change(7, 5, [delta("@o:1", ["place"], null)]));
    expect(w.placement("@o:1")).toBeNull();
    expect(w.worldMatrix("@o:2")).toBeNull();
    expect(w.ready).toBe(true);
  });

  it("an added unit arrives with its placement", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of snapshot(true)) deliver(f);
    const ops = [
      { root: "objs", kind: "list", op: "insert", key: "@o:3", index_before: null, index_after: 2, after: "mesh", effect: "visible" },
      { root: "obj_f", kind: "map", op: "set", key: "@o:3", field: "group", after: "g1" },
    ];
    deliver(change(6, 4, [delta("@o:3", ["add"], PLACED(5, "@o:2"))], ops));
    expect(w.unit("@o:3")).not.toBeNull();
    expect(w.worldMatrix("@o:3")?.[12]).toBe(8);
  });

  it("an unknown step kind is an unknown placement, and a chain through it has none", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of snapshot(true)) deliver(f);
    deliver(change(6, 4, [delta("@o:1", ["place"], { relative_to: null, steps: [{ spline: {} } as never] })]));
    expect(w.worldMatrix("@o:1")).toBeNull();
    expect(w.worldMatrix("@o:2")).toBeNull();
  });

  it("from a kit with no slot: no space, no placement, no world matrix", () => {
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome);
    for (const f of snapshot(false)) deliver(f);
    expect(w.ready).toBe(true);
    expect(w.space).toBeNull();
    expect(w.placement("@o:1")).toBeNull();
    expect(w.worldMatrix("@o:1")).toBeNull();
  });
});
