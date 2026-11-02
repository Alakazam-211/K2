// Per-connection carets (kit v0.10.1): the same person on two devices or in two tabs sees
// their other connection's caret, as Google Docs shows one login's other tabs. The host sends
// `devices` on roster entries and `client` on `cursor` events; `carets()` returns one caret a
// connection, never this connection's own (`welcome.client`), another connection of the same
// participant with `sameUser: true`. From an older host (no `devices`, no `client`) it is the
// 0.11 behaviour exactly: one caret a participant, its own skipped. The library marks the
// carets; it draws nothing.

import { afterEach, describe, expect, it, vi } from "vitest";
import { LiveText, type RemoteCaret } from "../src/live";
import type { CaretAt, ChangeEvent, Device, RosterEntry, RosterEvent, ServerFrame, Transport } from "../src/wire";
import { FakeServer, Net, rng } from "./fake-server";

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

const caret = (anchor: number, head = anchor, version = 1): CaretAt => ({ root: "source", anchor, head, version });
const change = (version: number, start: number, insert: string, client = 99): ChangeEvent => ({
  version,
  pid: "bob",
  client,
  via: "tab",
  root: "source",
  hunks: [{ start, end: start, insert }],
  at_ms: 0,
  on_behalf_of: null,
});
const entry = (handle: string, name: string, cursor: CaretAt | null, devices?: Device[]): RosterEntry => ({
  handle,
  name,
  colour: "#7c3aed",
  kind: "person",
  role: "editor",
  away: false,
  cursor,
  rights: "edit",
  ...(devices ? { devices } : {}),
});
const snapshot = (members: RosterEntry[], version = 1): RosterEvent => ({ kind: "snapshot", version, members, watching: 0 });
const roster = (ev: RosterEvent): ServerFrame => ({ type: "roster_event", event: ev });
/** What a caret test compares: who, where, which connection, how it's marked. */
const brief = (cs: RemoteCaret[]) => cs.map((c) => [c.handle, c.client ?? null, c.anchor, c.head, c.sameUser, c.isSelf, c.label ?? null]);

const TEXT = "0123456789abcdefghij\n";

/** A clock that never fires: the caret re-send stays out of these tests. */
const still = { set: () => 0, clear: () => {}, now: () => 0 };

/** Ada on three tabs (clients 11, 12 and 13; 12 labelled, and her most active) and Bob (21), as
 * a current host sends them; each of Ada's tabs is a `LiveText` with its own client ID. */
function adaOnThreeTabs() {
  const tabs = [11, 12, 13].map((client) => {
    const w = wire();
    const l = new LiveText({ text: TEXT, version: 1 }, w.t, { client, self: "ada", timers: still });
    return { w, l, client };
  });
  const all = (f: ServerFrame) => tabs.forEach((t) => t.w.deliver(f));
  all(
    roster(
      snapshot([
        entry("ada", "Ada", caret(8), [
          { client: 11, cursor: caret(3) },
          { client: 12, cursor: caret(8), label: "another tab" },
          { client: 13, cursor: caret(5) },
        ]),
        entry("bob", "Bob", caret(15), [{ client: 21, cursor: caret(15) }]),
      ]),
    ),
  );
  return { tabs, all };
}

/** The behaviour the fix is for: each tab of one person shows every other tab's caret
 * (the most active first), marked `sameUser`, and never its own; mapped through remote edits.
 * Throws when it doesn't hold. */
