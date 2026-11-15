// `KeyedView`: a watcher's view index of a live keyed doc (a deck's slides and shapes), kept
// from the snapshot's view and each change frame's view deltas (WIRE.md, "View deltas"). Data,
// no DOM. A change frame says which containers and units changed and how; the values come from
// the frame's own ops, which the watch has already applied to its keyed state. The view never
// runs pack code: a field the pack derives (the view schema's `derived`) keeps what the
// snapshot gave it.

import type {
  ContainerDelta,
  GridSlot,
  KeyedSchema,
  OpsChangeFrame,
  PlacementJson,
  UnitDelta,
  ViewContainer,
  ViewSchemaJson,
  ViewSnapshotJson,
  ViewSrc,
  ViewUnit,
} from "./wire";
import type { KeyedState } from "./keyed";

/** The longest name, alt text, title or text summary a view carries, in UTF-8 bytes (the kit's
 * `VIEW_TEXT_MAX`). */
export const VIEW_TEXT_MAX = 1024;

/** The deepest a container sits in the containers' tree, in levels (the top level is 1): the
 * kit's `CONTAINER_DEPTH_MAX`. A container's `parent` never puts it deeper. */
export const CONTAINER_DEPTH_MAX = 32;

const SEP = "\u001f";

/** A unit's or container's placement as a delta's `placement_after` gives it (`null`: none). */
function setPlacement(x: { placement?: PlacementJson | null }, p: PlacementJson | null | undefined): void {
  if (p) x.placement = p;
  else delete x.placement;
}

/** A container's parent as a delta's `parent_after` gives it (`null`: at the top level). */
function setParent(c: ViewContainer, p: string | null | undefined): void {
  if (p) c.parent = p;
  else delete c.parent;
}

/** One element's part of a change's `derived` (`d`, `{name: text or null}`) onto its `derived`,
 * for the schema's `names` only: a text sets one, `null` removes it, anything else is left. */
function setDerived(x: { derived?: Record<string, string> } | undefined, d: unknown, names: string[] | undefined): void {
  if (!x || !names || names.length === 0 || !d || typeof d !== "object") return;
  const vals = d as Record<string, unknown>;
  for (const name of names) {
    if (!(name in vals)) continue;
    const v = vals[name];
    if (typeof v === "string") x.derived = { ...(x.derived ?? {}), [name]: v };
    else if (v === null && x.derived && name in x.derived) {
      const rest = { ...x.derived };
      delete rest[name];
      if (Object.keys(rest).length === 0) delete x.derived;
      else x.derived = rest;
    }
  }
}

/** A view delta that doesn't fit the view: the watch asks for a snapshot. */
export class ViewMisfit extends Error {}

function misfit(why: string): never {
  throw new ViewMisfit(why);
}

/** `s` cut to at most `max` UTF-8 bytes at a character boundary, and whether it was cut. */
export function cutText(s: string, max = VIEW_TEXT_MAX): [string, boolean] {
  let n = 0;
  let out = "";
  for (const ch of s) {
    const c = ch.codePointAt(0)!;
    const len = c < 0x80 ? 1 : c < 0x800 ? 2 : c < 0x10000 ? 3 : 4;
    if (n + len > max) return [out, true];
    n += len;
    out += ch;
  }
  return [out, false];
}

/** A whole number as the kit reads one (`str::trim`, then `i64`): `null` when it isn't. */
function int(s: string): number | null {
  const t = s.trim();
  if (!/^[+-]?\d+$/.test(t)) return null;
  const b = BigInt(t);
  if (b > 9223372036854775807n || b < -9223372036854775808n) return null;
  return Number(b);
}

const flag = (v: string | null) => v === "1" || v === "true";

type Geom = { bbox: ViewUnit["bbox"]; rot: number; flip: ViewUnit["flip"] };

