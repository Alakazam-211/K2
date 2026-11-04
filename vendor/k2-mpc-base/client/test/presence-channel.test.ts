// `PresenceChannel`: selection by key, pointer and viewport over any `Transport`, no DOM. It sends
// only to a host that lists the feature, never carries an `op` or takes a number from the doc,
// shares a transport with `LiveText` without either reading the other's replies, and folds the
// kit's own roster streams (seeded traces written by `crates/base/tests/wire_golden.rs`) to the
// kit's final roster over a network with latency.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { LiveText } from "../src/live";
import { ELEM_SELECTION_MAX, PresenceChannel, type PresencePeer, type PresenceRefusal } from "../src/presence";
import type { ClientFrame, RosterEvent, ServerFrame, Transport, Welcome } from "../src/wire";
import { Net, rng } from "./fake-server";
import { withPresenceSentAnyway } from "./plants";

const GOLDEN = join(dirname(fileURLToPath(import.meta.url)), "golden");

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

const welcome = (over: Partial<Welcome> = {}): Welcome => ({
  root: "source",
  version: 1,
  client: 7,
  self: "me",
  rights: "edit",
  features: ["presence"],
  ...over,
});

/** Delivery at once, so a test sees the batch's frames as soon as it sets a value. */
const now = { schedule: (deliver: () => void) => deliver() };

/** `MPC_CLIENT_PLANT` runs every test here under the planted fault, so a run shows which ones
 * catch it: `presence-sent-anyway` is `withPresenceSentAnyway`. Any other value throws. */
const plant = process.env.MPC_CLIENT_PLANT;
if (plant !== undefined && plant !== "presence-sent-anyway") throw new Error(`MPC_CLIENT_PLANT=${plant}: not a plant this suite has`);
let plantSpy: { mockRestore(): void } | null = null;
beforeEach(() => {
  if (plant) plantSpy = vi.spyOn(PresenceChannel.prototype, "supported", "get").mockReturnValue(true);
});
afterEach(() => {
  plantSpy?.mockRestore();
  plantSpy = null;
});

/** Everything a channel would send, on a connection with this welcome. */
function framesSentTo(w: Welcome): ClientFrame[] {
  const { t, sent } = wire();
  const c = new PresenceChannel(t, w, now);
  c.setSelection({ root: "items", keys: ["i1"] });
  c.setPointer({ x: 1, y: 2 });
  c.setViewport({ x: 0, y: 0, w: 10, h: 20 });
  c.setPointer(null);
  c.setSelection(null);
  return sent;
}

describe("a host without the feature", () => {
  const { features: _, ...noField } = welcome();
  const cases: [string, Welcome][] = [
    ["no `features` field", noField],
    ["an empty list", welcome({ features: [] })],
    ["only names this client doesn't know", welcome({ features: ["claims", "presence2", "PRESENCE"] })],
  ];
  for (const [why, w] of cases) {
    it(`${why}: it never receives a presence frame`, () => {
      const sent = framesSentTo(w);
      expect(sent).toEqual([]);
    });
    it(`${why}: the plant that sends anyway is caught`, () => {
      expect(withPresenceSentAnyway(() => framesSentTo(w)).length).toBeGreaterThan(0);
    });
  }

  it("the values wait: a later connection that lists the feature gets them, and one that doesn't gets none", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, noField, now);
    c.setSelection({ root: "items", keys: ["i1"] });
    c.setPointer({ x: 3, y: 4 });
    if (!plant) expect(sent).toEqual([]);
    sent.length = 0;
    c.reconnected(welcome({ features: ["presence"], client: 8 }));
    expect(sent.map((f) => f.type).sort()).toEqual(["pointer", "select"]);
    sent.length = 0;
    c.reconnected(welcome({ features: [], client: 9 }));
    expect(sent).toEqual([]);
  });

  it("an unknown name beside `presence` is ignored", () => {
    expect(framesSentTo(welcome({ features: ["later-family", "presence"] })).length).toBeGreaterThan(0);
  });
});

