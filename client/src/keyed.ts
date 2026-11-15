// `KeyedWatch`: a watcher's copy of a live keyed doc (a deck, a board) from the kit's keyed
// watch (WIRE.md, "The keyed watch"): a snapshot in pages, then one numbered frame per change.
// Data and events, no DOM; it never writes the doc.

import { apply, validate, type Hunk } from "./hunk";
import type {
  ErrorReport,
  GridSlot,
  KeyedPageFrame,
  KeyedSchema,
  KeyedSnapshotFrame,
  KeyedValue,
  OpsChangeFrame,
  PlacementJson,
  ServerFrame,
  SpaceJson,
  Transport,
  ViewSnapshotJson,
  OutsideSaveFrame,
  ViewUnit,
  VerbRefusedFrame,
  WatchOp,
  Welcome,
} from "./wire";
import { KeyedView, ViewMisfit } from "./view";
import { worldMatrix, type Mat4 } from "./placement";

/** The name a host lists in `welcome.features` when it serves the keyed watch. */
export const KEYED_WATCH_FEATURE = "keyed_watch";

/** How many change frames go by between two `report`s (the default). */
export const REPORT_EVERY = 64;

/** One root of a watcher's state. */
export type KeyedRootState =
  | { kind: "list"; live: [string, KeyedValue][]; hidden: Map<string, KeyedValue> }
  | { kind: "map"; entries: Map<string, KeyedValue> }
  | { kind: "text"; text: string };

/** A keyed doc's state as a watcher holds it: its roots in the schema's order. */
export interface KeyedState {
  schema: KeyedSchema;
  roots: Map<string, KeyedRootState>;
}

/** Why the watch asked the host for a fresh snapshot. */
export type ResyncReason =
  /** The first ask on a connection. */
  | "start"
  /** A connection after a reconnect. */
  | "reconnect"
  /** A numbered frame went missing (a skipped `seq`). */
  | "gap"
  /** A frame didn't fit the state (an op on what isn't there, a change at or before the state's
   * version): never guessed at. */
  | "inconsistent"
  /** The host's code asked (`resync()`). */
  | "asked";

export interface KeyedWatchOptions {
  /** Change frames between two `report`s (default `REPORT_EVERY`). */
  reportEvery?: number;
}

const SEP = "\u001f";

/** The map key a watch op or a snapshot entry names: `key`, or `<key>\x1f<field>`. */
export function mapKey(key: string, field: string | null): string {
  return field === null ? key : key + SEP + field;
}

/** Compare by UTF-8 bytes (the kit's order for map keys): code point order. */
function compareUtf8(a: string, b: string): number {
  const x = a[Symbol.iterator]();
  const y = b[Symbol.iterator]();
  for (;;) {
    const p = x.next();
    const q = y.next();
    if (p.done || q.done) return p.done && q.done ? 0 : p.done ? -1 : 1;
    const d = p.value.codePointAt(0)! - q.value.codePointAt(0)!;
    if (d !== 0) return d;
  }
}

/** A JSON string as the kit's writer prints it: `"` `\` `\n` `\r` `\t` escaped, every other
 * control character, U+2028 and U+2029 as `\u00xx`, the rest as is. */
function jsonString(s: string): string {
  let out = '"';
  for (const ch of s) {
    const c = ch.codePointAt(0)!;
    if (ch === '"') out += '\\"';
    else if (ch === "\\") out += "\\\\";
    else if (ch === "\n") out += "\\n";
    else if (ch === "\r") out += "\\r";
    else if (ch === "\t") out += "\\t";
    else if (c < 0x20 || c === 0x2028 || c === 0x2029) out += "\\u" + c.toString(16).padStart(4, "0");
    else out += ch;
  }
  return out + '"';
}

const jsonValue = (v: KeyedValue) => (v === null ? "null" : jsonString(v));
const pair = (k: string, v: KeyedValue) => `[${jsonString(k)},${jsonValue(v)}]`;

/**
 * A keyed state as canonical JSON, byte for byte the kit's `keyed_state_json`: every root in
 * schema order, a list's live elements in order and its deleted ones by key, a map's entries by
 * key (UTF-8 byte order), a text whole. Two states are the same when this is the same.
 */
