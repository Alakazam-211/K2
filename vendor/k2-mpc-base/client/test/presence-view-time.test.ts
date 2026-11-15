// A connection's view address (the view it is looking at, by address) and time position (its
// playhead: `at` a decimal string in the pack's unit, `state`, `in`, `rate`; the host stamps
// `at_ms`), in `PresenceChannel` (WIRE.md, Presence frames). Sent only to a host that lists
// `view_address` / `time_position`, checked here before they go, and folded from the kit's own
// roster stream (`test/golden/roster_events_view_time.json`, written by
// `crates/base/tests/wire_golden.rs`) to the kit's final roster.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PresenceChannel, TIME_POSITION_FEATURE, VIEW_ADDRESS_FEATURE, type PresencePeer, type PresenceRefusal } from "../src/presence";
import type { ClientFrame, RosterEvent, ServerFrame, Transport, Welcome } from "../src/wire";

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

const welcome = (features: string[], over: Partial<Welcome> = {}): Welcome => ({ root: "source", version: 1, client: 7, self: "me", rights: "edit", features, ...over });
const now = { schedule: (deliver: () => void) => deliver() };
const ALL = ["presence", VIEW_ADDRESS_FEATURE, TIME_POSITION_FEATURE];

type Golden = { self: string; client: number; events: RosterEvent[]; final: RosterEvent };

function fold(self: string, client: number, events: RosterEvent[]): PresenceChannel {
  const w = wire();
  const c = new PresenceChannel(w.t, welcome(["presence"], { self, client }), now);
  for (const event of events) w.deliver({ type: "roster_event", event });
  return c;
}

const rows = (ps: readonly PresencePeer[]) => ps.map((p) => [p.handle, p.client ?? null, p.viewAddress, JSON.stringify(p.timePosition)]).sort();

describe("view address and time position", () => {
  it("go out only to a host that lists their names, the time without at_ms", () => {
    for (const [features, want] of [
      [["presence"], []],
      [["presence", VIEW_ADDRESS_FEATURE], ["view_address"]],
      [ALL, ["view_address", "time_position"]],
    ] as [string[], string[]][]) {
      const w = wire();
      const c = new PresenceChannel(w.t, welcome(features), now);
      c.setViewAddress("@seq:main");
      c.setTimePosition({ at: "254016000000", state: "playing", in: "@seq:main", rate: "1/2" });
      expect(w.sent.map((f) => f.type)).toEqual(want);
      const t = w.sent.find((f) => f.type === "time_position");
      if (t) expect(t).toEqual({ type: "time_position", req: 2, time_position: { at: "254016000000", state: "playing", in: "@seq:main", rate: "1/2" } });
    }
  });

  it("an unchanged value isn't sent again; clearing sends null; a reconnect sends them again", () => {
    const w = wire();
    const c = new PresenceChannel(w.t, welcome(ALL), now);
    c.setViewAddress("@seq:main");
    c.setViewAddress("@seq:main");
    c.setTimePosition({ at: "0", state: "paused" });
    c.setTimePosition({ at: "0", state: "paused" });
    expect(w.sent.length).toBe(2);
    c.reconnected(welcome(ALL));
    expect(w.sent.slice(2).map((f) => f.type)).toEqual(["view_address", "time_position"]);
    c.setViewAddress(null);
    expect(w.sent.at(-1)).toMatchObject({ type: "view_address", address: null });
  });

  it("what the host would refuse is refused here, unsent", () => {
    const w = wire();
    const c = new PresenceChannel(w.t, welcome(ALL), now);
    const refused: PresenceRefusal[] = [];
    c.onRefused((r) => refused.push(r));
    for (const a of ["", "@seq", "@Seq:1", "@k:BAD", `@seq:${"x".repeat(600)}`]) c.setViewAddress(a);
    for (const t of [
      { at: "01", state: "paused" },
      { at: "1.5", state: "paused" },
      { at: "-0", state: "paused" },
      { at: "99999999999999999999", state: "paused" },
      { at: "1", state: "stopped" },
      { at: "1", state: "paused", rate: "1/0" },
      { at: "1", state: "paused", rate: "x" },
      { at: "1", state: "paused", in: "@seq" },
    ] as const) c.setTimePosition(t as never);
    expect(w.sent).toEqual([]);
    expect(refused.length).toBe(13);
    expect(new Set(refused.map((r) => r.error.id))).toEqual(new Set(["MPC-E114"]));
    c.setTimePosition({ at: "-12", state: "paused", rate: "1001/1000" });
    c.setViewAddress("i3");
    expect(w.sent.map((f) => f.type)).toEqual(["time_position", "view_address"]);
  });

  it("a refused value isn't sent again on a reconnect", () => {
    const w = wire();
    const c = new PresenceChannel(w.t, welcome(ALL), now);
    c.setViewAddress("@seq:main");
    const req = (w.sent[0] as { req: number }).req;
    w.deliver({ type: "presence_refused", req, error: { id: "MPC-E305", message: "watching" } });
    c.reconnected(welcome(ALL));
    expect(w.sent.length).toBe(1);
  });

  it("roster_events_view_time.json folds to the kit's final roster, by connection", () => {
    const g = JSON.parse(readFileSync(join(GOLDEN, "roster_events_view_time.json"), "utf8")) as Golden;
    const got = fold(g.self, g.client, g.events).peers();
    const want = fold(g.self, g.client, [g.final]).peers();
    expect(rows(got)).toEqual(rows(want));
    const by = (h: string) => got.find((p) => p.handle === h && (p.viewAddress !== null || p.timePosition !== null));
    expect(by("ada")?.viewAddress).toBe("@seq:main");
    expect(by("ada")?.timePosition).toBeNull();
    expect(by("atlas")?.timePosition).toEqual({ at: "-1270080000", state: "paused", at_ms: 1700000000600, rate: "1001/1000" });
    expect(by("bob")?.timePosition?.at).toBe("0");
  });
});
