// A held undo or redo (one made while edits are queued: it waits in the queue after them) always
// goes out: on every path that takes the edits before it out of the queue (sent, acked, refused,
// rejected, composed away, dropped by a rebase, reset, a reconnect), it is sent next, in order.
// Only `close()` drops it, and nothing is sent after that.

import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { LiveText } from "../src/live";
import type { ChangeEvent, ClientFrame, ServerFrame, Transport } from "../src/wire";

/** A transport the test drives by hand. */
function wire() {
  const sent: ClientFrame[] = [];
  const ls = new Set<(f: ServerFrame) => void>();
  const t: Transport = {
    send: (f) => sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  const deliver = (f: ServerFrame) => ls.forEach((l) => l(f));
  return { t, sent, deliver };
}

const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
const ev = (version: number, hunks: Hunk[], pid = "p2", client = 2): ChangeEvent => ({
  version,
  pid,
  client,
  via: "tab",
  root: "source",
  hunks,
  at_ms: 0,
});
const types = (sent: ClientFrame[]) => sent.map((f) => f.type);

/** "ab" at version 1: "c" in flight (op 1), "d" queued behind it, then an undo held after "d". */
function held() {
  const w = wire();
  const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
  l.applyLocal([h(2, 2, "c")], { group: "g1" });
  l.applyLocal([h(3, 3, "d")], { group: "g2" });
  const u = l.undo();
  expect(types(w.sent)).toEqual(["edit"]); // held
  return { w, l, u };
}

describe("a held undo or redo always goes out (held_liveness)", () => {
  it("the edit ahead is acked, and no edit follows: the queue drains to the held undo (held_ack)", () => {
    const { w, u } = held();
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [h(2, 2, "c")] });
    expect(types(w.sent)).toEqual(["edit", "edit", "undo"]);
    expect(w.sent[2]).toEqual({ type: "undo", op: u });
  });

  it("the ack comes past a version gap: the `since` reply settles it, then the held undo goes (held_gap)", () => {
    const { w, l, u } = held();
    w.deliver({ type: "ack", op: 1, version: 3, hunks: [h(2, 2, "c")] }); // version 2 not seen yet
    expect(types(w.sent)).toEqual(["edit", "since"]);
    const since = w.sent[1] as Extract<ClientFrame, { type: "since" }>;
    w.deliver({ type: "events", op: since.op, events: [ev(2, [h(0, 0, ">")]), ev(3, [h(3, 3, "c")], "p1", 1)] });
    expect(l.text).toBe(">abcd");
    expect(types(w.sent)).toEqual(["edit", "since", "edit", "undo"]);
    expect(w.sent[3]).toEqual({ type: "undo", op: u });
  });

  it("the edit ahead is refused (a rate limit): the edit behind it goes, then the held undo (held_refused)", () => {
    const { w, l, u } = held();
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E901", message: "slow down" } });
    expect(l.text).toBe("abd");
    expect(types(w.sent)).toEqual(["edit", "edit", "undo"]);
    expect(w.sent[2]).toEqual({ type: "undo", op: u });
  });

  it("the edit ahead's base expired: everything pending is rejected, and the held undo goes at once (held_base_expired)", () => {
    const { w, l, u } = held();
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E112", message: "base expired" } });
    expect(l.rejected.map((r) => r.text)).toEqual(["c", "d"]);
    expect(types(w.sent)).toEqual(["edit", "undo"]);
    expect(w.sent[1]).toEqual({ type: "undo", op: u });
  });

  it("the queued edit ahead meets competing text and leaves: the held undo keeps its place and goes on the ack (held_recarry)", () => {
    const w = wire();
    const l = new LiveText({ text: "abc", version: 1 }, w.t, { client: 1 });
    l.applyLocal([h(3, 3, "!")], { group: "g1" }); // in flight
    l.applyLocal([h(0, 1, "X")], { group: "g2" }); // queued: replaces "a"
    const u = l.undo();
    w.deliver({ type: "change", event: ev(2, [h(0, 1, "Y")]) }); // someone replaced "a" first
    expect(l.rejected.map((r) => r.text)).toEqual(["X"]);
    expect(l.pending).toBe(1); // "!" in flight; nothing queued but the undo
    expect(types(w.sent)).toEqual(["edit"]);
    w.deliver({ type: "ack", op: 1, version: 3, hunks: [h(3, 3, "!")] });
    expect(l.text).toBe("Ybc!");
    expect(types(w.sent)).toEqual(["edit", "undo"]);
    expect(w.sent[1]).toEqual({ type: "undo", op: u });
  });

  it("the queued edit ahead composed away to nothing: it is acked locally, and the held undo goes (held_empty)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
    l.applyLocal([h(2, 2, "c")], { group: "g1" }); // in flight
    l.applyLocal([h(3, 3, "d")], { group: "g2" }); // queued
    l.applyLocal([h(3, 4)], { group: "g2" }); // and deleted again: the queued entry is empty
    const u = l.undo();
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [h(2, 2, "c")] });
    expect(l.text).toBe("abc");
    expect(types(w.sent)).toEqual(["edit", "undo"]);
    expect(w.sent[1]).toEqual({ type: "undo", op: u });
  });

  it("an undo, an undo and a redo, all held: each goes, in order, none merged (held_several)", () => {
    const { w, l, u } = held();
    const u2 = l.undo();
    const r = l.redo();
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [h(2, 2, "c")] });
    expect(w.sent.slice(2)).toEqual([
      { type: "undo", op: u },
      { type: "undo", op: u2 },
      { type: "redo", op: r },
    ]);
  });

  it("reconnect with nothing pending but the held undo (a reset while down): it goes on the new connection (held_reconnect_only)", () => {
    const { w, l, u } = held();
    l.disconnected();
    l.reset({ text: "ab", version: 1 }); // the host reset while down: "c" and "d" rejected
    expect(l.pending).toBe(0);
    expect(types(w.sent)).toEqual(["edit"]); // offline: the undo waits
    l.reconnected({ client: 2 });
    expect(w.sent.slice(1)).toEqual([
      { type: "since", op: expect.any(Number), version: 1 },
      { type: "undo", op: u },
    ]);
  });

  it("a snapshot still out at the reconnect is refused; connecting again without resume sends the held undo (held_snapshot_refused)", () => {
    const { w, l, u } = held();
    l.disconnected();
    l.reset({ text: "ab", version: 1 }); // the undo stays held while down
    l.reload(); // the host asked for a snapshot while down (dropped)
    l.reconnected({ client: 2 }); // a snapshot is still out: asked again, nothing else
    const snaps = w.sent.filter((f) => f.type === "snapshot") as Array<Extract<ClientFrame, { type: "snapshot" }>>;
    expect(snaps).toHaveLength(2);
    w.deliver({ type: "refused", op: snaps[1].op, error: { id: "MPC-E000", message: "no snapshot" } });
    expect(types(w.sent).filter((t) => t === "undo")).toEqual([]); // still held
    // WIRE.md: a refused snapshot means connecting again without `resume`; the welcome has text
    l.reconnected({ client: 3, version: 1, text: "ab" });
    expect(w.sent.filter((f) => f.type === "undo")).toEqual([{ type: "undo", op: u }]);
  });

  it("close() while held: the undo is dropped and nothing is sent after, whatever arrives (held_close)", () => {
    const { w, l } = held();
    l.close();
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [h(2, 2, "c")] });
    expect(types(w.sent)).toEqual(["edit"]);
    expect(() => l.applyLocal([h(0, 0, "x")])).toThrow(/closed/);
    l.reconnected({ client: 2 });
    expect(types(w.sent)).toEqual(["edit"]);
  });
});