export function keyedStateJson(state: KeyedState): string {
  const roots: string[] = [];
  for (const [name, r] of state.roots) {
    const head = `{"name":${jsonString(name)},"kind":"${r.kind}"`;
    if (r.kind === "text") roots.push(`${head},"text":${jsonString(r.text)}}`);
    else if (r.kind === "list") {
      const hidden = [...r.hidden].sort((a, b) => compareUtf8(a[0], b[0]));
      roots.push(`${head},"live":[${r.live.map(([k, v]) => pair(k, v)).join(",")}],"hidden":[${hidden.map(([k, v]) => pair(k, v)).join(",")}]}`);
    } else {
      const entries = [...r.entries].sort((a, b) => compareUtf8(a[0], b[0]));
      roots.push(`${head},"entries":[${entries.map(([k, v]) => pair(k, v)).join(",")}]}`);
    }
  }
  return `{"roots":[${roots.join(",")}]}`;
}

function emptyState(schema: KeyedSchema): KeyedState {
  const roots = new Map<string, KeyedRootState>();
  for (const r of schema.roots) {
    if (r.kind === "text") roots.set(r.name, { kind: "text", text: "" });
    else if (r.kind === "list") roots.set(r.name, { kind: "list", live: [], hidden: new Map() });
    else if (r.kind === "map") roots.set(r.name, { kind: "map", entries: new Map() });
  }
  return { schema, roots };
}

/** Thrown inside when a frame doesn't fit: the watch asks for a snapshot. */
class Misfit extends Error {}

function fail(why: string): never {
  throw new Misfit(why);
}

function has(o: object, k: string): boolean {
  return Object.prototype.hasOwnProperty.call(o, k);
}

/** One op on the state, as the kit's live view applies it; anything that doesn't fit throws. */
function applyOp(state: KeyedState, o: WatchOp): void {
  const r = state.roots.get(o.root) ?? fail(`no root ${o.root}`);
  if (o.kind === "text") {
    if (r.kind !== "text") fail(`${o.root} isn't a text`);
    const hunks: Hunk[] = o.hunks;
    if (validate(r.text, hunks)) fail(`hunks on ${o.root} don't fit`);
    r.text = apply(r.text, hunks);
    return;
  }
  if (o.kind === "map") {
    if (r.kind !== "map") fail(`${o.root} isn't a map`);
    const k = mapKey(o.key, o.field);
    if (o.op === "set") {
      if (!has(o, "after")) fail(`set of ${k} without a value`);
      r.entries.set(k, o.after ?? null);
    } else if (!r.entries.delete(k)) fail(`delete of ${k}, absent`);
    return;
  }
  if (r.kind !== "list") fail(`${o.root} isn't a list`);
  const { live, hidden } = r;
  const lo: Extract<WatchOp, { kind: "list" }> = o;
  const at = (i: number | null) => (i !== null && i < live.length && live[i][0] === lo.key ? i : fail(`${lo.op} ${lo.key}: not at ${i}`));
  const after = (): KeyedValue => (has(lo, "after") ? (lo.after ?? null) : fail(`${lo.op} ${lo.key} without a value`));
  if (o.effect === "hidden") {
    if (o.op === "set") {
      if (!hidden.has(o.key)) fail(`hidden set of ${o.key}, not deleted`);
      hidden.set(o.key, after());
    } else if (o.op === "drop") {
      if (!hidden.delete(o.key)) fail(`drop of ${o.key}, not deleted`);
    } else if (o.op !== "move") fail(`hidden ${o.op}`);
    return;
  }
  switch (o.op) {
    case "insert": {
      const i = o.index_after;
      if (i === null || i > live.length) fail(`insert of ${o.key} at ${i}`);
      if (hidden.has(o.key) || live.some(([k]) => k === o.key)) fail(`insert of ${o.key}, already there`);
      live.splice(i, 0, [o.key, after()]);
      return;
    }
    case "move": {
      const b = at(o.index_before);
      const a = o.index_after;
      if (a === null || a >= live.length) fail(`move of ${o.key} to ${a}`);
      const [e] = live.splice(b, 1);
      live.splice(a, 0, e);
      return;
    }
    case "delete": {
      const [[k, v]] = live.splice(at(o.index_before), 1);
      hidden.set(k, v);
      return;
    }
    case "restore": {
      const a = o.index_after;
      if (a === null || a > live.length) fail(`restore of ${o.key} at ${a}`);
      if (!hidden.has(o.key)) fail(`restore of ${o.key}, not deleted`);
      const v = hidden.get(o.key)!;
      hidden.delete(o.key);
      live.splice(a, 0, [o.key, v]);
      return;
    }
    case "set":
      live[at(o.index_after)][1] = after();
      return;
    case "drop":
      if (!hidden.delete(o.key)) fail(`drop of ${o.key}, not deleted`);
      return;
  }
}

