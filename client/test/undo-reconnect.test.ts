// An edit and an undo (or redo) both sent and applied, then the connection drops before their
// replies: the reconnect never sends the undo again (the server would apply it twice: ops aren't
// deduplicated, frames are never replayed), and typing made while down or before the `since` reply
// goes out once the reply settles the edit. With a snapshot still out at the reconnect, the
// snapshot's reset settles everything: nothing is left waiting.

import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { LiveText, type UndoResult } from "../src/live";
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
/** The person's own change (pid p1) under client 1, the connection that went. */
const own = (version: number, hunks: Hunk[], kind: ChangeEvent["kind"]): ChangeEvent => ({
  version,
  pid: "p1",
  client: 1,
  via: "tab",
  kind,
  root: "source",
  hunks,
  at_ms: 0,
});
const of = (sent: ClientFrame[], type: ClientFrame["type"]) => sent.filter((f) => f.type === type);

describe("an edit and an undo or redo sent, then the connection drops before their replies (reconnect_settle)", () => {
  it("undo: never sent again; typing while down and before the reply goes out after it (reconnect_settle_undo)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
    const undos: UndoResult[] = [];
    l.onUndo((u) => undos.push(u));
    l.applyLocal([h(2, 2, "s")], { group: "g1" }); // op 1, in flight
    l.undo(); // nothing queued: sent at once
    expect(w.sent.map((f) => f.type)).toEqual(["edit", "undo"]);
    // the host applied both (versions 2 and 3); the socket went before either reply
    l.disconnected();
    l.applyLocal([h(3, 3, "t")], { group: "g2" }); // typed while down
    l.reconnected({ client: 2 });
    l.applyLocal([h(4, 4, "u")], { group: "g3" }); // typed before the `since` reply
    const since = of(w.sent, "since") as Array<Extract<ClientFrame, { type: "since" }>>;
    expect(since).toHaveLength(1);
    expect(w.sent.slice(2).map((f) => f.type)).toEqual(["since"]); // nothing else yet: "s" is in flight
    w.deliver({ type: "events", op: since[0].op, events: [own(2, [h(2, 2, "s")], "edit"), own(3, [h(2, 3)], "undo")] });
    expect(of(w.sent, "undo")).toHaveLength(1); // never again
    expect(w.sent.slice(3)).toEqual([{ type: "edit", op: expect.any(Number), root: "source", base: 2, hunks: [h(3, 3, "t")], group: "g2" }]);
    expect(l.text).toBe("abtu"); // the undo's change applied once: "s" out, nothing else
    expect(l.version).toBe(3);
    const t = w.sent[3] as Extract<ClientFrame, { type: "edit" }>;
    w.deliver({ type: "ack", op: t.op, version: 4, hunks: [h(2, 2, "t")] });
    expect(w.sent.slice(4)).toEqual([{ type: "edit", op: expect.any(Number), root: "source", base: 4, hunks: [h(3, 3, "u")], group: "g3" }]);
    expect(undos).toEqual([]); // its reply went with the old socket
  });

  it("redo: the same (reconnect_settle_redo)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
    l.applyLocal([h(2, 2, "s")], { group: "g1" });
    l.redo(); // sent at once; the host applied "s", the redo had nothing to redo
    l.disconnected();
    l.applyLocal([h(3, 3, "t")], { group: "g2" });
    l.reconnected({ client: 2 });
    const since = of(w.sent, "since") as Array<Extract<ClientFrame, { type: "since" }>>;
    w.deliver({ type: "events", op: since[0].op, events: [own(2, [h(2, 2, "s")], "edit")] });
    expect(of(w.sent, "redo")).toHaveLength(1);
    expect(w.sent.slice(3)).toEqual([{ type: "edit", op: expect.any(Number), root: "source", base: 2, hunks: [h(3, 3, "t")], group: "g2" }]);
    expect(l.text).toBe("abst");
  });

  it("a snapshot still out at the reconnect: its reset settles the edit and the typing, nothing waits (reconnect_settle_snapshot)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
    l.applyLocal([h(2, 2, "s")], { group: "g1" });
    l.undo();
    l.disconnected();
    l.reload(); // the host asked for a snapshot while down (dropped)
    l.applyLocal([h(3, 3, "t")], { group: "g2" });
    l.reconnected({ client: 2 }); // asked again, nothing else
    const snaps = of(w.sent, "snapshot") as Array<Extract<ClientFrame, { type: "snapshot" }>>;
    expect(snaps).toHaveLength(2);
    expect(of(w.sent, "since")).toEqual([]);
    expect(l.pending).toBe(2); // "s" in flight, "t" queued
    w.deliver({ type: "snapshot", op: snaps[1].op, text: "ab", version: 3 }); // "s" and its undo
    expect(l.pending).toBe(0);
    expect(l.text).toBe("ab");
    expect(l.rejected.map((r) => r.text)).toEqual(["s", "t"]); // handed to onRejected, never dropped
    expect(of(w.sent, "undo")).toHaveLength(1);
    l.applyLocal([h(2, 2, "v")]);
    expect(w.sent[w.sent.length - 1]).toEqual({ type: "edit", op: expect.any(Number), root: "source", base: 3, hunks: [h(2, 2, "v")] });
  });
});

describe("the since reply's own change is the edit in flight only when it is an edit (reconnect_own_kind)", () => {
  /** "abx" at version 1: the person deleted "x" (acked, version 2), then typed it again ("s", in
   * flight) and pressed undo; the connection went before either reply. */
  function retyped() {
    const w = wire();
    const l = new LiveText({ text: "abx", version: 1 }, w.t, { client: 1 });
    const acks: number[][] = [];
    l.onAck((a) => acks.push(a.ops));
    l.applyLocal([h(2, 3)], { group: "g1" });
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [h(2, 3)] });
    const s = l.applyLocal([h(2, 2, "x")], { group: "g2" });
    l.undo();
    l.disconnected();
    l.reconnected({ client: 2 });
    const since = of(w.sent, "since") as Array<Extract<ClientFrame, { type: "since" }>>;
    expect(since).toHaveLength(1);
    return { w, l, acks, s, since: since[0] };
  }

  it("the host refused the edit and undid the step before it, with the same hunks: the undo's change is not the edit (reconnect_own_kind_undo)", () => {
    const { w, l, acks, s, since } = retyped();
    // the refusal went with the socket; the undo put "x" back: the edit's own hunks, kind "undo"
    w.deliver({ type: "events", op: since.op, events: [own(3, [h(2, 2, "x")], "undo")] });
    expect(acks.flat()).not.toContain(s);
    expect(l.rejected.map((r) => [r.ops, r.error?.id])).toEqual([[[s], "MPC-E000"]]);
    expect(l.text).toBe("abx"); // the host's text: the undo's "x", not the edit's
    expect(l.version).toBe(3);
    expect(l.presence(l.rejected[0].id)).toBe("absent");
  });

  it("from an older kit (no kind on change events), the edit's own change is still recognised (reconnect_own_kindless)", () => {
    const { w, l, acks, s, since } = retyped();
    const ev = own(3, [h(2, 2, "x")], "edit");
    delete (ev as { kind?: unknown }).kind;
    w.deliver({ type: "events", op: since.op, events: [ev] });
    expect(acks.flat()).toContain(s);
    expect(l.rejected).toEqual([]);
    expect(l.text).toBe("abx");
    expect(l.pending).toBe(0);
  });
});

