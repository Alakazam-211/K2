import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { LiveText, type Rejection } from "../src/live";
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
const ev = (version: number, hunks: Hunk[], pid = "p2", root = "source"): ChangeEvent => ({
  version,
  pid,
  client: 2,
  via: "tab",
  root,
  hunks,
  at_ms: 0,
});

describe("LiveText", () => {
  it("keeps one change in flight and composes the rest per group (rule 0)", () => {
    const w = wire();
    const l = new LiveText({ text: "abc", version: 5 }, w.t);
    l.applyLocal([h(3, 3, "d")], { group: "g" });
    l.applyLocal([h(4, 4, "e")], { group: "g" });
    l.applyLocal([h(5, 5, "f")], { group: "g" });
    expect(l.text).toBe("abcdef");
    expect(w.sent).toEqual([{ type: "edit", op: 1, root: "source", base: 5, hunks: [h(3, 3, "d")], group: "g" }]);
    expect(l.pending).toBe(2); // one in flight, one queued entry ("ef")
    w.deliver({ type: "ack", op: 1, version: 6, hunks: [h(3, 3, "d")] });
    expect(w.sent[1]).toEqual({ type: "edit", op: 2, root: "source", base: 6, hunks: [h(4, 4, "ef")], group: "g" });
    w.deliver({ type: "ack", op: 2, version: 7, hunks: [h(4, 4, "ef")] });
    expect(l.pending).toBe(0);
    expect(l.version).toBe(7);
  });

  it("rebases incoming changes through its pending one, server first at a tie", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t);
    const changes: Hunk[][] = [];
    l.onChange((e) => !e.local && changes.push(e.hunks));
    l.applyLocal([h(1, 1, "X")]);
    w.deliver({ type: "change", event: ev(2, [h(1, 1, "Y")]) });
    expect(l.text).toBe("aYXb"); // theirs, applied first by the server, stays left
    expect(changes).toEqual([[h(1, 1, "Y")]]);
    w.deliver({ type: "ack", op: 1, version: 3, hunks: [h(2, 2, "X")] });
    expect(l.text).toBe("aYXb");
    expect(l.version).toBe(3);
  });

  it("keeps refused text (MPC-E111): out of the doc, handed to onRejected, re-applicable", () => {
    const w = wire();
    const l = new LiveText({ text: "abcdef", version: 1 }, w.t);
    const rej: Rejection[] = [];
    l.onRejected((r) => rej.push(r));
    l.applyLocal([h(1, 4, "XYZ")]); // a replacement...
    w.deliver({ type: "change", event: ev(2, [h(2, 5, "")]) }); // ...meeting a deletion since
    // the client sees the conflict coming: its change leaves the text now
    expect(l.text).toBe("abf");
    expect(rej).toEqual([]);
    w.deliver({
      type: "refused",
      op: 1,
      error: { id: "MPC-E111", message: "write conflict", details: { conflicts: [] } },
    });
    expect(rej).toHaveLength(1);
    expect(rej[0]).toMatchObject({ text: "XYZ", base: 1, hunks: [h(1, 4, "XYZ")], region: { start: 1, end: 2 } });
    expect(rej[0].error?.id).toBe("MPC-E111");
    // the host offers re-apply at the new base
    l.reapply(rej[0].id);
    expect(l.text).toBe("aXYZbf");
    expect(w.sent[w.sent.length - 1]).toMatchObject({ type: "edit", base: 2, hunks: [h(1, 1, "XYZ")] });
  });

  it("an unforeseen refusal (rate limit) rolls back and keeps later typing", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t);
    const rej: Rejection[] = [];
    l.onRejected((r) => rej.push(r));
    l.applyLocal([h(2, 2, "c")]);
    l.applyLocal([h(0, 0, "Z")], { group: "other" });
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E901", message: "rate" } });
    expect(l.text).toBe("Zab");
    expect(rej.map((r) => r.text)).toEqual(["c"]);
    expect(w.sent[1]).toMatchObject({ type: "edit", base: 1, hunks: [h(0, 0, "Z")] });
  });

  it("MPC-E112: everything pending goes to onRejected and the host reloads", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t);
    const rej: Rejection[] = [];
    const reloads: string[] = [];
    l.onRejected((r) => rej.push(r));
    l.onReload((e) => reloads.push(e.error?.id ?? e.reason));
    l.applyLocal([h(2, 2, "c")]);
    l.applyLocal([h(3, 3, "d")], { group: "x" });
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E112", message: "base expired" } });
    expect(reloads).toEqual(["MPC-E112"]);
    expect(rej.map((r) => r.text)).toEqual(["c", "d"]);
    expect(l.text).toBe("ab");
    l.reset({ text: "abQ", version: 9 });
    expect(l.text).toBe("abQ");
    expect(l.version).toBe(9);
  });

  it("fills a gap with `since`, and ignores other roots' text but not their versions", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t);
    w.deliver({ type: "change", event: ev(2, [h(0, 0, "x")], "p2", "meta") });
    expect(l.version).toBe(2);
    expect(l.text).toBe("ab");
    w.deliver({ type: "change", event: ev(4, [h(0, 0, "z")]) });
    expect(w.sent).toEqual([{ type: "since", op: 1, version: 2 }]);
    w.deliver({ type: "events", op: 1, events: [ev(3, [h(2, 2, "y")]), ev(4, [h(0, 0, "z")])] });
    expect(l.text).toBe("zaby");
    expect(l.version).toBe(4);
  });

  it("maps remote carets to the current text, and sends its own when nothing is pending", () => {
    const w = wire();
    const l = new LiveText({ text: "hello", version: 1 }, w.t);
    l.applyLocal([h(0, 0, ">")]);
    l.setCursor(1, 1);
    expect(w.sent.filter((f) => f.type === "cursor")).toEqual([]);
    w.deliver({
      type: "roster",
      entries: [
        { handle: "p2", name: "Ada", colour: "#f00", kind: "person", role: "editor", away: false, cursor: { root: "source", anchor: 5, head: 5, version: 1 } },
        { handle: "p1", name: "Me", colour: "#00f", kind: "person", role: "editor", away: false, self: true },
      ],
    });
    expect(l.carets()).toEqual([{ handle: "p2", name: "Ada", colour: "#f00", anchor: 6, head: 6 }]);
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [h(0, 0, ">")] });
    expect(w.sent.filter((f) => f.type === "cursor")).toEqual([
      { type: "cursor", root: "source", version: 2, anchor: 1, head: 1 },
    ]);
    expect(l.carets()[0].anchor).toBe(6);
  });

  it("undo and redo are requests; the result arrives as a change", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t);
    const errors: string[] = [];
    l.onError((e) => errors.push(e.id));
    l.undo();
    expect(w.sent).toEqual([{ type: "undo", op: 1 }]);
    w.deliver({ type: "change", event: ev(2, [h(0, 1)], "p1") });
    expect(l.text).toBe("b");
    l.redo();
    w.deliver({ type: "refused", op: 2, error: { id: "MPC-E203", message: "nothing to redo" } });
    expect(errors).toEqual(["MPC-E203"]);
  });

  it("refuses invalid local hunks", () => {
    const l = new LiveText({ text: "é", version: 1 }, wire().t);
    expect(() => l.applyLocal([h(1, 1, "x")])).toThrow(/invalid/);
  });

  it("an ack past a gap waits for `since`, then settles (the host's order slipped)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 2 }, w.t, { client: 1 });
    l.applyLocal([h(2, 2, "!")]);
    // the ack for version 4 arrives before the change at 3
    w.deliver({ type: "ack", op: 1, version: 4, hunks: [h(3, 3, "!")] });
    expect(l.version).toBe(2);
    expect(w.sent[1]).toEqual({ type: "since", op: 2, version: 2 });
    w.deliver({ type: "change", event: ev(3, [h(0, 0, ">")]) }); // dropped: the since covers it
    w.deliver({
      type: "events",
      op: 2,
      events: [ev(3, [h(0, 0, ">")]), { ...ev(4, [h(3, 3, "!")], "p1"), client: 1 }],
    });
    expect(l.text).toBe(">ab!");
    expect(l.version).toBe(4);
    expect(l.pending).toBe(0);
  });

  it("a lost reply: `since` settles the change in flight (applied, or handed to onRejected)", () => {
    const w = wire();
    const l = new LiveText({ text: "ab", version: 1 }, w.t, { client: 1 });
    const acks: number[][] = [];
    const rej: string[] = [];
    l.onAck((a) => acks.push(a.ops));
    l.onRejected((r) => rej.push(r.text));
    const first = l.applyLocal([h(2, 2, "c")]);
    l.resync(); // the reply to the edit was lost
    w.deliver({ type: "events", op: 2, events: [{ ...ev(2, [h(2, 2, "c")], "p1"), client: 1 }] });
    expect(acks).toEqual([[first]]);
    l.applyLocal([h(3, 3, "d")]);
    l.resync();
    w.deliver({ type: "events", op: 4, events: [] }); // not applied: its refusal was lost
    expect(rej).toEqual(["d"]);
    expect(l.text).toBe("abc");
    // a late reply for a settled edit is ignored
    const errors: string[] = [];
    l.onError((e) => errors.push(e.id));
    w.deliver({ type: "refused", op: 3, error: { id: "MPC-E111", message: "late" } });
    expect(errors).toEqual([]);
  });

  it("folds the kit's roster events, stamping a caret without a version", () => {
    const w = wire();
    const l = new LiveText({ text: "hello", version: 3 }, w.t, { self: "p1" });
    const rosters: number[] = [];
    l.onRoster((r) => rosters.push(r.length));
    w.deliver({
      type: "roster_event",
      event: {
        kind: "snapshot",
        version: 3,
        members: [{ handle: "p1", name: "Me", colour: "#00f", kind: "person", role: "editor", away: false }],
        watching: 0,
      },
    });
    w.deliver({ type: "roster_event", event: { kind: "joined", who: "p2", participant: { name: "Ada", colour: "#f00", kind: "person" } } });
    w.deliver({ type: "roster_event", event: { kind: "role", who: "p2", role: "editor" } });
    w.deliver({ type: "roster_event", event: { kind: "cursor", who: "p2", cursor: { root: "source", anchor: 5, head: 5 } } });
    expect(l.roster.find((r) => r.handle === "p2")?.cursor?.version).toBe(3);
    w.deliver({ type: "change", event: ev(4, [h(0, 0, ">>")]) });
    expect(l.carets()).toEqual([{ handle: "p2", name: "Ada", colour: "#f00", anchor: 7, head: 7 }]);
    w.deliver({ type: "roster_event", event: { kind: "watching", count: 4 } });
    expect(l.watchers).toBe(4);
    w.deliver({ type: "roster_event", event: { kind: "left", who: "p2" } });
    expect(l.roster.map((r) => r.handle)).toEqual(["p1"]);
    expect(rosters).toEqual([1, 2, 2, 2, 2, 1]);
  });
});

