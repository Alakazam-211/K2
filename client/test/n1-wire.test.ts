// Wire 3's N−1, both ways (WIRE.md, "Wire versions"), run by `tools/n1/n1.sh node-wire`, which
// puts the previous release's client code and goldens in `test/n1-prev/` (`src/`, `golden/`).
// Without them (every other run) the suite is skipped.
//
// - The previous client (wire 2) on this kit's goldens (wire 3): change events with `turn` and
//   `verb` replay to the kit's text, and every roster stream (focus, range, claim events with
//   actor and times) folds without error to the kit's final roster, as the previous client knows
//   it.
// - This client (wire 3) on the previous release's goldens (wire 2): the same, with no `turn`, no
//   focus or range, and claims attributed from the roster.
// - This client on a wire-2 welcome sends no frame a wire-2 host doesn't take.

import { existsSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { LiveText } from "../src/live";
import { PresenceChannel } from "../src/presence";
import type { ChangeEvent, ClientFrame, RosterEntry, RosterEvent, ServerFrame, Transport, Welcome } from "../src/wire";

const HERE = dirname(fileURLToPath(import.meta.url));
const PREV = join(HERE, "n1-prev");
const OURS = join(HERE, "golden");
const has = existsSync(join(PREV, "src", "presence.ts")) && existsSync(join(PREV, "golden"));

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
const rosterFiles = (dir: string) => readdirSync(dir).filter((n) => n.startsWith("roster_events") && n.endsWith(".json") && !/rollup|closed|grid/.test(n));
/** The streams a `PresenceChannel` folds (element presence and claims), where both have them. */
const PRESENCE_FILES = ["roster_events_elem_presence.json", "roster_events_claims_elems.json", "roster_events_focus_range.json"];
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

describe.skipIf(!has)("wire N−1 both ways (tools/n1/n1.sh node-wire)", () => {
  it("the previous client (wire 2) reads this kit's goldens (wire 3)", async () => {
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

  it("this client (wire 3) reads the previous release's goldens (wire 2)", () => {
    const n = readsAll({ LiveText, PresenceChannel }, join(PREV, "golden"));
    expect(n).toBeGreaterThanOrEqual(8);
    // no turn or verb there: none read
    for (const e of load<Change>(join(PREV, "golden"), "change_events.json").events) {
      expect(e.turn).toBeUndefined();
      expect(e.verb).toBeUndefined();
    }
    // claims from a wire-2 stream: attributed from the roster, no start time
    const g = load<Roster>(join(PREV, "golden"), "roster_events_claims_elems.json");
    const w = wire();
    const p = new PresenceChannel(w.t, { root: "source", version: 0, client: 1, self: g.self, rights: "edit" }, now);
    for (const event of g.events) w.deliver({ type: "roster_event", event });
    expect(p.claims().length).toBeGreaterThan(0);
    for (const c of p.claims()) {
      expect(c.actor.name.length).toBeGreaterThan(0);
      expect(c.startedMs).toBeNull();
    }
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