/** A geometry value by the schema's parts, as the kit parses it: whole or not at all. */
export function parseGeom(v: string | null, parts: string[]): Geom | null {
  if (!v) return null;
  const vals = v.split(",");
  if (vals.length !== parts.length) return null;
  const g = { bbox: { x: 0, y: 0, w: 0, h: 0 }, rot: 0, flip: { h: false, v: false } };
  for (let i = 0; i < parts.length; i++) {
    const s = vals[i].trim();
    const p = parts[i];
    if (p === "fh" || p === "fv") {
      const f = s === "0" || s === "false" ? false : s === "1" || s === "true" ? true : null;
      if (f === null) return null;
      if (p === "fh") g.flip.h = f;
      else g.flip.v = f;
      continue;
    }
    const n = int(s);
    if (n === null) return null;
    if (p === "x" || p === "y" || p === "w" || p === "h") g.bbox[p] = n;
    else if (p === "rot") g.rot = n;
    else return null;
  }
  return g;
}

const EDITABLE = new Set(["plain", "formatted", "never"]);

/** The field a source reads (`null` for none, the list value, or a derived value). */
function srcField(src: ViewSrc): string | null {
  if (typeof src === "string") return src;
  if (src && "field" in src) return src.field;
  return null;
}

const isListValue = (src: ViewSrc) => !!src && typeof src === "object" && "list_value" in src;

/**
 * A view index kept from a snapshot and the change frames after it. Built by `KeyedWatch` when
 * the snapshot carries a view; `apply` runs after each change's ops are in the state.
 */
export class KeyedView {
  /** The snapshot's view, kept current: its containers in order, and its `rev`. */
  readonly snapshot: ViewSnapshotJson;
  /** Each live container's units, back to front. */
  private readonly lists = new Map<string, ViewUnit[]>();
  /** Every shown unit by address. */
  private readonly shown = new Map<string, ViewUnit>();
  /** Units that left the view, kept for what the pack derived (a restore brings them back). */
  private readonly gone = new Map<string, ViewUnit>();
  /** Containers that left, likewise. */
  private readonly goneContainers = new Map<string, ViewContainer>();
  private readonly vs: ViewSchemaJson;
  private readonly cmap: string | null;
  private readonly umap: string | null;
  /** Whether every change since the snapshot carried its deltas (an older kit's don't). */
  private _current: boolean;

  constructor(snapshot: ViewSnapshotJson, units: Map<string, ViewUnit[]>, schema: KeyedSchema) {
    this.snapshot = snapshot;
    this.vs = snapshot.schema;
    const mapOf = (list: string) => schema.composite?.find((c) => c.list === list)?.map ?? null;
    this.cmap = mapOf(this.vs.containers);
    this.umap = mapOf(this.vs.units);
    // an older kit's snapshot names no composite maps: the view can't read fields
    this._current = schema.composite !== undefined;
    for (const c of snapshot.containers) {
      const l = units.get(c.key) ?? [];
      this.lists.set(c.key, l);
      for (const u of l) this.shown.set(u.key, u);
    }
  }

  /** The view equals the doc's: every change since the snapshot carried its deltas. */
  get current(): boolean {
    return this._current;
  }

  /** The units of the container with address `container`, back to front. */
  units(container: string): readonly ViewUnit[] {
    return this.lists.get(container) ?? [];
  }

  /** The unit with address `address` the view shows (a nested one too), `null` when it shows
   * none. */
  unit(address: string): ViewUnit | null {
    const u = this.shown.get(address);
    return u && this.lists.get(u.container)?.includes(u) ? u : null;
  }

  /** For a chain's walk: the placement of the unit or container with address `address` the view
   * shows (`null`: shown, not placed), `undefined` when it shows none. */
  placementOf(address: string): PlacementJson | null | undefined {
    const u = this.unit(address);
    if (u) return u.placement ?? null;
    const c = this.snapshot.containers.find((x) => x.key === address);
    return c ? (c.placement ?? null) : undefined;
  }