describe("its frames", () => {
  it("carry `req`, never `op` or `seq`, and count up in their own numbering", () => {
    const sent = framesSentTo(welcome());
    expect(sent.map((f) => f.type)).toEqual(["select", "pointer", "viewport", "pointer", "select"]);
    expect(sent.map((f) => (f as { req: number }).req)).toEqual([1, 2, 3, 4, 5]);
    for (const f of sent) {
      expect(f).not.toHaveProperty("op");
      expect(f).not.toHaveProperty("seq");
      expect(f).not.toHaveProperty("version");
      expect(f).not.toHaveProperty("base");
    }
    expect(sent[0]).toEqual({ type: "select", req: 1, selection: { root: "items", keys: ["i1"] } });
    expect(sent[1]).toEqual({ type: "pointer", req: 2, point: { x: 1, y: 2 } });
    expect(sent[2]).toEqual({ type: "viewport", req: 3, rect: { x: 0, y: 0, w: 10, h: 20 } });
    expect(sent[3]).toEqual({ type: "pointer", req: 4, point: null });
    expect(sent[4]).toEqual({ type: "select", req: 5, selection: null });
  });

  it("the pointer and the viewport are batched to the latest; the selection goes at once", () => {
    const { t, sent } = wire();
    const due: (() => void)[] = [];
    const c = new PresenceChannel(t, welcome(), { schedule: (d) => void due.push(d) });
    for (let i = 0; i < 100; i++) c.setPointer({ x: i, y: 0 });
    for (let i = 0; i < 100; i++) c.setViewport({ x: i, y: 0, w: 1, h: 1 });
    c.setSelection({ root: "items", keys: ["i1"] });
    expect(sent.map((f) => f.type)).toEqual(["select"]);
    expect(due.length).toBe(1);
    due[0]();
    expect(sent.slice(1)).toEqual([
      { type: "pointer", req: 2, point: { x: 99, y: 0 } },
      { type: "viewport", req: 3, rect: { x: 99, y: 0, w: 1, h: 1 } },
    ]);
  });

  it("an unchanged value isn't sent again; repeats in a selection count once", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    c.setSelection({ root: "items", keys: ["i1", "i2", "i1"] });
    c.setSelection({ root: "items", keys: ["i1", "i2"] });
    c.setPointer({ x: 1, y: 1 });
    c.setPointer({ x: 1, y: 1 });
    expect(sent).toEqual([
      { type: "select", req: 1, selection: { root: "items", keys: ["i1", "i2"] } },
      { type: "pointer", req: 2, point: { x: 1, y: 1 } },
    ]);
    c.setSelection({ root: "items", keys: [] });
    expect(sent.at(-1)).toEqual({ type: "select", req: 3, selection: null });
  });

  it("what the host would refuse is refused here, unsent, and the value stays", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const got: PresenceRefusal[] = [];
    c.onRefused((r) => void got.push(r));
    c.setSelection({ root: "items", keys: ["i1"] });
    sent.length = 0;
    c.setSelection({ root: "items", keys: Array.from({ length: ELEM_SELECTION_MAX + 1 }, (_, i) => `k${i}`) });
    c.setPointer({ x: Number.NaN, y: 0 });
    c.setViewport({ x: 0, y: Number.POSITIVE_INFINITY, w: 1, h: 1 });
    c.setViewport({ x: 0, y: 0, w: -1, h: 1 });
    expect(sent).toEqual([]);
    expect(got.map((r) => [r.frame, r.error.id])).toEqual([
      ["select", "MPC-E310"],
      ["pointer", "MPC-E114"],
      ["viewport", "MPC-E114"],
      ["viewport", "MPC-E114"],
    ]);
    c.reconnected(welcome({ client: 8 }));
    expect(sent).toEqual([{ type: "select", req: 2, selection: { root: "items", keys: ["i1"] } }]);
  });

  it("exactly the most keys goes out", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    c.setSelection({ root: "items", keys: Array.from({ length: ELEM_SELECTION_MAX }, (_, i) => `k${i}`) });
    expect(sent.length).toBe(1);
  });

  it("nothing goes out after close, and no frame is read", () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const seen = vi.fn();
    c.onChange(seen);
    c.close();
    c.setPointer({ x: 1, y: 1 });
    deliver({ type: "roster_event", event: { kind: "snapshot", version: 1, members: [] } });
    expect(sent).toEqual([]);
    expect(seen).not.toHaveBeenCalled();
  });
});