function checkSameUser() {
  const { tabs, all } = adaOnThreeTabs();
  const [t1, t2] = tabs;
  expect(brief(t1.l.carets())).toEqual([
    ["ada", 12, 8, 8, true, false, "another tab"],
    ["ada", 13, 5, 5, true, false, null],
    ["bob", 21, 15, 15, false, false, null],
  ]);
  expect(brief(t2.l.carets())).toEqual([
    ["ada", 11, 3, 3, true, false, null],
    ["ada", 13, 5, 5, true, false, null],
    ["bob", 21, 15, 15, false, false, null],
  ]);
  // Bob types 4 bytes at 0, then tab 2 moves (its own event, `client: 12`), and a change
  // before the caret is mapped through: every caret is in the replica's current text
  all({ type: "change", event: change(2, 0, "BOB ") });
  all(roster({ kind: "cursor", who: "ada", cursor: caret(14, 16, 2), version: 2, client: 12 }));
  all({ type: "change", event: change(3, 0, ">") });
  expect(brief(t1.l.carets())).toEqual([
    ["ada", 12, 15, 17, true, false, "another tab"],
    ["ada", 13, 10, 10, true, false, null],
    ["bob", 21, 20, 20, false, false, null],
  ]);
  expect(brief(t2.l.carets())).toEqual([
    ["ada", 11, 8, 8, true, false, null],
    ["ada", 13, 10, 10, true, false, null],
    ["bob", 21, 20, 20, false, false, null],
  ]);
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe("per-connection carets (kit v0.10.1)", () => {
  it("connections of one person each see the others' carets, mapped across remote edits", () => {
    checkSameUser();
  });

  it("this connection's own caret is never returned, from a snapshot or from events", () => {
    const { tabs, all } = adaOnThreeTabs();
    const [t1] = tabs;
    const own = () => t1.l.carets().filter((c) => c.client === 11 || c.isSelf);
    expect(own()).toEqual([]);
    // its own caret moves, goes and comes back, by events: never returned
    all(roster({ kind: "cursor", who: "ada", cursor: caret(5, 5, 1), version: 1, client: 11 }));
    expect(own()).toEqual([]);
    all(roster({ kind: "cursor", who: "ada", cursor: null, version: 1, client: 11 }));
    all(roster({ kind: "cursor", who: "ada", cursor: caret(6, 6, 1), version: 1, client: 11 }));
    expect(own()).toEqual([]);
    // the other tabs leave with their carets (`null` each, then the person's caret without
    // `client`, which is tab 1's own): nothing of Ada's is left to show in tab 1
    all(roster({ kind: "cursor", who: "ada", cursor: null, version: 1, client: 12 }));
    all(roster({ kind: "cursor", who: "ada", cursor: null, version: 1, client: 13 }));
    all(roster({ kind: "cursor", who: "ada", cursor: caret(6, 6, 1), version: 1 }));
    expect(t1.l.carets().filter((c) => c.handle === "ada")).toEqual([]);
    // and a new tab of Ada's shows up by its first caret event
    all(roster({ kind: "cursor", who: "ada", cursor: caret(2, 2, 1), version: 1, client: 14 }));
    expect(brief(t1.l.carets()).filter((c) => c[0] === "ada")).toEqual([["ada", 14, 2, 2, true, false, null]]);
  });

  it("without its own client ID a connection hides every connection of its own participant", () => {
    const w = wire();
    const l = new LiveText({ text: TEXT, version: 1 }, w.t, { self: "ada", timers: still });
    w.deliver(
      roster(
        snapshot([
          entry("ada", "Ada", caret(8), [
            { client: 11, cursor: caret(3) },
            { client: 12, cursor: caret(8) },
          ]),
          entry("bob", "Bob", caret(15), [{ client: 21, cursor: caret(15) }]),
        ]),
      ),
    );
    expect(brief(l.carets())).toEqual([["bob", 21, 15, 15, false, false, null]]);
  });

  it("other people: one caret a connection, unmarked, the most active first", () => {
    const w = wire();
    const l = new LiveText({ text: TEXT, version: 1 }, w.t, { client: 11, self: "ada", timers: still });
    w.deliver(
      roster(
        snapshot([
          entry("ada", "Ada", null, [{ client: 11, cursor: null }]),
          // Bob on two devices: his row caret (the most active) is his second's
          entry("bob", "Bob", caret(9), [
            { client: 21, cursor: caret(4) },
            { client: 22, cursor: caret(9), label: "Phone" },
          ]),
          entry("cy", "Cy", caret(1), [{ client: 31, cursor: caret(1) }]),
        ]),
      ),
    );
    expect(brief(l.carets())).toEqual([
      ["bob", 22, 9, 9, false, false, "Phone"],
      ["bob", 21, 4, 4, false, false, null],
      ["cy", 31, 1, 1, false, false, null],
    ]);
    // `find` by handle (a host following a person) gets the caret an older host showed
    expect(l.carets().find((c) => c.handle === "bob")?.anchor).toBe(9);
    // a person with one connection: one caret, as before, now with its client
    w.deliver(roster({ kind: "cursor", who: "cy", cursor: caret(2, 2, 1), version: 1, client: 31 }));
    expect(brief(l.carets()).filter((c) => c[0] === "cy")).toEqual([["cy", 31, 2, 2, false, false, null]]);
    // `roster` keeps the person's caret as the last event gave it (what 0.11 kept)
    expect(l.roster.find((r) => r.handle === "cy")?.cursor?.anchor).toBe(2);
  });

  it("an older host (no `devices`, no `client`): exactly the 0.11 carets", () => {
    // 0.11's `carets()`, verbatim but for the two fields every caret now has at their defaults
    const before = (l: LiveText): RemoteCaret[] => {
      const self = (l as unknown as { selfHandle: string | undefined }).selfHandle;
      const map = (l as unknown as { mapFromVersion(p: number, v: number): number | null }).mapFromVersion.bind(l);
      const out: RemoteCaret[] = [];
      for (const r of l.roster) {
        const c = r.cursor;
        if (r.self || r.handle === self || !c || c.root !== l.root) continue;
        const v = c.version ?? l.version;
        const anchor = map(c.anchor, v);
        const head = map(c.head, v);
        if (anchor === null || head === null) continue;
        out.push({ handle: r.handle, name: r.name, colour: r.colour, anchor, head, isSelf: false, sameUser: false });
      }
      return out;
    };
    const r = rng(11);
    for (let run = 0; run < 40; run++) {
      const w = wire();
      const l = new LiveText({ text: TEXT, version: 1 }, w.t, { client: 11, self: "ada", timers: still });
      const who = ["ada", "bob", "cy", "dee"];
      w.deliver(roster(snapshot(who.map((h, i) => entry(h, h.toUpperCase(), i % 2 ? caret(i) : null)))));
      let version = 1;
      let len = TEXT.length;
      for (let step = 0; step < 30; step++) {
        const k = r.int(10);
        const h = who[r.int(who.length)];
        if (k < 3) {
          version++;
          const at = r.int(len + 1);
          w.deliver({ type: "change", event: change(version, at, "xy") });
          len += 2;
        } else if (k < 8) {
          const a = r.int(len + 1);
          w.deliver(roster({ kind: "cursor", who: h, cursor: r.int(5) ? caret(a, a, version) : null, version }));
        } else if (k === 8) {
          w.deliver(roster({ kind: "left", who: h }));
        } else {
          w.deliver(roster({ kind: "joined", who: h, participant: { name: h, colour: "#000", kind: "person" } }));
        }
        expect(l.carets(), `run ${run} step ${step}`).toEqual(before(l));
        for (const c of l.carets()) expect("client" in c || "label" in c).toBe(false);
      }
    }
  });

  it("planted faults fail: skipping by handle again, or one caret a person", () => {
    const real = LiveText.prototype.carets;
    // the 0.11 rule: every connection of this participant skipped (the bug)
    vi.spyOn(LiveText.prototype, "carets").mockImplementation(function (this: LiveText) {
      return real.call(this).filter((c) => !c.sameUser);
    });
    expect(checkSameUser).toThrow();
    vi.restoreAllMocks();
    // the kit's old fold: one caret a person, the first connection's
    vi.spyOn(LiveText.prototype, "carets").mockImplementation(function (this: LiveText) {
      const seen = new Set<string>();
      return real.call(this).filter((c) => !seen.has(c.handle) && !!seen.add(c.handle));
    });
    expect(checkSameUser).toThrow();
    vi.restoreAllMocks();
    // and the check passes on the real thing
    expect(checkSameUser).not.toThrow();
  });
});

describe("per-connection carets through the fake host's caret route", () => {
  /** Ada on two connections, Bob on one, on the fake host; Bob types while Ada's tabs idle. */
  async function run(olderHost: boolean) {
    const net = new Net();
    const server = new FakeServer(net, TEXT);
    server.caretRoute = { names: { ada: "Ada", bob: "Bob" }, olderHost, labels: { 1002: "another tab" } };
    const open = (conn: number, handle: string) =>
      new LiveText(server.snapshot(), server.connect(conn, () => 3, undefined, { handle, client: conn }), {
        client: conn,
        self: handle,
        timers: still,
      });
    const a1 = open(1001, "ada");
    const a2 = open(1002, "ada");
    const bob = open(2001, "bob");
    const step = () => {
      while (net.step());
    };
    server.caretTick(); // a current host's snapshot (or nothing, on an older host's route)
    step();
    a1.setCursor(3, 3);
    a2.setCursor(8, 10);
    step();
    server.caretTick();
    step();
    // Bob types at the start: Ada's carets come mapped, without either tab re-sending
    bob.applyLocal([{ start: 0, end: 0, insert: "BOB " }]);
    step();
    return { server, a1, a2, bob, step };
  }

  it("a current host: each of Ada's tabs shows the other's caret, mapped through Bob's typing", async () => {
    const { a1, a2, bob } = await run(false);
    expect(brief(a1.carets())).toEqual([["ada", 1002, 12, 14, true, false, "another tab"]]);
    expect(brief(a2.carets())).toEqual([["ada", 1001, 7, 7, true, false, null]]);
    // Bob sees both of Ada's carets, unmarked: the most active (the route's row caret, her
    // first connection's) first
    expect(brief(bob.carets())).toEqual([
      ["ada", 1001, 7, 7, false, false, null],
      ["ada", 1002, 12, 14, false, false, "another tab"],
    ]);
  });

  it("a current host: a tab that leaves takes its caret with it", async () => {
    const { server, a1, bob, step } = await run(false);
    server.leave(1002);
    server.caretTick();
    step();
    expect(a1.carets()).toEqual([]);
    expect(brief(bob.carets())).toEqual([["ada", 1001, 7, 7, false, false, null]]);
  });

  it("an older host: one caret a person, Ada's own never shown in her tabs (as 0.11)", async () => {
    const { a1, a2, bob } = await run(true);
    expect(a1.carets()).toEqual([]);
    expect(a2.carets()).toEqual([]);
    // the route's fold: the first of Ada's connections with a caret
    expect(bob.carets()).toEqual([{ handle: "ada", name: "Ada", colour: "#7c3aed", anchor: 7, head: 7, isSelf: false, sameUser: false }]);
  });
});