  /** A change's `derived.units` values of the schema's `unit_derived` names onto the units, and
   * its `derived.containers` values of the `container_derived` names onto the containers, as the
   * kit sends them (the values the change set or removed): a text sets one, `null` removes it.
   * Names the schema doesn't declare, and elements the view doesn't know, are left alone. */
  applyDerived(f: OpsChangeFrame): void {
    const units = f.derived?.units;
    if (units) {
      for (const [addr, d] of Object.entries(units)) setDerived(this.shown.get(addr) ?? this.gone.get(addr), d, this.vs.unit_derived);
    }
    // the containers' (`container_derived`), the same way
    const containers = f.derived?.containers;
    if (containers) {
      for (const [addr, d] of Object.entries(containers)) {
        const c = this.snapshot.containers.find((x) => x.key === addr) ?? this.goneContainers.get(addr);
        setDerived(c, d, this.vs.container_derived);
      }
    }
  }

  /** The view schema's live grid (`schema.grid`, kept as the snapshot gave it, with its
   * `field_class_prefix`), `null` when the pack has none. */
  get grid(): GridSlot | null {
    return this.vs.grid ?? null;
  }

  /** One change, after its ops were applied to `st`. A frame with no deltas (an older kit)
   * leaves the view as it was and no longer current. */
  apply(f: OpsChangeFrame, st: KeyedState): void {
    if (!this._current) return;
    if (f.units === undefined || f.containers_changed === undefined) {
      this._current = false;
      return;
    }
    const dirty = new Set<string>();
    for (const d of f.containers_changed) this.containerDelta(d, st, dirty);
    this.snapshot.containers.forEach((c, i) => (c.position = i + 1));
    for (const d of f.units) this.unitDelta(d, st, dirty);
    for (const c of dirty) this.relist(c, st);
    for (const c of this.snapshot.containers) c.unit_count = this.lists.get(c.key)?.length ?? 0;
    const page = this.vs.page;
    if (page && f.ops.some((o) => o.root === page.root)) this.repage(st);
    if (f.view_space) this.snapshot.space = f.view_space;
    this.snapshot.rev = f.rev;
  }

  // ---------- reading the state by address ----------

  private mapGet(st: KeyedState, map: string | null, key: string): string | null {
    if (map === null) return null;
    const r = st.roots.get(map);
    if (!r || r.kind !== "map") return null;
    return r.entries.get(key) ?? null;
  }

  private listValue(st: KeyedState, list: string, addr: string): string | null {
    const r = st.roots.get(list);
    if (!r || r.kind !== "list") return null;
    const e = r.live.find(([k]) => k === addr);
    return e ? e[1] : (r.hidden.get(addr) ?? null);
  }

  private read(st: KeyedState, units: boolean, addr: string, src: ViewSrc): string | null {
    const map = units ? this.umap : this.cmap;
    const list = units ? this.vs.units : this.vs.containers;
    if (src === null) return null;
    if (typeof src === "string") return this.mapGet(st, map, addr + SEP + src);
    if ("list_value" in src) return this.listValue(st, list, addr);
    if ("derived" in src) return null;
    const raw = this.mapGet(st, map, addr + SEP + src.field);
    return raw === null ? null : (src.values[raw] ?? null);
  }

  // ---------- containers ----------

  private containerField(st: KeyedState, c: ViewContainer, name: "title" | "hidden" | "layout"): void {
    if (this.vs.derived.containers.includes(name)) return;
    const v = this.read(st, false, c.key, this.vs.container_fields[name]);
    if (name === "title") c.title = v === null ? null : cutText(v)[0];
    else if (name === "hidden") c.hidden = flag(v);
    else c.layout = v ? v : null;
  }

  private buildContainer(st: KeyedState, key: string): ViewContainer {
    const prev = this.goneContainers.get(key);
    this.goneContainers.delete(key);
    const c: ViewContainer = prev ?? { key, position: 0, title: null, hidden: false, layout: null, render_hash: null, fonts: [], has_fields: [], unit_count: 0 };
    for (const n of ["title", "hidden", "layout"] as const) this.containerField(st, c, n);
    return c;
  }

