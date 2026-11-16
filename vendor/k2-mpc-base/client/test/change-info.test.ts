// `ChangeInfo`'s event fields: a server change hands the host its version, kind, turn, verb and
// time, as its `ChangeEvent` carries them; a change that isn't one server change (the client's
// own edit, a refused change leaving, a reload's diff) has all five `null`.

import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { LiveText, type ChangeInfo } from "../src/live";
import type { ChangeEvent, ClientFrame, ServerFrame, Transport } from "../src/wire";

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
  return { t, sent, deliver: (f: ServerFrame) => ls.forEach((l) => l(f)) };
}

const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
const fields = (c: ChangeInfo) => ({ version: c.version, kind: c.kind, turn: c.turn, verb: c.verb, atMs: c.atMs });
const NONE = { version: null, kind: null, turn: null, verb: null, atMs: null };

function live(text: string, version: number) {
  const w = wire();
  const l = new LiveText({ text, version }, w.t);
  const seen: ChangeInfo[] = [];
  l.onChange((c) => seen.push(c));
  return { w, l, seen };
}

describe("ChangeInfo's event fields", () => {
  it("a remote change carries its event's version, kind, turn, verb and time exactly", () => {
    const { w, seen } = live("abc", 4);
    const event: ChangeEvent = {
      version: 5,
      pid: "agent-7",
      client: 2,
      via: "cli",
      root: "source",
      hunks: [h(3, 3, "d")],
      at_ms: 1_700_000_000_123,
      kind: "edit",
      turn: "t:1a2b.3",
      verb: "replace-section",
    };
    w.deliver({ type: "change", event });
    expect(seen).toHaveLength(1);
    expect(fields(seen[0])).toEqual({ version: 5, kind: "edit", turn: "t:1a2b.3", verb: "replace-section", atMs: 1_700_000_000_123 });
    expect(seen[0]).toMatchObject({ by: "agent-7", local: false, hunks: [h(3, 3, "d")] });
  });

  it("an undo says so, with no verb; an outside save says external", () => {
    const { w, seen } = live("abc", 1);
    const base = { pid: "p2", client: 2, via: "tab", root: "source", turn: null, verb: null };
    w.deliver({ type: "change", event: { ...base, version: 2, hunks: [h(0, 1)], at_ms: 10, kind: "undo" } });
    w.deliver({ type: "change", event: { ...base, version: 3, hunks: [h(0, 0, "x")], at_ms: 20, kind: "external", pid: "kit:external", via: "disk" } });
    expect(seen.map(fields)).toEqual([
      { version: 2, kind: "undo", turn: null, verb: null, atMs: 10 },
      { version: 3, kind: "external", turn: null, verb: null, atMs: 20 },
    ]);
  });

  it("an older host's event without kind, turn and verb gives null for each", () => {
    const { w, seen } = live("abc", 1);
    w.deliver({ type: "change", event: { version: 2, pid: "p2", client: 2, via: "tab", root: "source", hunks: [h(0, 0, "z")], at_ms: 7 } });
    expect(fields(seen[0])).toEqual({ version: 2, kind: null, turn: null, verb: null, atMs: 7 });
  });

  it("the client's own edit, a refused change leaving and a reload's diff have none", () => {
    const { w, l, seen } = live("abc", 1);
    l.applyLocal([h(3, 3, "d")]);
    expect(seen[0].local).toBe(true);
    expect(fields(seen[0])).toEqual(NONE);
    // refused: its text leaves
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E120", message: "refused" } });
    expect(l.text).toBe("abc");
    expect(seen).toHaveLength(2);
    expect(seen[1].local).toBe(false);
    expect(fields(seen[1])).toEqual(NONE);
    // a reload to other text
    l.reset({ text: "xyz", version: 9 });
    expect(seen).toHaveLength(3);
    expect(fields(seen[2])).toEqual(NONE);
  });

  it("a remote change carried through a pending edit keeps its event's fields", () => {
    const { w, l, seen } = live("ab", 1);
    l.applyLocal([h(1, 1, "X")]);
    w.deliver({ type: "change", event: { version: 2, pid: "p2", client: 2, via: "tab", root: "source", hunks: [h(1, 1, "Y")], at_ms: 33, kind: "edit", turn: "t:9", verb: null } });
    expect(l.text).toBe("aYXb");
    const remote = seen.filter((c) => !c.local);
    expect(remote).toHaveLength(1);
    expect(fields(remote[0])).toEqual({ version: 2, kind: "edit", turn: "t:9", verb: null, atMs: 33 });
  });
});