/** One page's run onto the state being built (a view index's units onto `units`). */
function applyPage(state: KeyedState, units: Map<string, ViewUnit[]>, f: KeyedPageFrame): void {
  const parts = ["live", "hidden", "entries", "text", "units"].filter((k) => has(f, k));
  if (parts.length !== 1) fail(`page ${f.page} holds ${parts.join(", ") || "nothing"}`);
  if (f.units) {
    const have = units.get(f.units.container) ?? [];
    if (f.units.from !== have.length) fail(`units of ${f.units.container} from ${f.units.from}, after ${have.length}`);
    units.set(f.units.container, [...have, ...f.units.units]);
    return;
  }
  const r = state.roots.get(f.root) ?? fail(`page for a root not in the schema: ${f.root}`);
  if (f.live && r.kind === "list") r.live.push(...f.live.map(([k, v]): [string, KeyedValue] => [k, v]));
  else if (f.hidden && r.kind === "list") for (const [k, v] of f.hidden) r.hidden.set(k, v);
  else if (f.entries && r.kind === "map") for (const [k, field, v] of f.entries) r.entries.set(mapKey(k, field), v);
  else if (typeof f.text === "string" && r.kind === "text") r.text += f.text;
  else fail(`page ${f.page} doesn't fit root ${f.root}`);
}

/**
 * A watcher's copy of a live keyed doc over any `Transport`: it asks for a snapshot (`watch`),
 * builds the state from its pages, then applies each change frame, in `seq` order. A skipped
 * `seq` (a lost frame) or a frame that doesn't fit makes it ask for a fresh snapshot and drop
 * everything until one comes: it never guesses. Presence, roster and claim frames are never
 * numbered, so they never count. It can share a transport with a `PresenceChannel`.
 *
 * It sends only on a connection whose `welcome.features` lists `"keyed_watch"`; otherwise it
 * sends nothing, reads nothing and has no state.
 */
export class KeyedWatch {
  private readonly t: Transport;
  private readonly unsub: () => void;
  private welcome: Welcome;
  private closed = false;
  private _seq = 0;
  private _rev: number | null = null;
  private st: KeyedState | null = null;
  private building: {
    state: KeyedState;
    units: Map<string, ViewUnit[]>;
    view: ViewSnapshotJson | null;
    derived: Map<string, string>;
    got: number;
    pages: number;
    upto: number;
  } | null = null;
  private kv: KeyedView | null = null;
  /** The derived values kept by cell field (`cellDerived`), by `derivedKey`. */
  private cd = new Map<string, string>();
  /** A `watch` is out: every frame before the next snapshot's head is dropped. */
  private awaiting = false;
  private sinceReport = 0;
  private readonly reportEvery: number;
  private readonly changeCbs = new Set<(c: OpsChangeFrame) => void>();
  private readonly snapshotCbs = new Set<(upto: number) => void>();
  private readonly resyncCbs = new Set<(why: ResyncReason, detail: string) => void>();
  private readonly refusedCbs = new Set<(e: ErrorReport) => void>();
  private readonly verbRefusedCbs = new Set<(r: VerbRefusedFrame) => void>();
  private readonly outsideSaveCbs = new Set<(o: OutsideSaveFrame) => void>();