describe("on a transport shared with LiveText", () => {
  function both() {
    const w = wire();
    const live = new LiveText({ text: "abc", version: 1 }, w.t, { self: "me" });
    const channel = new PresenceChannel(w.t, welcome(), now);
    const liveErrors = vi.fn();
    const liveRejected = vi.fn();
    const refused: PresenceRefusal[] = [];
    live.onError(liveErrors);
    live.onRejected(liveRejected);
    channel.onRefused((r) => void refused.push(r));
    return { ...w, live, channel, liveErrors, liveRejected, refused };
  }

  /** Presence requests until the channel's numbering reaches `n`; the last one's number. */
  function reachReq(b: ReturnType<typeof both>, n: number): number {
    let req = 0;
    while (req < n) {
      b.channel.setPointer({ x: req, y: 1 });
      req = (b.sent.filter((f) => f.type === "pointer").at(-1) as { req: number }).req;
    }
    return req;
  }

  it("a presence refusal never reaches LiveText, even with its number on an edit in flight", () => {
    const b = both();
    b.live.applyLocal([{ start: 0, end: 0, insert: "x" }]);
    const edit = b.sent.find((f) => f.type === "edit") as Extract<ClientFrame, { type: "edit" }>;
    expect(edit).toBeDefined();
    const req = reachReq(b, edit.op);
    expect(req).toBe(edit.op); // the same number, on both sides
    const error = { id: "MPC-E102", message: "no" };
    b.deliver({ type: "presence_refused", req, error });
    expect(b.refused).toEqual([{ frame: "pointer", error }]);
    expect(b.liveErrors).not.toHaveBeenCalled();
    expect(b.liveRejected).not.toHaveBeenCalled();
    // the edit is still in flight, and its ack lands
    b.deliver({ type: "ack", op: edit.op, version: 2, hunks: edit.hunks });
    expect(b.live.text).toBe("xabc");
    expect(b.live.version).toBe(2);
    expect(b.liveRejected).not.toHaveBeenCalled();
  });

  it("an edit's refusal never reaches the channel, even with its number on a presence request", () => {
    const b = both();
    b.live.applyLocal([{ start: 0, end: 0, insert: "x" }]);
    const edit = b.sent.find((f) => f.type === "edit") as Extract<ClientFrame, { type: "edit" }>;
    expect(reachReq(b, edit.op)).toBe(edit.op);
    b.deliver({ type: "refused", op: edit.op, error: { id: "MPC-E111", message: "conflict" } });
    expect(b.liveRejected).toHaveBeenCalledTimes(1);
    expect(b.refused).toEqual([]);
  });

  it("both fold the same roster frames, and LiveText ignores the new kinds", () => {
    const b = both();
    const event: RosterEvent = {
      kind: "snapshot",
      version: 1,
      members: [{ handle: "me", name: "Me", colour: "#111111", kind: "person", role: "viewer", away: false, cursor: null, devices: [{ client: 7, cursor: null }] }],
    };
    b.deliver({ type: "roster_event", event });
    b.deliver({ type: "roster_event", event: { kind: "pointer", who: "me", pointer: { x: 1, y: 2 }, client: 7 } });
    expect(b.channel.peers()[0].pointer).toEqual({ x: 1, y: 2 });
    expect(b.live.version).toBe(1);
    expect(b.liveErrors).not.toHaveBeenCalled();
  });
});