describe("a closed LiveText sends nothing (closed_sends)", () => {
  /** "ab" at version 1, a caret set, then closed: nothing sent but the caret before it. */
  function closed() {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
    l.setCursor(1, 1);
    l.close();
    const before = w.sent.length;
    return { w, l, after: () => w.sent.slice(before) };
  }

  it("reset() after close with an undo held: a no-op, the undo not sent (closed_reset)", () => {
    const { w, l } = held();
    l.close();
    l.reset({ text: "ab", version: 1 });
    expect(types(w.sent)).toEqual(["edit"]);
  });

  it("undo() after close throws, as applyLocal does, and sends nothing (closed_undo)", () => {
    const { l, after } = closed();
    expect(() => l.undo()).toThrow(/closed/);
    expect(after()).toEqual([]);
  });

  it("redo() after close throws, as applyLocal does, and sends nothing (closed_redo)", () => {
    const { l, after } = closed();
    expect(() => l.redo()).toThrow(/closed/);
    expect(after()).toEqual([]);
  });

  it("setVisible() after close: a no-op (closed_visible)", () => {
    const { l, after } = closed();
    l.setVisible(false);
    expect(after()).toEqual([]);
  });

  it("setView() after close: a no-op (closed_view)", () => {
    const { l, after } = closed();
    l.setView(1, 3);
    expect(after()).toEqual([]);
  });

  it("setCursor() after close: a no-op (closed_cursor)", () => {
    const { l, after } = closed();
    l.setCursor(2, 2);
    expect(after()).toEqual([]);
  });
});