  constructor(transport: Transport, welcome: Welcome, opts: KeyedWatchOptions = {}) {
    this.t = transport;
    this.welcome = welcome;
    this.reportEvery = Math.max(1, opts.reportEvery ?? REPORT_EVERY);
    this.unsub = transport.onFrame((f) => this.onFrame(f));
    this.ask("start", "");
  }

  /** The connection's `welcome.features` lists `"keyed_watch"`. */
  get supported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(KEYED_WATCH_FEATURE);
  }

  /** The last numbered frame applied (0: none on this connection). */
  get seq(): number {
    return this._seq;
  }

  /** The doc's version the state is at (`null`: no state yet). */
  get rev(): number | null {
    return this._rev;
  }

  /** A whole snapshot was applied, and nothing since failed to fit. */
  get ready(): boolean {
    return this.st !== null && !this.awaiting && this.building === null;
  }

  /** The state: read it, never change it (`null` before the first snapshot). */
  get state(): KeyedState | null {
    return this.st;
  }

  /** The view index (the contract's view schema, page, fonts, layouts and containers, by
   * address), as the last whole snapshot carried it and each change's view deltas have kept it
   * since (`KeyedView`); `null` when the pack has no view schema. A field the pack derives keeps
   * the snapshot's value (its changes ride on the frames' `derived`). */
  get view(): ViewSnapshotJson | null {
    return this.kv?.snapshot ?? null;
  }

  /** The view index's units of the container with address `container`, back to front, kept as
   * `view` is (empty for a container with none). */
  units(container: string): readonly ViewUnit[] {
    return this.kv?.units(container) ?? [];
  }

  /** The view schema's live grid (`view.schema.grid`), `null` when the pack has none or there is
   * no view: what the grid helpers (`gridNumbers`, `cornerA1`...) read with `state`. */
  get grid(): GridSlot | null {
    return this.kv?.grid ?? null;
  }

  /**
   * A derived value the pack keeps by cell field (one of the grid's `cell_derived`, such as `a1`,
   * a formula's text as people read it) for field `field` of row `unit`, `null` when it has none.
   * A snapshot's rows pages carry them, so a cell's is here as soon as its page lands; each
   * change's `derived.units` updates them (`null` there removes one).
   */
  cellDerived(name: string, unit: string, field: string): string | null {
    return this.cd.get(derivedKey(name, unit, field)) ?? null;
  }

  /**
   * A unit's derived value (one of the view schema's `unit_derived`, such as a transform or a
   * label as people read it) for the unit with address `unit`, `null` when it has none or the
   * view doesn't show the unit. A snapshot's units pages carry each unit's own; each change's
   * `derived.units` updates them (`null` there removes one). Always `null` from an older kit.
   */
  unitDerived(name: string, unit: string): string | null {
    if (!this.kv?.snapshot.schema.unit_derived?.includes(name)) return null;
    return this.kv.unit(unit)?.derived?.[name] ?? null;
  }

  /**
   * A container's derived value (one of the view schema's `container_derived`, such as a
   * timeline track's label or kind) for the container with address `container`, `null` when it
   * has none or the view doesn't show the container. The snapshot's containers carry each one's
   * own; each change's `derived.containers` updates them (`null` there removes one). Always
   * `null` from an older kit.
   */
  containerDerived(name: string, container: string): string | null {
    if (!this.kv?.snapshot.schema.container_derived?.includes(name)) return null;
    return this.kv.snapshot.containers.find((c) => c.key === container)?.derived?.[name] ?? null;
  }

  /** The doc's space (a 3D slot), kept from the snapshot and each change's `view_space`;
   * `null` when the pack has none or from an older kit. */
  get space(): SpaceJson | null {
    return this.kv?.snapshot.space ?? null;
  }

  /** The placement of the unit or container with address `address` (a 3D slot), kept from the
   * snapshot's pages and each change's `placement_after`; `null` when it has none or the view
   * doesn't show it. */
  placement(address: string): PlacementJson | null {
    return this.kv?.placementOf(address) ?? null;
  }

  /** The world matrix of the unit or container with address `address` in the doc's space
   * (`placement.ts`, `worldMatrix`), at the slot's precision; `null` when it has no placement or
   * its chain is broken. */
  worldMatrix(address: string): Mat4 | null {
    const kv = this.kv;
    if (!kv) return null;
    const precision = kv.snapshot.schema.space?.precision === "f32" ? "f32" : "f64";
    return worldMatrix((a) => kv.placementOf(a), address, precision);
  }

  /** The view index's unit with address `address` (a nested one too), kept as `units` is;
   * `null` when the view doesn't show it. */
  unit(address: string): ViewUnit | null {
    return this.kv?.unit(address) ?? null;
  }

  /** The view is the doc's: every change since the snapshot carried its view deltas. `false`
   * after a change from a kit that sends none (an older kit), until the next snapshot. */
  get viewCurrent(): boolean {
    return this.kv?.current ?? false;
  }

  /** One root of the state. */
  root(name: string): KeyedRootState | undefined {
    return this.st?.roots.get(name);
  }

  /** The state as canonical JSON (`keyedStateJson`), `null` before the first snapshot. */
  stateJson(): string | null {
    return this.st && keyedStateJson(this.st);
  }

  /** Each change applied, after it is. */
  onChange(cb: (c: OpsChangeFrame) => void): () => void {
    this.changeCbs.add(cb);
    return () => this.changeCbs.delete(cb);
  }

  /** A snapshot applied whole: the state is the doc at `upto`. */
  onSnapshot(cb: (upto: number) => void): () => void {
    this.snapshotCbs.add(cb);
    return () => this.snapshotCbs.delete(cb);
  }

  /** The watch asked for a fresh snapshot, and why. */
  onResync(cb: (why: ResyncReason, detail: string) => void): () => void {
    this.resyncCbs.add(cb);
    return () => this.resyncCbs.delete(cb);
  }

  /** The host can't serve the watch (`watch_refused`). */
  onRefused(cb: (e: ErrorReport) => void): () => void {
    this.refusedCbs.add(cb);
    return () => this.refusedCbs.delete(cb);
  }

  /** A verb on the doc was refused (`verb_refused`): who, the verb, the unit by address, the
   * code and the holder. Unnumbered and data only: it changes nothing here. */
  onVerbRefused(cb: (r: VerbRefusedFrame) => void): () => void {
    this.verbRefusedCbs.add(cb);
    return () => this.verbRefusedCbs.delete(cb);
  }

  /** What came of an outside save of the doc's file (`outside_save`): its outcome, after the
   * save's own change. Unnumbered and data only: it changes nothing here. */
  onOutsideSave(cb: (o: OutsideSaveFrame) => void): () => void {
    this.outsideSaveCbs.add(cb);
    return () => this.outsideSaveCbs.delete(cb);
  }

  /** Ask for a fresh snapshot now (the host suspects lost frames). */
  resync(): void {
    this.ask("asked", "");
  }

  /** A new connection: its numbering starts again; the state is kept until the new snapshot
   * replaces it. */
  reconnected(welcome: Welcome): void {
    this.welcome = welcome;
    this._seq = 0;
    this.building = null;
    this.ask("reconnect", "");
  }

  /** Stop: nothing more is read or sent. */
  close(): void {
    this.closed = true;
    this.unsub();
  }

  /** Whether `seq` is the next number (a frame skipped otherwise). */
  inOrder(seq: number): boolean {
    return seq === this._seq + 1;
  }

  private ask(why: ResyncReason, detail: string): void {
    if (this.closed || !this.supported) return;
    this.awaiting = true;
    this.building = null;
    this.t.send({ type: "watch" });
    for (const cb of this.resyncCbs) cb(why, detail);
  }

  private report(): void {
    this.sinceReport = 0;
    if (this.supported) this.t.send({ type: "report", seq: this._seq });
  }

  private onFrame(f: ServerFrame): void {
    if (this.closed || !this.supported) return;
    try {
      switch (f.type) {
        case "keyed_snapshot":
          return this.onHead(f);
        case "keyed_page":
          return this.onPage(f);
        case "ops_change":
          return this.onOps(f);
        case "watch_refused":
          this.awaiting = false;
          for (const cb of this.refusedCbs) cb(f.error);
          return;
        case "verb_refused":
          for (const cb of this.verbRefusedCbs) cb(f);
          return;
        case "outside_save":
          for (const cb of this.outsideSaveCbs) cb(f);
          return;
      }
    } catch (e) {
      if (!(e instanceof Misfit)) throw e;
      this.ask("inconsistent", e.message);
    }
  }

  /** A snapshot's head: always taken (it replaces everything), whatever came before. */
  private onHead(f: KeyedSnapshotFrame): void {
    this.awaiting = false;
    this._seq = f.seq;
    this.building = { state: emptyState(f.schema), units: new Map(), view: f.view ?? null, derived: new Map(), got: 0, pages: f.pages, upto: f.upto };
    if (f.pages === 0) this.finish();
  }

  private onPage(f: KeyedPageFrame): void {
    if (this.awaiting) return;
    if (!this.inOrder(f.seq)) return this.ask("gap", `page seq ${f.seq} after ${this._seq}`);
    const b = this.building ?? fail(`a page with no snapshot (seq ${f.seq})`);
    if (f.page !== b.got + 1 || f.pages !== b.pages || f.upto !== b.upto) fail(`page ${f.page} of ${f.pages} out of order`);
    applyPage(b.state, b.units, f);
    if (f.derived) {
      const names = new Set(b.view?.schema.grid?.cell_derived ?? []);
      for (const [name, byUnit] of Object.entries(f.derived)) {
        if (!names.has(name)) continue;
        for (const [unit, byField] of Object.entries(byUnit)) {
          for (const [field, text] of Object.entries(byField)) if (typeof text === "string") b.derived.set(derivedKey(name, unit, field), text);
        }
      }
    }
    b.got++;
    this._seq = f.seq;
    if (b.got === b.pages) this.finish();
  }

  private finish(): void {
    const b = this.building!;
    this.building = null;
    this.st = b.state;
    this.kv = b.view ? new KeyedView(b.view, b.units, b.state.schema) : null;
    this.cd = b.derived;
    this._rev = b.upto;
    this.report();
    for (const cb of this.snapshotCbs) cb(b.upto);
  }

  private onOps(f: OpsChangeFrame): void {
    if (this.awaiting) return;
    if (!this.inOrder(f.seq)) return this.ask("gap", `change seq ${f.seq} after ${this._seq}`);
    if (this.building || !this.st) fail(`a change with no whole snapshot (seq ${f.seq})`);
    const st = this.st!;
    if (this._rev !== null && f.rev <= this._rev) fail(`a change at rev ${f.rev}, at or before ${this._rev}`);
    for (const o of f.ops) applyOp(st, o);
    try {
      this.kv?.apply(f, st);
    } catch (e) {
      if (e instanceof ViewMisfit) fail(`view: ${e.message}`);
      throw e;
    }
    this.kv?.applyDerived(f);
    this.keepCellDerived(f);
    this._seq = f.seq;
    this._rev = f.rev;
    if (++this.sinceReport >= this.reportEvery) this.report();
    for (const cb of this.changeCbs) cb(f);
  }

  /** A change's `derived.units` into the values kept by cell field: a text sets one, `null`
   * removes it; names the grid doesn't keep by cell field are left alone. */
  private keepCellDerived(f: OpsChangeFrame): void {
    const units = f.derived?.units;
    const names = this.grid?.cell_derived;
    if (!units || !names) return;
    for (const [unit, d] of Object.entries(units)) {
      for (const name of names) {
        const byField = d[name];
        if (!byField || typeof byField !== "object") continue;
        for (const [field, text] of Object.entries(byField as Record<string, unknown>)) {
          const k = derivedKey(name, unit, field);
          if (text === null) this.cd.delete(k);
          else if (typeof text === "string") this.cd.set(k, text);
        }
      }
    }
  }
}

/** The key of one derived value kept by cell field. */
function derivedKey(name: string, unit: string, field: string): string {
  return name + SEP + unit + SEP + field;
}