describe("its peers", () => {
  it("are one a connection, the person's other connection `sameUser`, and clear on a host's `null`", () => {
    const { t, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const changes = vi.fn();
    c.onChange(changes);
    deliver({
      type: "roster_event",
      event: {
        kind: "snapshot",
        version: 1,
        members: [
          {
            handle: "me",
            name: "Me",
            colour: "#111111",
            kind: "person",
            role: "editor",
            away: false,
            cursor: null,
            devices: [
              { client: 7, cursor: null },
              { client: 8, cursor: null, pointer: { x: 5.5, y: -1.25 } },
            ],
          },
          { handle: "bob", name: "Bob", colour: "#222222", kind: "agent", role: "viewer", away: false, cursor: null, devices: [{ client: 9, cursor: null }] },
        ],
      },
    });
    expect(changes).toHaveBeenCalledTimes(1);
    const brief = (p: PresencePeer) => [p.handle, p.client ?? null, p.isSelf, p.sameUser, p.kind, p.pointer];
    expect(c.peers().map(brief)).toEqual([
      ["me", 7, true, false, "person", null],
      ["me", 8, false, true, "person", { x: 5.5, y: -1.25 }],
      ["bob", null, false, false, "agent", null],
    ]);
    deliver({ type: "roster_event", event: { kind: "pointer", who: "me", pointer: null, client: 8 } });
    deliver({ type: "roster_event", event: { kind: "selection", who: "bob", selection: { root: "items", keys: ["i1"] }, client: 9 } });
    expect(c.peers().map(brief)).toEqual([
      ["me", 7, true, false, "person", null],
      ["bob", 9, false, false, "agent", null],
    ]);
    expect(c.peers()[1].selection).toEqual({ root: "items", keys: ["i1"] });
    deliver({ type: "roster_event", event: { kind: "left", who: "bob", reason: "left" } });
    expect(c.peers().length).toBe(1);
  });

  it("a host with no per-connection rows (a grid) gives one row a participant", () => {
    const { t, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    deliver({ type: "roster", entries: [{ handle: "ann", name: "Ann", colour: "#333333", kind: "person", role: "viewer", away: false, cursor: null }] });
    deliver({ type: "roster_event", event: { kind: "selection", who: "ann", selection: { root: "rows", keys: ["r1"] } } });
    const [p] = c.peers();
    expect(p.client).toBeUndefined();
    expect(p.selection).toEqual({ root: "rows", keys: ["r1"] });
  });
});

describe("the kit's roster streams", () => {
  type Golden = { self: string; client?: number; events: RosterEvent[]; final: RosterEvent };
  const fold = (self: string, client: number, events: RosterEvent[]) => {
    const { t, deliver } = wire();
    const c = new PresenceChannel(t, welcome({ self, client }), now);
    for (const event of events) deliver({ type: "roster_event", event });
    return c;
  };
  const sameRows = (ps: readonly PresencePeer[]) => [...ps].sort((a, b) => `${a.handle}:${a.client}`.localeCompare(`${b.handle}:${b.client}`));

  it("roster_events_elem_presence.json folds to the kit's final roster", () => {
    const g = JSON.parse(readFileSync(join(GOLDEN, "roster_events_elem_presence.json"), "utf8")) as Golden;
    const got = fold(g.self, g.client!, g.events).peers();
    const want = fold(g.self, g.client!, [g.final]).peers();
    expect(sameRows(got)).toEqual(sameRows(want));
    expect(got.some((p) => p.selection !== null)).toBe(true);
  });

  const traces = (JSON.parse(readFileSync(join(GOLDEN, "roster_trace_elem_presence.json"), "utf8")) as { traces: (Golden & { seed: number })[] }).traces;
  it("the seeded traces exist and hold every kind", () => {
    expect(traces.length).toBe(8);
    const kinds = new Set(traces.flatMap((t) => t.events.map((e) => e.kind)));
    for (const k of ["selection", "pointer", "viewport", "left"]) expect(kinds.has(k as RosterEvent["kind"]), k).toBe(true);
  });
  for (const trace of traces) {
    it(`trace ${trace.seed}: every channel on a network with latency folds to the kit's final roster`, () => {
      const net = new Net();
      const r = rng(trace.seed);
      const channels = [0, 1, 2].map(() => {
        const w = wire();
        return { w, c: new PresenceChannel(w.t, welcome({ self: trace.self, client: 1 }), now), at: 0 };
      });
      for (const event of trace.events) {
        for (const ch of channels) {
          // in order per connection, as a WebSocket: each frame after the one before it
          const delay = r.int(40);
          net.at(ch.at + delay, () => ch.w.deliver({ type: "roster_event", event }));
          ch.at += delay;
        }
      }
      while (net.step());
      const want = sameRows(fold(trace.self, 1, [trace.final]).peers());
      for (const { c } of channels) expect(sameRows(c.peers())).toEqual(want);
    });
  }
});
