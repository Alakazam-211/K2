// The wire's JSON as the Rust kit encodes it (crates/base/tests/wire_golden.rs writes
// test/golden/*.json from a real session): every value must parse as `wire.ts`'s types, and the
// client must do the right thing with it. The change events replay to the kit's final text; the
// roster events fold to the kit's own final roster; the refusals carry the fields the client
// reads. Regenerate with `MPC_BLESS=1 cargo test -p k2-mpc-base --test wire_golden`.

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import type { RemovedTextNote } from "../src/editor/notes";
import { LiveText, type Rejection } from "../src/live";
import type { CaretAt, ChangeEvent, ErrorReport, RosterEntry, RosterEvent, ServerFrame, Transport } from "../src/wire";
import { runrec } from "./runrec";

const load = (name: string) => JSON.parse(readFileSync(new URL(`./golden/${name}`, import.meta.url), "utf8"));

function wire() {
  const ls = new Set<(f: ServerFrame) => void>();
  const sent: unknown[] = [];
  const t: Transport = {
    send: (f) => sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  return { t, sent, deliver: (f: ServerFrame) => ls.forEach((l) => l(f)) };
}

const isInt = (x: unknown) => typeof x === "number" && Number.isSafeInteger(x) && x >= 0;
const keys = (o: object) => Object.keys(o).sort();

/** Exactly the fields `ChangeEvent` names, with their types. */
function checkChange(e: ChangeEvent) {
  expect(keys(e)).toEqual(["at_ms", "client", "hunks", "on_behalf_of", "pid", "root", "version", "via"]);
  expect(isInt(e.version) && isInt(e.client) && isInt(e.at_ms)).toBe(true);
  expect(typeof e.pid).toBe("string");
  expect(typeof e.via).toBe("string");
  expect(typeof e.root).toBe("string");
  expect(e.on_behalf_of === null || typeof e.on_behalf_of === "string").toBe(true);
  for (const h of e.hunks) {
    expect(keys(h)).toEqual(["end", "insert", "start"]);
    expect(isInt(h.start) && isInt(h.end) && typeof h.insert === "string").toBe(true);
  }
}

function checkCaret(c: CaretAt | null | undefined) {
  if (c == null) return;
  expect(keys(c)).toEqual(["anchor", "anchor_lc", "head", "head_lc", "root", "version"]);
  expect(isInt(c.anchor) && isInt(c.head) && isInt(c.version)).toBe(true);
  expect(c.anchor_lc).toMatch(/^[1-9]\d*:[1-9]\d*$/);
  expect(c.head_lc).toMatch(/^[1-9]\d*:[1-9]\d*$/);
}

type Overwritten = Extract<RosterEvent, { kind: "overwritten" }>;
type Cell = Overwritten["cells"][number];

/** Every field of `overwritten`, as `wire.ts` names them. `satisfies` makes a name the type
 * doesn't have a compile error; `Complete` makes a field the list misses one. The key-set
 * checks below then hold the kit's JSON to exactly these names. */
const OVERWRITTEN_KEYS = [
  "kind",
  "to",
  "change",
  "version",
  "restore_from",
  "source",
  "rule",
  "row",
  "line",
  "cells",
  "more",
  "not_shown",
  "withheld",
  "no_history",
  "message",
] as const satisfies readonly (keyof Overwritten)[];
const CELL_KEYS = ["region", "col", "yours", "file", "cut"] as const satisfies readonly (keyof Cell)[];
type Complete<T, K extends readonly unknown[]> = [Exclude<keyof T, K[number]>] extends [never] ? true : never;
const overwrittenComplete: Complete<Overwritten, typeof OVERWRITTEN_KEYS> = true;
// the editor's own copy of the type (`RemovedTextNote`) is held to the same names
const NOTE_KEYS = OVERWRITTEN_KEYS satisfies readonly (keyof RemovedTextNote)[];
const noteComplete: Complete<RemovedTextNote, typeof NOTE_KEYS> = true;
const cellComplete: Complete<Cell, typeof CELL_KEYS> = true;
const SNAPSHOT_KEYS = ["kind", "version", "members", "watching"];

const RIGHTS = ["edit", "read", "none"];
const ROLES = ["editor", "viewer"];
const KINDS = ["person", "agent", "app", "external", "restore", "import"];

function checkEntry(m: RosterEntry) {
  expect(keys(m)).toEqual(["away", "colour", "cursor", "handle", "kind", "name", "rights", "role"]);
  expect(typeof m.handle).toBe("string");
  expect(m.colour).toMatch(/^#[0-9a-f]{6}$/);
  expect(KINDS).toContain(m.kind);
  expect(ROLES).toContain(m.role);
  expect(RIGHTS).toContain(m.rights);
  expect(typeof m.away).toBe("boolean");
  checkCaret(m.cursor);
}

const isIntOrNull = (x: unknown) => x === null || isInt(x);

/** Exactly the fields `overwritten` names, each of its type (WIRE.md, "The `overwritten` event"). */
function checkOverwritten(e: Overwritten) {
  expect(overwrittenComplete && cellComplete && noteComplete).toBe(true);
  expect(keys(e)).toEqual([...OVERWRITTEN_KEYS].sort());
  expect(e.kind).toBe("overwritten");
  expect(typeof e.to).toBe("string");
  expect(isInt(e.change)).toBe(true);
  expect(isIntOrNull(e.version) && isIntOrNull(e.restore_from)).toBe(true);
  // only link from `version`: it is set only when `restore_from` is, and is the same version
  if (e.version !== null) expect(e.version).toBe(e.restore_from);
  expect(e.source).toBe("an outside save");
  expect(["removed_text", "replaced_text", "rollup"]).toContain(e.rule);
  expect(e.row).toBeNull(); // a text doc
  expect(isIntOrNull(e.line)).toBe(true);
  expect(Array.isArray(e.cells)).toBe(true);
  for (const c of e.cells) {
    expect(keys(c)).toEqual([...CELL_KEYS].sort());
    expect(c.region).toMatch(/^L\d+(–L\d+)?$/);
    expect(c.col).toBeNull();
    expect(typeof c.yours).toBe("string");
    expect(typeof c.file).toBe("string");
    expect(typeof c.cut).toBe("boolean");
  }
  expect(isInt(e.more) && isInt(e.not_shown)).toBe(true);
  expect(typeof e.withheld).toBe("boolean");
  expect(typeof e.no_history).toBe("boolean");
  if (e.no_history) expect(e.version).toBeNull();
  expect(typeof e.message).toBe("string");
  expect(e.message).toContain("may have");
  if (e.rule === "rollup") {
    expect(e.cells).toEqual([]);
    expect(e.line).toBeNull();
    expect(e.more).toBeGreaterThan(0);
  } else {
    expect(e.cells.length).toBeGreaterThan(0);
    expect(isInt(e.line)).toBe(true);
    // the message holds the person's text
    if (!e.withheld) for (const c of e.cells) expect(e.message).toContain(JSON.stringify(c.yours).slice(1, -1));
  }
}

/** How many places a note speaks for (the kit's `Overwrite::cells`). */
const places = (o: Overwritten) => (o.rule === "rollup" ? 0 : o.cells.length) + o.more;

function checkRoster(e: RosterEvent) {
  switch (e.kind) {
    case "snapshot":
      // `overwritten` only when this subscriber has waiting notes
      expect(keys(e)).toEqual((e.overwritten === undefined ? SNAPSHOT_KEYS : [...SNAPSHOT_KEYS, "overwritten"]).sort());
      expect(isInt(e.version) && isInt(e.watching)).toBe(true);
      e.members.forEach(checkEntry);
      if (e.overwritten !== undefined) {
        expect(e.overwritten.length).toBeGreaterThan(0);
        e.overwritten.forEach(checkOverwritten);
      }
      return;
    case "joined":
      expect(keys(e.participant)).toEqual(["colour", "kind", "name", "via"]);
      expect(KINDS).toContain(e.participant.kind);
      expect(RIGHTS).toContain(e.rights);
      return;
    case "left":
      expect(["left", "idle", "kicked", "rights_lost"]).toContain(e.reason);
      return;
    case "rights":
      expect(RIGHTS).toContain(e.rights);
      return;
    case "role":
      expect(ROLES).toContain(e.role);
      return;
    case "away":
      expect(typeof e.away).toBe("boolean");
      return;
    case "cursor":
      checkCaret(e.cursor);
      expect(isInt(e.version)).toBe(true);
      if (e.cursor) expect(e.cursor.version).toBe(e.version);
      return;
    case "claim_started":
      expect(e.claim).toMatch(/^claim-\d+$/);
      expect(isInt(e.lines.first) && isInt(e.lines.last) && isInt(e.version)).toBe(true);
      return;
    case "claim_ended":
      expect(["released", "edited", "expired", "left", "reset"]).toContain(e.how);
      return;
    case "overwritten":
      checkOverwritten(e);
      return;
    case "closed":
      expect(keys(e)).toEqual(["kind", "reason"]);
      expect(typeof e.reason).toBe("string");
      expect(e.reason.length).toBeGreaterThan(0);
      return;
    case "note":
    case "watching":
      return;
    default:
      throw new Error(`an unknown roster event kind: ${JSON.stringify(e)}`);
  }
}

describe("golden wire JSON written by the Rust kit", () => {
  for (const name of ["change_events.json", "change_events_app.json"]) {
    it(`${name}: change events parse and replay to the kit's text`, () => {
      const g = load(name);
      const events = g.events as ChangeEvent[];
      expect(events.length).toBeGreaterThanOrEqual(3);
      events.forEach(checkChange);
      const w = wire();
      const l = new LiveText({ text: g.text, version: g.version }, w.t, { root: g.root });
      for (const event of events) w.deliver({ type: "change", event });
      expect(w.sent).toEqual([]); // no gap, no `since`
      expect(l.text).toBe(g.final_text);
      expect(l.version).toBe(g.final_version);
      runrec(`client_golden_${name.replace(".json", "")}`, { events: events.length });
    });
  }

  it("change_events.json: a 53-bit client ID survives; the agent's change names its person; escapes round-trip", () => {
    const g = load("change_events.json");
    const events = g.events as ChangeEvent[];
    expect(events[0].client).toBe(Number.MAX_SAFE_INTEGER); // MAX_CLIENT_ID = 2^53 - 1
    const agent = events.find((e) => e.on_behalf_of !== null)!;
    expect(agent.on_behalf_of).toBe("ada");
    expect(agent.hunks.map((h) => h.insert).join("")).toContain('say "hi"\t\\ café 😀\u0001\n');
    // the undo comes back as a change by the same participant and client
    expect(events[2].pid).toBe(events[0].pid);
    expect(events[2].client).toBe(events[0].client);
  });

  it("change_events_app.json: an App viewer sees handles, never pids", () => {
    const g = load("change_events_app.json");
    for (const e of g.events as ChangeEvent[]) {
      expect(e.pid).toMatch(/^p\d+$/);
      if (e.on_behalf_of !== null) expect(e.on_behalf_of).toMatch(/^p\d+$/);
    }
  });

  for (const name of [
    "roster_events.json",
    "roster_events_app.json",
    "roster_events_overwritten.json",
    "roster_events_overwritten_app.json",
  ]) {
    it(`${name}: roster events parse and fold to the kit's own final roster`, () => {
      const g = load(name);
      const events = g.events as RosterEvent[];
      expect(events[0].kind).toBe("snapshot");
      events.forEach(checkRoster);
      checkRoster(g.final);
      const w = wire();
      const l = new LiveText({ text: "", version: 0 }, w.t, { self: g.self ?? undefined });
      for (const event of events) w.deliver({ type: "roster_event", event });
      const want = (g.final.members as RosterEntry[]).map((m) => ({
        handle: m.handle,
        name: m.name,
        colour: m.colour,
        kind: m.kind,
        role: m.role,
        away: m.away,
        caret: m.cursor ? [m.cursor.root, m.cursor.anchor, m.cursor.head] : null,
        self: m.handle === g.self,
      }));
      const got = l.roster.map((m) => ({
        handle: m.handle,
        name: m.name,
        colour: m.colour,
        kind: m.kind,
        role: m.role,
        away: m.away,
        caret: m.cursor ? [m.cursor.root, m.cursor.anchor, m.cursor.head] : null,
        self: m.self === true,
      }));
      // the same members with the same fields; the kit orders its roster (editors first, then
      // viewers, then away: `Session::roster`), while the client keeps join order and doesn't
      // re-sort on a role change, so the order agrees only where no role change reorders it
      const byHandle = (a: { handle: string }, b: { handle: string }) => (a.handle < b.handle ? -1 : 1);
      expect([...got].sort(byHandle)).toEqual([...want].sort(byHandle));
      if (!name.startsWith("roster_events_overwritten")) expect(got).toEqual(want);
      runrec(`client_golden_${name.replace(".json", "")}`, { events: events.length, members: want.length });
    });
  }

  it("roster_events_app.json: an App viewer's roster names handles", () => {
    const g = load("roster_events_app.json");
    for (const m of g.final.members as RosterEntry[]) expect(m.handle).toMatch(/^p\d+$/);
  });

  it("roster_events.json: a snapshot without waiting notes leaves `overwritten` out", () => {
    for (const name of ["roster_events.json", "roster_events_app.json"]) {
      const g = load(name);
      for (const e of [...(g.events as RosterEvent[]), g.final as RosterEvent])
        if (e.kind === "snapshot") expect("overwritten" in e, name).toBe(false);
    }
  });

  it("roster_events_overwritten.json: the author's own pushes, live, then the same notes waiting in a later snapshot", () => {
    const g = load("roster_events_overwritten.json");
    const events = g.events as RosterEvent[];
    const pushed = events.filter((e): e is Overwritten => e.kind === "overwritten");
    expect(pushed.map((o) => o.rule)).toEqual(["removed_text", "replaced_text"]);
    for (const o of pushed) {
      expect(o.to).toBe(g.self);
      expect(o.no_history).toBe(false); // Ada may read history: a version to open
      expect(isInt(o.version)).toBe(true);
      expect(o.withheld).toBe(false);
      expect(o.not_shown).toBe(0);
      expect(o.more).toBe(0);
    }
    const [removed, replaced] = pushed;
    expect(removed.line).toBe(3);
    expect(removed.cells.map((c) => [c.yours, c.file, c.cut])).toEqual([
      ["first by ada\n", "", false],
      ["second by ada\n", "", false],
    ]);
    expect(replaced.cells.map((c) => [c.yours, c.file])).toEqual([["ada's draft\n", "the editor's draft"]]);
    expect(replaced.message).toContain(`Version ${replaced.version} holds it.`);
    // `closed` is the last event
    expect(events.at(-1)).toEqual({ kind: "closed", reason: "core_off" });
    expect(events.filter((e) => e.kind === "closed")).toHaveLength(1);
    // the later snapshot: the same notes, deduped on (change, row)
    const final = g.final as Extract<RosterEvent, { kind: "snapshot" }>;
    expect(final.overwritten!.map((o) => [o.change, o.row])).toEqual(pushed.map((o) => [o.change, o.row]));
    for (const o of final.overwritten!) expect(o.to).toBe(g.self);
  });

  it("roster_events_overwritten_app.json: an App viewer's own note names its handle and no version", () => {
    const g = load("roster_events_overwritten_app.json");
    expect(g.self).toMatch(/^p\d+$/);
    const events = g.events as RosterEvent[];
    const pushed = events.filter((e): e is Overwritten => e.kind === "overwritten");
    expect(pushed).toHaveLength(1);
    const [o] = pushed;
    expect(o.to).toBe(g.self);
    expect(o.rule).toBe("removed_text");
    expect(o.cells.map((c) => c.yours)).toEqual(["by the widget\n"]);
    expect(o.version).toBeNull(); // no history right: nothing to link
    expect(o.no_history).toBe(true);
    expect(events.at(-1)).toEqual({ kind: "closed", reason: "core_off" });
    const final = g.final as Extract<RosterEvent, { kind: "snapshot" }>;
    expect(final.overwritten!.map((o) => [o.change, o.row])).toEqual([[o.change, o.row]]);
    // no pid anywhere in the App's stream
    expect(JSON.stringify(g)).not.toMatch(/"(ada|bob|widget)"/);
  });

  for (const name of ["roster_events_closed.json", "roster_events_closed_app.json"]) {
    it(`${name}: a later subscription's snapshot carries the waiting notes; \`closed\` ends it`, () => {
      const g = load(name);
      const events = g.events as RosterEvent[];
      events.forEach(checkRoster);
      expect(events[0].kind).toBe("snapshot");
      const snap = events[0] as Extract<RosterEvent, { kind: "snapshot" }>;
      expect(snap.overwritten!.length).toBeGreaterThan(0);
      for (const o of snap.overwritten!) expect(o.to).toBe(g.self);
      // the same notes as the pushes the first subscription got
      const first = load(name.replace("closed", "overwritten"));
      const pushed = (first.events as RosterEvent[]).filter((e): e is Overwritten => e.kind === "overwritten");
      expect(snap.overwritten).toEqual(first.final.overwritten);
      expect(snap.overwritten!.map((o) => o.change)).toEqual(pushed.map((o) => o.change));
      const last = events.at(-1)!;
      expect(last).toEqual({ kind: "closed", reason: "core_off" });
      // the client folds it without changing the roster
      const w = wire();
      const l = new LiveText({ text: "", version: 0 }, w.t, { self: g.self });
      const seen: string[] = [];
      l.onRosterEvent((e) => seen.push(e.kind));
      for (const event of events) w.deliver({ type: "roster_event", event });
      expect(seen).toEqual(events.map((e) => e.kind));
      expect(l.roster.map((m) => m.handle)).toEqual(snap.members.map((m) => m.handle));
      runrec(`client_golden_${name.replace(".json", "")}`, { events: events.length });
    });
  }

  it("roster_events_rollup.json: past 64 waiting notes the last is a roll-up; no note is dropped", () => {
    const g = load("roster_events_rollup.json");
    expect(typeof g.about).toBe("string");
    expect(g.pushed).toBe(g.saves); // every save pushes
    const tail = g.tail as RosterEvent[];
    tail.forEach(checkRoster);
    const t = tail as Overwritten[];
    expect(t.map((o) => o.kind)).toEqual(["overwritten", "overwritten", "overwritten"]);
    expect(t.map((o) => o.rule)).toEqual(["removed_text", "rollup", "rollup"]);
    expect(t.map((o) => o.more)).toEqual([0, 2, 3]);
    checkRoster(g.final);
    const notes = (g.final as Extract<RosterEvent, { kind: "snapshot" }>).overwritten!;
    expect(notes).toHaveLength(64);
    expect(notes.slice(0, -1).every((o) => o.rule === "removed_text" && o.cells.length === 1)).toBe(true);
    const last = notes.at(-1)!;
    expect(last.rule).toBe("rollup");
    expect(last.cells).toEqual([]);
    expect(last.more).toBe(g.saves - 63);
    expect(last).toEqual(t.at(-1));
    expect(notes.reduce((n, o) => n + places(o), 0)).toBe(g.saves);
    expect(new Set(notes.map((o) => o.change)).size).toBe(64); // one note per change
    runrec("client_golden_roster_events_rollup", { saves: g.saves, waiting: notes.length });
  });

  it("errors.json: each report is `ErrorReport` JSON, and the client acts on it", () => {
    const g = load("errors.json") as Record<string, ErrorReport>;
    for (const [name, r] of Object.entries(g)) {
      expect(keys(r), name).toEqual(["details", "exit", "id", "message", "next", "rule"]);
      expect(r.id).toMatch(/^MPC-E\d{3}$/);
      expect(isInt(r.exit)).toBe(true);
      expect(typeof r.message).toBe("string");
      expect(typeof r.next).toBe("string");
      expect(typeof r.rule).toBe("string");
      expect(typeof r.details).toBe("object");
    }
    expect(Object.fromEntries(Object.entries(g).map(([k, r]) => [k, r.id]))).toEqual({
      write_conflict: "MPC-E111",
      bad_hunk: "MPC-E151",
      base_expired: "MPC-E112",
      bad_position: "MPC-E114",
      bad_edit_frame_cap: "MPC-E113",
      declined_too_large: "MPC-E401",
      history_lost: "MPC-E206",
    });
    expect(Array.isArray(g.write_conflict.details?.conflicts)).toBe(true);

    // a refused edit: the change leaves the text, `details.conflicts` passed along
    const w = wire();
    const l = new LiveText({ text: "abc", version: 1 }, w.t);
    const rej: Rejection[] = [];
    l.onRejected((r) => rej.push(r));
    l.applyLocal([{ start: 3, end: 3, insert: "!" }]);
    w.deliver({ type: "refused", op: 1, error: g.write_conflict });
    expect(rej).toHaveLength(1);
    expect(rej[0].error).toEqual(g.write_conflict);
    expect(l.text).toBe("abc");
    // MPC-E112 on an edit: reload
    const reloads: string[] = [];
    l.onReload((e) => reloads.push(e.error!.id));
    l.applyLocal([{ start: 0, end: 0, insert: ">" }]);
    w.deliver({ type: "refused", op: 2, error: g.base_expired });
    expect(reloads).toEqual(["MPC-E112"]);
  });
});
