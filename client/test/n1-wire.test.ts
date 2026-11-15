// Wire 3's N−1, both ways (WIRE.md, "Wire versions"), run by `tools/n1/n1.sh node-wire`, which
// puts the previous release's client code and goldens in `test/n1-prev/` (`src/`, `golden/`).
// Without them (every other run) the suite is skipped.
//
// - The previous client (wire 2) on this kit's goldens (wire 3): change events with `turn` and
//   `verb` replay to the kit's text, and every roster stream (focus, range, claim events with
//   actor and times) folds without error to the kit's final roster, as the previous client knows
//   it. The live grid's stream (`cells`, `claim_started_cells`; still wire 3) folds on it without
//   error: its roster events are kinds it doesn't know.
// - This client (wire 3) on the previous release's goldens (wire 2): the same, with no `turn`, no
//   focus or range, and claims attributed from the roster.
// - This client on a wire-2 welcome sends no frame a wire-2 host doesn't take.
//
// The previous release's wire is its `WIRE_VERSION` (src/wire.ts; none there: 2). Above is a wire-2
// release (v0.14.0). A wire-3 one (v0.15.0 and later) is a plain round trip: this client reads
// its turns, verbs and claim times as they are.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KeyedWatch } from "../src/keyed";
import { LiveText } from "../src/live";
import { PresenceChannel } from "../src/presence";
import type { ChangeEvent, ClientFrame, RosterEntry, RosterEvent, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
const PREV = join(HERE, "n1-prev");
const OURS = join(HERE, "golden");
const has = existsSync(join(PREV, "src", "presence.ts")) && existsSync(join(PREV, "golden"));
/** The previous release's wire version: its `WIRE_VERSION`, or 2 before it had one. */
const prevWire = has ? Number(/export const WIRE_VERSION = (\d+);/.exec(readFileSync(join(PREV, "src", "wire.ts"), "utf8"))?.[1] ?? 2) : 2;

type Change = { root: string; version: number; text: string; events: ChangeEvent[]; final_version: number; final_text: string };
type Roster = { self: string; client?: number; events: RosterEvent[]; final: RosterEvent };

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

const load = <T>(dir: string, name: string): T => JSON.parse(readFileSync(join(dir, name), "utf8")) as T;
/** The `{events, final}` roster goldens. `roster_events_cells.json` is left out here: it is a
 * stream in flushes (`{self, client, flushes}`) with no final snapshot, and its own row below
 * folds it on the previous client. */
const rosterFiles = (dir: string) => readdirSync(dir).filter((n) => n.startsWith("roster_events") && n.endsWith(".json") && !/rollup|closed|grid|cells/.test(n));
/** The streams a `PresenceChannel` folds (element presence and claims), where both have them. */
const PRESENCE_FILES = ["roster_events_elem_presence.json", "roster_events_claims_elems.json", "roster_events_focus_range.json", "roster_events_view_time.json"];
const handlesOf = (e: RosterEvent) => (e.kind === "snapshot" ? (e.members as RosterEntry[]).map((m) => m.handle).sort() : []);
const now = { schedule: (deliver: () => void) => deliver() };

/** The client modules a run uses: this one's, or the previous release's. */
interface Client {
  LiveText: typeof LiveText;
  PresenceChannel: typeof PresenceChannel;
}

async function previous(): Promise<Client> {
  const live = (await import(/* @vite-ignore */ join(PREV, "src", "live.ts"))) as { LiveText: typeof LiveText };
  const presence = (await import(/* @vite-ignore */ join(PREV, "src", "presence.ts"))) as { PresenceChannel: typeof PresenceChannel };
  return { LiveText: live.LiveText, PresenceChannel: presence.PresenceChannel };
}

/** `client` reads every change and roster golden in `dir`: the changes replay to the kit's text,
 * the LiveText roster holds the final snapshot's members, and a PresenceChannel folds the stream
 * to the same peers as the final snapshot alone. Returns how many files it read. */
function readsAll(client: Client, dir: string): number {
  let n = 0;
  for (const name of ["change_events.json", "change_events_app.json"]) {
    const g = load<Change>(dir, name);
    const w = wire();
    const l = new client.LiveText({ text: g.text, version: g.version }, w.t, { root: g.root });
    for (const event of g.events) w.deliver({ type: "change", event });
    expect(l.text, name).toBe(g.final_text);
    expect(l.version, name).toBe(g.final_version);
    expect(w.sent, name).toEqual([]);
    n++;
  }
  for (const name of rosterFiles(dir)) {
    const g = load<Roster>(dir, name);
    const w = wire();
    const l = new client.LiveText({ text: "", version: 0 }, w.t, { self: g.self ?? undefined });
    const welcome: Welcome = { root: "source", version: 0, client: g.client ?? 1, self: g.self, rights: "edit" };
    const p = new client.PresenceChannel(w.t, welcome, now);
    for (const event of g.events) w.deliver({ type: "roster_event", event });
    expect(l.roster.map((m) => m.handle).sort(), name).toEqual(handlesOf(g.final));
    n++;
    if (!PRESENCE_FILES.includes(name)) continue;
    const w2 = wire();
    const late = new client.PresenceChannel(w2.t, welcome, now);
    w2.deliver({ type: "roster_event", event: g.final });
    const rows = (c: PresenceChannel) => c.peers().map((x) => [x.handle, x.client ?? null, JSON.stringify(x.selection), JSON.stringify(x.pointer), JSON.stringify(x.viewport)]).sort();
    expect(rows(p), name).toEqual(rows(late));
  }
  return n;
}

/** The container fields added after v0.18.0 (WIRE.md, "Containers in containers", "Derived
 * values per container"): a previous client keeps the snapshot's copy and never updates them, so
 * an N-1 compare removes exactly these, by name, from both sides. */
const NEW_CONTAINER_FIELDS = ["parent", "derived"] as const;

/** `W`'s keyed watch on its own copy of `frames`: what it ends with, every container and unit
 * whole, and the kinds of frame it sent. */
function replayKeyed(W: typeof KeyedWatch, frames: ServerFrame[]) {
  const w = wire();
  const k = new W(w.t, { root: "objs", version: 1, client: 7, self: "ada", rights: "read", features: ["keyed_watch"] });
  for (const f of structuredClone(frames)) w.deliver(f);
  const containers = structuredClone(k.view?.containers ?? []);
  const units = (k.view?.containers ?? []).map((c) => [c.key, structuredClone(k.units(c.key))]);
  return { ready: k.ready, state: k.stateJson(), rev: k.rev, current: k.viewCurrent, containers, units, sent: w.sent.map((f) => f.type) };
}

/** `r` with the fields in `NEW_CONTAINER_FIELDS` taken off every container. */
function withoutNewFields(r: ReturnType<typeof replayKeyed>) {
  return {
    ...r,
    containers: r.containers.map((c) => {
      const x: Record<string, unknown> = { ...c };
      for (const f of NEW_CONTAINER_FIELDS) delete x[f];
      return x;
    }),
  };
}

describe.skipIf(!has)("wire N−1 both ways (tools/n1/n1.sh node-wire)", () => {
  it("the previous client reads this kit's goldens (wire 3)", async () => {
    const prev = await previous();
    const n = readsAll(prev, OURS);
    expect(n).toBeGreaterThanOrEqual(10);
    // the wire-3 additions are in what it read: a turn, focus and range, a claim's actor
    const turns = load<Change>(OURS, "change_events.json").events.filter((e) => typeof e.turn === "string");
    expect(turns.length).toBeGreaterThan(0);
    const kinds = new Set(load<Roster>(OURS, "roster_events_focus_range.json").events.map((e) => e.kind));
    expect(kinds.has("focus") && kinds.has("range")).toBe(true);
    // a pack verb's change (`verb`, after v0.14.0) replays on the previous client as any other
    const g = load<Change>(OURS, "change_events_verb.json");
    expect(g.events.some((e) => typeof e.verb === "string")).toBe(true);
    const w = wire();
    const l = new prev.LiveText({ text: g.text, version: g.version }, w.t, { root: g.root });
    for (const event of g.events) w.deliver({ type: "change", event });
    expect(l.text).toBe(g.final_text);
    expect(l.version).toBe(g.final_version);
    expect(w.sent).toEqual([]);
  });

  it("this client (wire 3) reads the previous release's goldens", () => {
    expect([2, 3]).toContain(prevWire);
    const n = readsAll({ LiveText, PresenceChannel }, join(PREV, "golden"));
    expect(n).toBeGreaterThanOrEqual(8);
    const changes = load<Change>(join(PREV, "golden"), "change_events.json").events;
    if (prevWire >= 3) {
      // wire 3 both sides: its turns are there, and read as they are
      expect(changes.some((e) => typeof e.turn === "string")).toBe(true);
    } else {
      // no turn or verb there: none read
      for (const e of changes) {
        expect(e.turn).toBeUndefined();
        expect(e.verb).toBeUndefined();
      }
    }
    // claims: from a wire-2 stream, attributed from the roster with no start time; from a
    // wire-3 one, with the actor and start time the stream carries
    const g = load<Roster>(join(PREV, "golden"), "roster_events_claims_elems.json");
    const w = wire();
    const p = new PresenceChannel(w.t, { root: "source", version: 0, client: 1, self: g.self, rights: "edit" }, now);
    for (const event of g.events) w.deliver({ type: "roster_event", event });
    expect(p.claims().length).toBeGreaterThan(0);
    for (const c of p.claims()) {
      expect(c.actor.name.length).toBeGreaterThan(0);
      if (prevWire >= 3) expect(typeof c.startedMs).toBe("number");
      else expect(c.startedMs).toBeNull();
    }
  });

  it("the previous client ignores the keyed watch's frames (kit after v0.15.1, still wire 3)", async () => {
    const prev = await previous();
    const g = load<{ frames: ServerFrame[] }>(OURS, "keyed_frames.json");
    expect(g.frames.length).toBeGreaterThan(5);
    const w = wire();
    const l = new prev.LiveText({ text: "x", version: 3 }, w.t, { root: "source" });
    const welcome: Welcome = { root: "source", version: 3, client: 1, self: "ada", rights: "read", features: ["presence", "claims", "keyed_watch"] };
    const p = new prev.PresenceChannel(w.t, welcome, now);
    for (const f of g.frames) w.deliver(f);
    expect(l.text).toBe("x");
    expect(l.version).toBe(3);
    expect(p.peers()).toEqual([]);
    expect(w.sent).toEqual([]);
  });

  it("the previous client folds this kit's view addresses and time positions (after v0.18.0) without error, to the same peers", async () => {
    const name = "roster_events_view_time.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const prev = await previous();
    const g = load<Roster>(OURS, name);
    const kinds = new Set(g.events.map((e) => e.kind as string));
    expect(kinds.has("view_address") && kinds.has("time_position")).toBe(true);
    const w = wire();
    const welcome: Welcome = { root: "source", version: 0, client: g.client ?? 1, self: g.self, rights: "edit" };
    const p = new prev.PresenceChannel(w.t, welcome, now);
    for (const event of g.events) w.deliver({ type: "roster_event", event });
    expect([...new Set(p.peers().map((x) => x.handle))].sort()).toEqual(["ada", "atlas", "bob"]);
    expect(w.sent).toEqual([]);
  });

  it("the previous client folds a claim gone in an outside save (after v0.18.0: `how: gone_outside_save`, `keys`) as an ended claim: the same claims and peers as this client, nothing sent", async () => {
    const name = "roster_events_claim_gone_outside.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const prev = await previous();
    const g = load<Roster>(OURS, name);
    const end = g.events.find((e) => e.kind === "claim_ended") as { how?: string; keys?: string[] } | undefined;
    expect(end?.how).toBe("gone_outside_save");
    expect(end?.keys).toEqual(["@item:i2"]);
    const fold = (P: typeof PresenceChannel) => {
      const w = wire();
      const welcome: Welcome = { root: "items", version: 0, client: g.client ?? 1, self: g.self, rights: "edit" };
      const p = new P(w.t, welcome, now);
      let held = 0;
      for (const event of g.events) {
        w.deliver({ type: "roster_event", event });
        if (event.kind === "claim_started_elems") held = p.claims().length;
      }
      return { held, claims: p.claims().length, peers: [...new Set(p.peers().map((x) => x.handle))].sort(), sent: w.sent.length };
    };
    const old = fold(prev.PresenceChannel);
    // the claim was held, then ended: the previous client drops it as this one does
    expect(old).toEqual({ held: 1, claims: 0, peers: old.peers, sent: 0 });
    expect(old).toEqual(fold(PresenceChannel));
  });

  it("the previous client's keyed watch reads this kit's frames with an outside_save frame (after v0.18.0): the same state as this client's", async () => {
    const g = load<{ frames: ServerFrame[]; final_json: string; final_rev: number }>(OURS, "keyed_frames.json");
    expect(g.frames.some((f) => (f.type as string) === "outside_save")).toBe(true);
    if (!existsSync(join(PREV, "src", "keyed.ts"))) return; // a previous release with no keyed watch
    const prevKeyed = (await import(/* @vite-ignore */ join(PREV, "src", "keyed.ts"))) as { KeyedWatch: typeof KeyedWatch };
    const welcome: Welcome = { root: "shapes", version: 1, client: 7, self: "ada", rights: "read", features: ["keyed_watch"] };
    const read = (W: typeof KeyedWatch) => {
      const w = wire();
      const k = new W(w.t, welcome);
      for (const f of g.frames) w.deliver(f);
      return { ready: k.ready, state: k.stateJson(), rev: k.rev, watches: w.sent.filter((f) => f.type === "watch").length };
    };
    const old = read(prevKeyed.KeyedWatch);
    expect(old).toEqual({ ready: true, state: g.final_json, rev: g.final_rev, watches: 1 });
    expect(read(KeyedWatch)).toEqual(old);
  });

  it("the previous client folds the live grid's roster stream (`cells`, `claim_started_cells`) without error and sends nothing", async () => {
    const prev = await previous();
    const name = "roster_events_cells.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const g = load<{ self: string; client: number; flushes: { rev: number; events: RosterEvent[] }[] }>(OURS, name);
    const kinds = new Set(g.flushes.flatMap((f) => f.events.map((e) => e.kind)));
    expect(kinds.has("cells") && kinds.has("claim_started_cells")).toBe(true);
    const w = wire();
    const l = new prev.LiveText({ text: "", version: 0 }, w.t, { self: g.self });
    const welcome: Welcome = { root: "source", version: 0, client: g.client, self: g.self, rights: "edit" };
    const p = new prev.PresenceChannel(w.t, welcome, now);
    for (const f of g.flushes) for (const event of f.events) w.deliver({ type: "roster_event", event });
    expect(l.roster.some((m) => m.handle === "agent:atlas")).toBe(true);
    expect(p.peers().some((x) => x.handle === "agent:atlas")).toBe(true);
    expect(w.sent).toEqual([]);
  });

  it("the previous client's keyed watch reads this kit's frames with units' derived values (after v0.17.0): the same state and units as this client's", async () => {
    const name = "keyed_frames_unit_derived.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const g = load<{ frames: ServerFrame[]; final_json: string; final_rev: number }>(OURS, name);
    if (!existsSync(join(PREV, "src", "keyed.ts"))) return; // a previous release with no keyed watch: the row above covers it
    const prevKeyed = (await import(/* @vite-ignore */ join(PREV, "src", "keyed.ts"))) as { KeyedWatch: typeof KeyedWatch };
    const welcome: Welcome = { root: "objs", version: 1, client: 7, self: "ada", rights: "read", features: ["keyed_watch"] };
    const read = (W: typeof KeyedWatch) => {
      const w = wire();
      const k = new W(w.t, welcome);
      for (const f of g.frames) w.deliver(f);
      const units = (k.view?.containers ?? []).map((c) => [c.key, k.units(c.key).map((u) => u.key)]);
      return { ready: k.ready, state: k.stateJson(), rev: k.rev, units, watches: w.sent.filter((f) => f.type === "watch").length };
    };
    const old = read(prevKeyed.KeyedWatch);
    const ours = read(KeyedWatch);
    expect(old.ready).toBe(true);
    expect(old.state).toBe(g.final_json);
    expect(old.rev).toBe(g.final_rev);
    expect(old.watches).toBe(1);
    expect(old.units).toEqual(ours.units);
    expect(ours.state).toBe(g.final_json);
  });

  it("the previous client's keyed watch reads this kit's snapshot whose view schema declares nesting (after v0.18.0): the same state and units as this client's", async () => {
    const name = "keyed_snapshot_nesting.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const g = load<{ frames: ServerFrame[]; final_json: string }>(OURS, name);
    if (!existsSync(join(PREV, "src", "keyed.ts"))) return; // a previous release with no keyed watch: the row above covers it
    const prevKeyed = (await import(/* @vite-ignore */ join(PREV, "src", "keyed.ts"))) as { KeyedWatch: typeof KeyedWatch };
    const welcome: Welcome = { root: "items", version: 1, client: 7, self: "ada", rights: "read", features: ["keyed_watch"] };
    const read = (W: typeof KeyedWatch) => {
      const w = wire();
      const k = new W(w.t, welcome);
      for (const f of g.frames) w.deliver(f);
      const units = (k.view?.containers ?? []).map((c) => [c.key, k.units(c.key).map((u) => u.key)]);
      return { ready: k.ready, state: k.stateJson(), units, nesting: k.view?.schema.nesting, watches: w.sent.filter((f) => f.type === "watch").length };
    };
    const old = read(prevKeyed.KeyedWatch);
    const ours = read(KeyedWatch);
    // the frames carry it, and the previous client reads past it
    expect(ours.nesting?.length).toBe(2);
    expect(old.ready).toBe(true);
    expect(old.state).toBe(g.final_json);
    expect(old.watches).toBe(1);
    expect(old.units).toEqual(ours.units);
    expect(ours.state).toBe(g.final_json);
  });

  it("the previous client's keyed watch reads this kit's frames with containers in containers (after v0.18.0): this client's state with the new container fields removed", async () => {
    const name = "keyed_frames_container_parent.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const g = load<Record<"field" | "derived", { frames: ServerFrame[]; final_json: string; final_rev: number; parents: Record<string, string | null>[] }>>(OURS, name);
    if (!existsSync(join(PREV, "src", "keyed.ts"))) return; // a previous release with no keyed watch: the row above covers it
    const prevKeyed = (await import(/* @vite-ignore */ join(PREV, "src", "keyed.ts"))) as { KeyedWatch: typeof KeyedWatch };
    for (const mode of ["field", "derived"] as const) {
      const run = g[mode];
      // a nested container in the snapshot, and parent_after on the frames
      expect(JSON.stringify(run.frames[0]), mode).toContain('"parent":"@coll:1"');
      expect(JSON.stringify(run.frames), mode).toContain('"parent_after"');
      const old = replayKeyed(prevKeyed.KeyedWatch, run.frames);
      const ours = replayKeyed(KeyedWatch, run.frames);
      // this client: the kit's tree, every frame taken
      expect(Object.fromEntries(ours.containers.map((c) => [c.key, c.parent ?? null])), mode).toEqual(run.parents[run.parents.length - 1]);
      expect(ours.sent.filter((t) => t === "watch"), mode).toEqual(["watch"]);
      // the previous client: every frame taken, no error and no new snapshot asked for, the kit's
      // state; and this client's view but for the new container fields
      expect(old.ready, mode).toBe(true);
      expect(old.state, mode).toBe(run.final_json);
      expect(old.rev, mode).toBe(run.final_rev);
      expect(old.current, mode).toBe(true);
      expect(old.sent.filter((t) => t === "watch"), mode).toEqual(["watch"]);
      expect(old.sent.every((t) => t === "watch" || t === "report"), mode).toBe(true);
      expect(withoutNewFields(old), mode).toEqual(withoutNewFields(ours));
    }
  });

  it("the previous client's keyed watch reads this kit's frames with containers' derived values (after v0.18.0): this client's state with the new container fields removed", async () => {
    const name = "keyed_frames_container_derived.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const g = load<{ frames: ServerFrame[]; final_json: string; final_rev: number; derived: Record<string, Record<string, string>>[] }>(OURS, name);
    if (!existsSync(join(PREV, "src", "keyed.ts"))) return;
    const prevKeyed = (await import(/* @vite-ignore */ join(PREV, "src", "keyed.ts"))) as { KeyedWatch: typeof KeyedWatch };
    expect(JSON.stringify(g.frames)).toContain('"containers":{"@coll:1"');
    const old = replayKeyed(prevKeyed.KeyedWatch, g.frames);
    const ours = replayKeyed(KeyedWatch, g.frames);
    // this client: the kit's values, every frame taken
    expect(Object.fromEntries(ours.containers.map((c) => [c.key, c.derived ?? {}]))).toEqual(g.derived[g.derived.length - 1]);
    expect(ours.sent.filter((t) => t === "watch")).toEqual(["watch"]);
    // the previous client: every frame taken, no error and no new snapshot asked for, the kit's
    // state; and this client's view but for the new container fields
    expect(old.ready).toBe(true);
    expect(old.state).toBe(g.final_json);
    expect(old.rev).toBe(g.final_rev);
    expect(old.current).toBe(true);
    expect(old.sent.filter((t) => t === "watch")).toEqual(["watch"]);
    expect(old.sent.every((t) => t === "watch" || t === "report")).toBe(true);
    expect(withoutNewFields(old)).toEqual(withoutNewFields(ours));
  });

  it("the previous client's keyed watch reads this kit's frames with placements in 3D: the same state and units as this client's", async () => {
    const name = "keyed_frames_placement.json";
    if (!existsSync(join(OURS, name))) throw new Error(`test/golden/${name} is missing: the kit writes it; this row needs it`);
    const g = load<{ frames: ServerFrame[]; final_json: string; final_rev: number }>(OURS, name);
    if (!existsSync(join(PREV, "src", "keyed.ts"))) return;
    const prevKeyed = (await import(/* @vite-ignore */ join(PREV, "src", "keyed.ts"))) as { KeyedWatch: typeof KeyedWatch };
    const welcome: Welcome = { root: "objs", version: 1, client: 7, self: "ada", rights: "read", features: ["keyed_watch"] };
    const read = (W: typeof KeyedWatch) => {
      const w = wire();
      const k = new W(w.t, welcome);
      for (const f of g.frames) w.deliver(f);
      const units = (k.view?.containers ?? []).map((c) => [c.key, k.units(c.key).map((u) => u.key)]);
      return { ready: k.ready, state: k.stateJson(), rev: k.rev, units, watches: w.sent.filter((f) => f.type === "watch").length };
    };
    const old = read(prevKeyed.KeyedWatch);
    const ours = read(KeyedWatch);
    expect(old.ready).toBe(true);
    expect(old.state).toBe(g.final_json);
    expect(old.rev).toBe(g.final_rev);
    expect(old.watches).toBe(1);
    expect(old.units).toEqual(ours.units);
  });

  it("this client's keyed watch on the previous release's host (no `keyed_watch`) sends nothing", () => {
    const w = wire();
    const k = new KeyedWatch(w.t, { root: "shapes", version: 1, client: 7, self: "ada", rights: "read", features: ["presence", "claims", "focus", "range"], wire: prevWire });
    k.resync();
    for (const f of load<{ frames: ServerFrame[] }>(OURS, "keyed_frames.json").frames) w.deliver(f);
    expect(w.sent).toEqual([]);
    expect(k.stateJson()).toBeNull();
  });

  it("this client on a wire-2 welcome (no `wire`, the previous features) sends no focus or range", () => {
    const w = wire();
    const p = new PresenceChannel(w.t, { root: "source", version: 1, client: 7, self: "ada", rights: "edit", features: ["presence", "claims"] }, now);
    expect(p.wire).toBe(2);
    p.setFocus({ root: "slides", key: "s256" });
    p.setRange({ root: "shapes", key: "s256-2", field: "text", anchor: 0, head: 1 }, "x");
    p.setSelection({ root: "shapes", keys: ["s256-2"] });
    expect(w.sent.map((f) => f.type)).toEqual(["select"]);
  });
});
