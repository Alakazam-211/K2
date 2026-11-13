// Focus and range (wire 3) in `PresenceChannel`: the
// frames go only to a host that lists their feature, a range's UTF-16 offsets go out as UTF-8
// bytes, refusals forget the value, a reconnect sends the focus before the range, and the roster's
// `focus` and `range` events fold into `peers()`. A wire-2 host (no `welcome.wire`, no features)
// gets nothing new.

import { describe, expect, it } from "vitest";
import { FOCUS_FEATURE, PresenceChannel, RANGE_FEATURE, rangeToUtf16, type PresenceRefusal } from "../src/presence";
import { ERR, WIRE_VERSION, type ClientFrame, type RosterEvent, type ServerFrame, type Transport, type Welcome } from "../src/wire";

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
  features: ["presence", FOCUS_FEATURE, RANGE_FEATURE],
  wire: WIRE_VERSION,
  ...over,
});

const now = { schedule: (deliver: () => void) => deliver() };

describe("focus and range frames (wire 3)", () => {
  it("the wire version is 3, and a welcome without one is a wire-2 host", () => {
    expect(WIRE_VERSION).toBe(3);
    const { t } = wire();
    expect(new PresenceChannel(t, welcome(), now).wire).toBe(3);
    const { wire: _, ...old } = welcome();
    expect(new PresenceChannel(t, old, now).wire).toBe(2);
  });

  it("a wire-2 host (no `focus` or `range` feature) is sent neither; a later host that lists them gets both, focus first", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome({ features: ["presence"], wire: undefined }), now);
    c.setFocus({ root: "slides", key: "s259" });
    c.setRange({ root: "shapes", key: "s259-3", field: "text", anchor: 0, head: 4 }, "Hire two engineers");
    expect(sent).toEqual([]);
    expect(c.focusSupported || c.rangeSupported).toBe(false);
    c.reconnected(welcome());
    expect(sent.map((f) => f.type)).toEqual(["focus", "range"]);
  });

  it("setFocus sends `focus` with its own request number, never an op; null clears; the same value twice is one frame", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    c.setFocus({ root: "slides", key: "s256" });
    c.setFocus({ root: "slides", key: "s256" });
    c.setFocus(null);
    expect(sent).toEqual([
      { type: "focus", req: 1, focus: { root: "slides", key: "s256" } },
      { type: "focus", req: 2, focus: null },
    ]);
    expect(sent.every((f) => !("op" in f))).toBe(true);
  });

  it("setRange takes UTF-16 indexes into the field's text and sends UTF-8 bytes (contract case A: `Café plan` 0..4 is 0..5)", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    c.setRange({ root: "shapes", key: "s259-3", field: "text", anchor: 0, head: 4 }, "Café plan");
    expect(sent).toEqual([{ type: "range", req: 1, range: { root: "shapes", key: "s259-3", field: "text", anchor: 0, head: 5 } }]);
    expect(rangeToUtf16({ anchor: 0, head: 5 }, "Café plan")).toEqual({ anchor: 0, head: 4 });
    // ASCII: the same numbers either way (contract case A's `Hire two engineers`, 0..18)
    expect(rangeToUtf16({ anchor: 0, head: 18 }, "Hire two engineers\nShip the v2 beta")).toEqual({ anchor: 0, head: 18 });
    // inside a character, or past the end: not this text's
    expect(rangeToUtf16({ anchor: 0, head: 4 }, "Café plan")).toBeNull();
    expect(rangeToUtf16({ anchor: 0, head: 99 }, "Café plan")).toBeNull();
  });

  it("a range outside the field's text is refused here, unsent (MPC-E114)", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const refused: PresenceRefusal[] = [];
    c.onRefused((r) => refused.push(r));
    c.setRange({ root: "shapes", key: "s1-2", field: "text", anchor: 0, head: 10 }, "short");
    c.setRange({ root: "shapes", key: "s1-2", field: "text", anchor: -1, head: 0 }, "short");
    expect(sent).toEqual([]);
    expect(refused.map((r) => [r.frame, r.error.id])).toEqual([
      ["range", ERR.BAD_POSITION],
      ["range", ERR.BAD_POSITION],
    ]);
  });

  it("a refused focus or range (a watcher's MPC-E305) is forgotten: not sent again after a reconnect", () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const refused: PresenceRefusal[] = [];
    c.onRefused((r) => refused.push(r));
    c.setFocus({ root: "slides", key: "s256" });
    c.setRange({ root: "shapes", key: "s256-2", field: "text", anchor: 0, head: 0 }, "x");
    const watching = { id: "MPC-E305", message: "watching" };
    deliver({ type: "presence_refused", req: 1, error: watching });
    deliver({ type: "presence_refused", req: 2, error: watching });
    expect(refused.map((r) => r.frame)).toEqual(["focus", "range"]);
    sent.length = 0;
    c.reconnected(welcome());
    expect(sent).toEqual([]);
  });
});

describe("focus and range in the roster", () => {
  const snapshot = (): RosterEvent => ({
    kind: "snapshot",
    version: 40,
    watching: 0,
    members: [
      { handle: "ada", name: "Ada", colour: "#2f9e8f", kind: "person", role: "editor", away: false, cursor: null, focus: { root: "slides", key: "s259" } },
      { handle: "atlas", name: "Atlas", colour: "#6d5bd0", kind: "agent", role: "editor", away: false, cursor: null },
    ],
  });

  it("a snapshot's focus and the events after it fold into peers(); a dropped range is null, the focus stays", () => {
    const { t, deliver } = wire();
    const c = new PresenceChannel(t, welcome({ self: "kai" }), now);
    deliver({ type: "roster_event", event: snapshot() });
    expect(c.peers().find((p) => p.handle === "ada")!.focus).toEqual({ root: "slides", key: "s259" });
    const range = { root: "shapes", key: "s259-3", field: "text", anchor: 0, head: 18 };
    deliver({ type: "roster_event", event: { kind: "range", who: "ada", range } });
    deliver({ type: "roster_event", event: { kind: "focus", who: "atlas", focus: { root: "slides", key: "s256" } } });
    const ada = () => c.peers().find((p) => p.handle === "ada")!;
    expect(ada().range).toEqual(range);
    expect(c.peers().find((p) => p.handle === "atlas")!.focus).toEqual({ root: "slides", key: "s256" });
    deliver({ type: "roster_event", event: { kind: "range", who: "ada", range: null } });
    expect(ada().range).toBeNull();
    expect(ada().focus).toEqual({ root: "slides", key: "s259" });
  });

  it("per connection: a device's focus and range are its own", () => {
    const { t, deliver } = wire();
    const c = new PresenceChannel(t, welcome({ self: "kai" }), now);
    deliver({
      type: "roster_event",
      event: {
        kind: "snapshot",
        version: 1,
        watching: 0,
        members: [
          {
            handle: "ada",
            name: "Ada",
            colour: "#2f9e8f",
            kind: "person",
            role: "editor",
            away: false,
            cursor: null,
            devices: [
              { client: 1, cursor: null, focus: { root: "slides", key: "s1" } },
              { client: 2, cursor: null, focus: { root: "slides", key: "s2" } },
            ],
          },
        ],
      },
    });
    deliver({ type: "roster_event", event: { kind: "focus", who: "ada", focus: { root: "slides", key: "s3" }, client: 2 } });
    const rows = c.peers().filter((p) => p.handle === "ada");
    expect(rows.map((p) => [p.client, p.focus?.key])).toEqual([
      [1, "s1"],
      [2, "s3"],
    ]);
  });
});