  /** One container delta. */
  containerDelta(d: ContainerDelta, st: KeyedState, dirty: Set<string>): void {
    const cs = this.snapshot.containers;
    const at = (p: number | null) => (p === null ? misfit(`${d.op} ${d.key} with no position`) : p - 1);
    const here = (i: number) => (cs[i]?.key === d.key ? i : misfit(`${d.op} ${d.key}: not at ${i + 1}`));
    switch (d.op) {
      case "add":
      case "restore": {
        const i = at(d.position_after);
        if (i > cs.length || cs.some((c) => c.key === d.key)) misfit(`${d.op} ${d.key} at ${i + 1}`);
        const c = this.buildContainer(st, d.key);
        if ("placement_after" in d) setPlacement(c, d.placement_after);
        if ("parent_after" in d) setParent(c, d.parent_after);
        cs.splice(i, 0, c);
        this.lists.set(d.key, []);
        dirty.add(d.key);
        return;
      }
      case "delete": {
        const [c] = cs.splice(here(at(d.position_before)), 1);
        this.goneContainers.set(c.key, c);
        this.lists.delete(c.key);
        return;
      }
      case "reorder": {
        const [c] = cs.splice(here(at(d.position_before)), 1);
        const j = at(d.position_after);
        if (j > cs.length) misfit(`reorder ${d.key} to ${j + 1}`);
        cs.splice(j, 0, c);
        return;
      }
      default: {
        const c = cs.find((x) => x.key === d.key) ?? misfit(`${d.op} of ${d.key}, not in the view`);
        const cf = this.vs.container_fields;
        for (const n of ["title", "hidden", "layout"] as const) {
          const src = cf[n];
          const f = srcField(src);
          if ((f !== null && d.fields.includes(f)) || (isListValue(src) && d.op === "props")) this.containerField(st, c, n);
        }
        if (d.op === "hide_show" && d.hidden_after !== null && !this.vs.derived.containers.includes("hidden")) c.hidden = d.hidden_after;
        if ("placement_after" in d) setPlacement(c, d.placement_after);
        // the kit sends a container's parent (an address it mapped from the store's key): never
        // read from the keyed state
        if ("parent_after" in d) setParent(c, d.parent_after);
      }
    }
  }

  // ---------- units ----------

  private unitField(st: KeyedState, u: ViewUnit, name: keyof ViewSchemaJson["unit_fields"]): void {
    if (this.vs.derived.units.includes(name)) return;
    const v = this.read(st, true, u.key, this.vs.unit_fields[name]);
    switch (name) {
      case "kind":
        u.kind = v ?? "";
        return;
      case "name":
        u.name = cutText(v ?? "")[0];
        return;
      case "alt":
        u.alt = v ? cutText(v)[0] : null;
        return;
      case "text": {
        if (v === null) u.text = null;
        else {
          const [summary, truncated] = cutText(v);
          u.text = { summary, truncated };
        }
        return;
      }
      case "hidden":
        u.hidden = flag(v);
        return;
      case "editable_text":
        u.editable_text = v !== null && EDITABLE.has(v) ? (v as ViewUnit["editable_text"]) : null;
        return;
      case "placeholder": {
        if (v === null) u.placeholder = null;
        else {
          const i = v.indexOf(":");
          const [ty, idx] = i < 0 ? [v, ""] : [v.slice(0, i), v.slice(i + 1)];
          u.placeholder = { type: ty, idx: idx ? idx : null };
        }
        return;
      }
    }
  }

  private setGeom(u: ViewUnit, v: string | null): void {
    const g = this.vs.geometry;
    if (!g || ["bbox", "rot", "flip"].some((n) => this.vs.derived.units.includes(n))) return;
    const p = parseGeom(v, g.parts);
    u.bbox = p ? p.bbox : null;
    u.rot = p ? p.rot : 0;
    u.flip = p ? p.flip : { h: false, v: false };
  }

  private buildUnit(st: KeyedState, key: string, container: string): ViewUnit {
    const prev = this.gone.get(key);
    this.gone.delete(key);
    const u: ViewUnit = prev ?? {
      key,
      container,
      parent: null,
      live: true,
      kind: "",
      name: "",
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
    };
    u.container = container;
    for (const n of ["kind", "name", "alt", "text", "hidden", "editable_text", "placeholder"] as const) this.unitField(st, u, n);
    const g = this.vs.geometry;
    if (g) this.setGeom(u, this.mapGet(st, this.umap, key + SEP + g.field));
    return u;
  }

  /** One unit delta. */
  private unitDelta(d: UnitDelta, st: KeyedState, dirty: Set<string>): void {
    const was = this.shown.get(d.key);
    if (was) dirty.add(was.container);
    if (d.ops.includes("delete")) {
      if (!was) misfit(`delete of ${d.key}, not in the view`);
      this.shown.delete(d.key);
      this.gone.set(d.key, was!);
      return;
    }
    const container = d.container ?? misfit(`${d.key} with no container`);
    if (!this.lists.has(container)) misfit(`${d.key} in ${container}, not in the view`);
    dirty.add(container);
    if (d.ops.includes("add") || d.ops.includes("restore")) {
      if (was) misfit(`${d.ops[0]} of ${d.key}, already in the view`);
      const u = this.buildUnit(st, d.key, container);
      if ("placement_after" in d) setPlacement(u, d.placement_after);
      this.shown.set(d.key, u);
      return;
    }
    const u = was ?? misfit(`${d.ops.join(",")} of ${d.key}, not in the view`);
    u.container = container;
    const uf = this.vs.unit_fields;
    for (const n of ["kind", "name", "alt", "text", "hidden", "editable_text", "placeholder"] as const) {
      const src = uf[n];
      const f = srcField(src);
      if ((f !== null && d.fields.includes(f)) || (isListValue(src) && d.ops.includes("props"))) this.unitField(st, u, n);
    }
    const g = this.vs.geometry;
    if (g && d.fields.includes(g.field)) this.setGeom(u, d.geom_after);
    if (d.ops.includes("hide_show") && d.hidden_after !== null && !this.vs.derived.units.includes("hidden")) u.hidden = d.hidden_after;
    if ("placement_after" in d) setPlacement(u, d.placement_after);
  }

  /** Container `c`'s units again, in the units' list order: its shown units, each followed by
   * the nested units the pack derived under it; `z` from 0 among the top-level ones. */
  private relist(c: string, st: KeyedState): void {
    if (!this.lists.has(c)) return;
    const prev = this.lists.get(c)!;
    const nested = new Map<string, ViewUnit[]>();
    let top: string | null = null;
    for (const u of prev) {
      if (u.parent === null) top = u.key;
      else if (top !== null) nested.set(top, [...(nested.get(top) ?? []), u]);
    }
    const r = st.roots.get(this.vs.units);
    const order = r && r.kind === "list" ? r.live : [];
    const out: ViewUnit[] = [];
    let z = 0;
    for (const [k] of order) {
      const u = this.shown.get(k);
      if (!u || u.container !== c) continue;
      u.z = z++;
      out.push(u, ...(nested.get(k) ?? []));
    }
    this.lists.set(c, out);
  }

  private repage(st: KeyedState): void {
    const p = this.vs.page!;
    const w = this.mapGet(st, p.root, p.w);
    const h = this.mapGet(st, p.root, p.h);
    const wn = w === null ? null : int(w);
    const hn = h === null ? null : int(h);
    this.snapshot.page = wn === null || hn === null ? null : { w: wn, h: hn, unit: this.vs.native_unit };
  }
}
