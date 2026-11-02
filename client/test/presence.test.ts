// Presence and marks as data (`participants()` / `onParticipants`, `marks()` / `onMarks`,
// `insertHerePreview` / `insertHere`): what a host draws its own editor UI from. Offsets are
// UTF-16 code units of the current text, across remote edits, with CJK and emoji; this
// connection is listed once with no caret, the person's other connections as `sameUser`; the
// events come at most once a delivery with the latest snapshot and never when nothing changed.
//
// Each check is a function that throws when it doesn't hold, so the planted faults below can
// show it bites. `PRESENCE_PLANT=<name>` plants one fault in every test (the gate runs each
// against its check and expects a failure).

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { utf8Len } from "../src/hunk";
import { LiveText, type LiveTimers, type Rejection } from "../src/live";
import { participantKind, type Mark, type Participant } from "../src/presence";
import type { CaretAt, ChangeEvent, ClientFrame, Device, RosterEntry, RosterEvent, ServerFrame, Transport } from "../src/wire";

function wire() {
  const ls = new Set<(f: ServerFrame) => void>();
  const sent: ClientFrame[] = [];
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
const entry = (handle: string, name: string, cursor: CaretAt | null, devices?: Device[], kind = "person", colour = "#7c3aed"): RosterEntry => ({
  handle,
  name,
  colour,
  kind,
  role: "editor",
  away: false,
  cursor,
  rights: "edit",
  ...(devices ? { devices } : {}),
});
const snapshot = (members: RosterEntry[], version = 1): RosterEvent => ({ kind: "snapshot", version, members, watching: 0 });
const roster = (ev: RosterEvent): ServerFrame => ({ type: "roster_event", event: ev });
const move = (who: string, c: CaretAt | null, client?: number): ServerFrame =>
  roster({ kind: "cursor", who, cursor: c, version: c?.version ?? 1, ...(client !== undefined ? { client } : {}) });
const changeEv = (version: number, start: number, insert: string, pid = "carol", client = 31, end = start): ChangeEvent => ({
  version,
  pid,
  client,
  via: "tab",
  root: "source",
  hunks: [{ start, end, insert }],
  at_ms: 0,
});
const change = (version: number, start: number, insert: string, pid?: string, client?: number, end?: number): ServerFrame => ({
  type: "change",
  event: changeEv(version, start, insert, pid, client, end),
});
/** UTF-8 offset of UTF-16 index `i` in `s`. */
const b8 = (s: string, i: number) => utf8Len(s.slice(0, i));

/** A clock the test sets; it never fires (the caret re-send stays out of these tests). */
function clock(start = 1000) {
  let t = start;
  const timers: LiveTimers = { set: () => 0, clear: () => {}, now: () => t };
  return { timers, at: (v: number) => void (t = v) };
}

/** A scheduler the test runs by hand: one "frame" is `run()`. */
function frames() {
  const q: Array<() => void> = [];
  return {
    schedule: (f: () => void) => void q.push(f),
    run: () => {
      const fs = q.splice(0);
      fs.forEach((f) => f());
      return fs.length;
    },
    get pending() {
      return q.length;
    },
  };
}

const TEXT = "héllo 日本語 😀 wörld end\n";
const ix = (s: string, sub: string) => {
  const i = s.indexOf(sub);
  if (i < 0) throw new Error(`no ${sub}`);
  return i;
};

/** Ada (this connection: client 11; and her other tab, 12), Bob (21), Carol (no caret). On
 * `TEXT`, Ada's other tab is after the emoji and Bob selects from "日" to "end"; on another
 * text, `at` gives the bytes. */
function room(text = TEXT, at?: { ada: number; bob: [number, number] }) {
  const w = wire();
  const c = clock();
  const f = frames();
  const l = new LiveText({ text, version: 1 }, w.t, { client: 11, self: "ada", timers: c.timers, schedule: f.schedule });
  const ada = at ? at.ada : b8(text, ix(text, "😀") + 2);
  const [ba, bh] = at ? at.bob : [b8(text, ix(text, "日")), b8(text, ix(text, "end"))];
  w.deliver(
    roster(
      snapshot([
        entry("ada", "Ada", caret(ada), [
          { client: 11, cursor: caret(0) },
          { client: 12, cursor: caret(ada), label: "another tab" },
        ]),
        entry("bob", "Bob", caret(ba, bh), [{ client: 21, cursor: caret(ba, bh) }]),
        entry("carol", "Carol", null, []),
      ]),
    ),
  );
  return { w, c, f, l };
}

const of = (ps: Participant[], handle: string, client?: number) => {
  const p = ps.find((x) => x.handle === handle && x.client === client && !x.isSelf);
  if (!p) throw new Error(`no participant ${handle}:${client}`);
  return p;
};

// ---- the checks (each throws when it doesn't hold)

/** Carets in UTF-16 of the current text, through remote inserts before, after and inside them. */
function checkOffsets() {
  const { w, l } = room();
  const at = (s: string, sub: string, plus = 0) => ix(s, sub) + plus;
  let t = TEXT;
  const expectAt = () => {
    expect(l.text).toBe(t);
    const ps = l.participants();
    expect(of(ps, "bob", 21).caret).toEqual({ anchor: at(t, "日"), head: at(t, "end") });
    expect(of(ps, "bob", 21).selections).toEqual([{ anchor: at(t, "日"), head: at(t, "end") }]);
    expect(of(ps, "ada", 12).caret).toEqual({ anchor: at(t, "😀", 2), head: at(t, "😀", 2) });
  };
  expectAt();
  // Carol inserts emoji and CJK at the start: before every caret
  w.deliver(change(2, 0, "🎉漢字"));
  t = "🎉漢字" + t;
  expectAt();
  // after every caret (before the newline): nothing moves
  w.deliver(change(3, b8(t, t.length - 1), "末"));
  t = t.slice(0, -1) + "末\n";
  expectAt();
  // inside Bob's selection, before Ada's caret: the head and Ada's caret move, the anchor not
  const inside = at(t, "😀");
  w.deliver(change(4, b8(t, inside), "ü😀"));
  t = t.slice(0, inside) + "ü😀" + t.slice(inside);
  const ps = l.participants();
  expect(of(ps, "bob", 21).caret).toEqual({ anchor: at(t, "日"), head: at(t, "end") });
  expect(of(ps, "ada", 12).caret).toEqual({ anchor: inside + 3 + 2, head: inside + 3 + 2 });
  // a caret event at the newest version, in bytes, comes out in UTF-16
  w.deliver(move("bob", caret(b8(t, at(t, "wörld", 2)), b8(t, at(t, "wörld", 2)), 4), 21));
  expect(of(l.participants(), "bob", 21).caret).toEqual({ anchor: at(t, "wörld", 2), head: at(t, "wörld", 2) });
}

/** This connection: once, `isSelf`, never a caret; Ada's other tab `sameUser` with its caret. */
function checkOwn() {
  const { w, l } = room();
  const check = () => {
    const ps = l.participants();
    const self = ps.filter((p) => p.isSelf);
    expect(self.length).toBe(1);
    expect(self[0]).toMatchObject({ handle: "ada", client: 11, isSelf: true, sameUser: false, caret: null, selections: [] });
    expect(ps.filter((p) => p.client === 11).length).toBe(1);
    for (const p of ps) if (p.isSelf) expect(p.caret).toBeNull();
    const other = of(ps, "ada", 12);
    expect(other).toMatchObject({ isSelf: false, sameUser: true, label: "another tab" });
    expect(other.caret).not.toBeNull();
    expect(of(ps, "bob", 21).sameUser).toBe(false);
    expect(of(ps, "carol", undefined)).toMatchObject({ caret: null, selections: [], sameUser: false, isSelf: false });
  };
  check();
  // this connection's own caret moves (its own event comes back): still never given a caret
  w.deliver(move("ada", caret(4), 11));
  check();
  l.setCursor(5, 5);
  check();
}

/** 50 caret moves in one frame: one callback, with the latest positions; then nothing. */
function checkBatching() {
  const text = "x".repeat(100) + "\n";
  const { w, f, l } = room(text, { ada: 10, bob: [20, 30] });
  f.run();
  const got: Participant[][] = [];
  l.onParticipants((ps) => got.push(ps));
  for (let i = 0; i < 50; i++) w.deliver(move("bob", caret(i, i + 1), 21));
  expect(got.length).toBe(0); // nothing before the frame
  expect(f.pending).toBe(1); // one delivery asked for, whatever the number of changes
  f.run();
  expect(got.length).toBe(1);
  expect(of(got[0], "bob", 21).caret).toEqual({ anchor: 49, head: 50 });
  expect(got[0]).toEqual(l.participants());
  f.run();
  expect(got.length).toBe(1);
  // the next frame's changes: one more, the latest again (never a stale one)
  w.deliver(move("bob", caret(60), 21));
  w.deliver(move("bob", caret(61), 21));
  f.run();
  expect(got.length).toBe(2);
  expect(of(got[1], "bob", 21).caret).toEqual({ anchor: 61, head: 61 });
}

/** Changes that change no snapshot: no callback (frames may run). */
function checkQuiet() {
  const { w, c, f, l } = room();
  f.run();
  let calls = 0;
  l.onParticipants(() => calls++);
  l.onMarks(() => calls++);
  c.at(5000);
  // someone not in the roster types after every caret, and Bob re-sends his caret unmoved at
  // the new version; a role and a rights change; the person's other tab re-sends its caret
  const t = TEXT;
  w.deliver(change(2, b8(t, t.length - 1), "!", "dave", 41));
  w.deliver(move("bob", caret(b8(t, ix(t, "日")), b8(t, ix(t, "end")), 2), 21));
  w.deliver(roster({ kind: "role", who: "bob", role: "editor" }));
  w.deliver(roster({ kind: "rights", who: "bob", rights: "edit" }));
  w.deliver(move("ada", caret(b8(t, ix(t, "😀") + 2), b8(t, ix(t, "😀") + 2), 2), 12));
  f.run();
  f.run();
  expect(calls).toBe(0);
  // and a real change does call back (the check isn't vacuous)
  w.deliver(move("bob", caret(0), 21));
  f.run();
  expect(calls).toBe(1);
}

/** The person typing and moving the caret alone (no remote activity, everything after the
 * remote carets): no `onParticipants` callback, though the own `lastActiveAt` moves; going
 * away and coming back: one callback each, the own activity riding along. */
function checkSelfQuiet() {
  const { w, c, f, l } = room();
  f.run();
  const got: Participant[][] = [];
  l.onParticipants((ps) => got.push(ps));
  const self = (ps: Participant[]) => ps.find((p) => p.isSelf)!;
  let v = 1;
  for (let i = 0; i < 10; i++) {
    c.at(2000 + i * 100);
    const end = utf8Len(l.text) - 1; // before the newline: after every remote caret
    l.setCursor(end, end);
    l.applyLocal([{ start: end, end, insert: "a" }]);
    const e = w.sent.filter((x): x is Extract<ClientFrame, { type: "edit" }> => x.type === "edit").pop()!;
    w.deliver({ type: "ack", op: e.op, version: ++v, hunks: e.hunks });
    l.setCursor(end + 1, end + 1);
    f.run();
  }
  expect(l.text).toBe(TEXT.slice(0, -1) + "a".repeat(10) + "\n");
  expect(got.length).toBe(0);
  expect(self(l.participants()).lastActiveAt).toBe(2900);
  w.deliver(roster({ kind: "away", who: "ada", away: true }));
  f.run();
  expect(got.length).toBe(1);
  expect(self(got[0])).toMatchObject({ away: true, lastActiveAt: 2900 });
  w.deliver(roster({ kind: "away", who: "ada", away: false }));
  f.run();
  expect(got.length).toBe(2);
  expect(self(got[1]).away).toBe(false);
}

/** `lastActiveAt`: first seen, then each caret move and text change; not an unmoved re-send. */
function checkActive() {
  const { w, c, l } = room();
  const bob = () => of(l.participants(), "bob", 21).lastActiveAt;
  expect(bob()).toBe(1000);
  c.at(2000);
  w.deliver(move("bob", caret(1), 21));
  expect(bob()).toBe(2000);
  // Carol inserts before Bob's caret; Bob's editor re-sends the same place at the new version
  c.at(3000);
  w.deliver(change(2, 0, "日"));
  w.deliver(move("bob", caret(4, 4, 2), 21));
  expect(of(l.participants(), "bob", 21).caret).toEqual({ anchor: 2, head: 2 });
  expect(bob()).toBe(2000);
  // Bob types
  c.at(4000);
  w.deliver(change(3, 0, "b", "bob", 21));
  expect(bob()).toBe(4000);
  // Bob moves again
  c.at(4500);
  w.deliver(move("bob", caret(0, 0, 3), 21));
  expect(bob()).toBe(4500);
  // this connection's own entry: its own moves
  c.at(6000);
  l.setCursor(2, 2);
  expect(l.participants().find((p) => p.isSelf)!.lastActiveAt).toBe(6000);
}

/** An older host (no devices, no client on caret events): one participant a remote person. */
function checkOlderHost() {
  const w = wire();
  const l = new LiveText({ text: TEXT, version: 1 }, w.t, { client: 11, self: "ada", timers: clock().timers, schedule: frames().schedule });
  w.deliver(roster(snapshot([entry("ada", "Ada", caret(3)), entry("bob", "Bob", caret(b8(TEXT, ix(TEXT, "end")))), entry("carol", "Carol", null)])));
  const ps = l.participants();
  expect(ps.map((p) => [p.handle, p.client ?? null, p.isSelf])).toEqual([
    ["ada", 11, true],
    ["bob", null, false],
    ["carol", null, false],
  ]);
  expect(of(ps, "bob").caret).toEqual({ anchor: ix(TEXT, "end"), head: ix(TEXT, "end") });
  expect(of(ps, "carol").caret).toBeNull();
  w.deliver(change(2, 0, "😀"));
  w.deliver(move("bob", caret(b8(TEXT, ix(TEXT, "日")) + 4, b8(TEXT, ix(TEXT, "日")) + 4, 2)));
  const t = "😀" + TEXT;
  expect(of(l.participants(), "bob").caret).toEqual({ anchor: ix(t, "日"), head: ix(t, "日") });
  expect(l.participants().filter((p) => !p.isSelf).length).toBe(2);
}

/** One replica with one rejected change (refused by rate), `hunks` on `text`. */
function rejected(text: string, hunks: Array<{ start: number; end: number; insert: string }>) {
  const w = wire();
  const f = frames();
  const l = new LiveText({ text, version: 1 }, w.t, { client: 1, self: "p1", timers: clock().timers, schedule: f.schedule });
  const rej: Rejection[] = [];
  l.onRejected((r) => rej.push(r));
  l.applyLocal(hunks);
  const op = (w.sent.find((x) => x.type === "edit") as Extract<ClientFrame, { type: "edit" }>).op;
  w.deliver({ type: "refused", op, error: { id: "MPC-E901", message: "rate limit" } });
  expect(rej.length).toBe(1);
  expect(l.text).toBe(text);
  return { w, f, l, id: rej[0].id };
}

/** A rejected replacement: its UTF-16 range and reason, moved by a remote insert. */
function checkMarkRange() {
  const text = "ab日本cd\n";
  // "日本" (bytes 2..8) replaced by "X", refused
  const { w, l, id } = rejected(text, [{ start: 2, end: 8, insert: "X" }]);
  const m = l.marks();
  expect(m.length).toBe(1);
  expect(m[0]).toMatchObject<Partial<Mark>>({
    id,
    range: { from: 2, to: 4 },
    text: "X",
    reason: { code: "MPC-E901", message: "rate limit" },
    mayHaveApplied: false,
    presence: "absent",
    placement: "ok",
    canReapply: true,
    canInsertHere: true,
    removed: true,
  });
  w.deliver(change(2, 0, "😀漢"));
  expect(l.marks()[0].range).toEqual({ from: 5, to: 7 });
}

/** "Insert my text here": exactly where the preview said, once, in its own undo group. */
function checkInsertHere() {
  const text = "ab日本cd\n";
  const { w, l, id } = rejected(text, [{ start: 8, end: 8, insert: "漢字" }]);
  w.deliver(change(2, 0, "😀"));
  // the person's own edits pending around it: one in flight, one queued (no group)
  l.applyLocal([{ start: 0, end: 0, insert: "Q" }]);
  l.applyLocal([{ start: utf8Len(l.text) - 1, end: utf8Len(l.text) - 1, insert: "R" }]);
  const before = l.text;
  const p = l.insertHerePreview(id);
  if (!("at" in p)) throw new Error(`preview refused: ${p.refused}`);
  expect(p).toEqual({ at: ix(before, "cd"), text: "漢字", removed: false });
  expect(l.text).toBe(before); // the preview changed nothing
  const r = l.insertHere(id);
  if (!("op" in r)) throw new Error(`insert refused: ${r.refused}`);
  expect(r.at).toBe(p.at);
  expect(l.text).toBe(before.slice(0, p.at) + "漢字" + before.slice(p.at));
  expect(l.text.split("漢字").length - 1).toBe(1);
  expect(l.marks()).toEqual([]);
  // once: the rejection is gone, a second call does nothing
  expect(l.insertHere(id)).toEqual({ refused: "unavailable" });
  expect(l.text.split("漢字").length - 1).toBe(1);
  // its own undo group: the queued edit goes alone, then the insertion with its own group
  const edits = () => w.sent.filter((x): x is Extract<ClientFrame, { type: "edit" }> => x.type === "edit");
  const inflight = edits()[edits().length - 1];
  expect(inflight.hunks).toEqual([{ start: 0, end: 0, insert: "Q" }]);
  w.deliver({ type: "ack", op: inflight.op, version: 3, hunks: inflight.hunks });
  const queued = edits()[edits().length - 1];
  expect(queued.hunks.map((h) => h.insert)).toEqual(["R"]);
  expect(queued.group).toBeUndefined();
  w.deliver({ type: "ack", op: queued.op, version: 4, hunks: queued.hunks });
  const ins = edits()[edits().length - 1];
  expect(ins.group).toMatch(/^insert-here-/);
  expect(ins.hunks).toEqual([{ start: b8(l.text, p.at), end: b8(l.text, p.at), insert: "漢字" }]);
}

/** A change of more than one hunk: no one place, refused; nothing inserted, the mark kept. */
function checkMultiHunk() {
  const text = "abcdef\n";
  const { l, id } = rejected(text, [
    { start: 1, end: 1, insert: "x" },
    { start: 4, end: 4, insert: "y" },
  ]);
  expect(l.marks()[0].canInsertHere).toBe(false);
  expect(l.insertHerePreview(id)).toEqual({ refused: "multi-hunk" });
  expect(l.insertHere(id)).toEqual({ refused: "multi-hunk" });
  expect(l.text).toBe(text);
  expect(l.marks().map((m) => m.id)).toEqual([id]);
}

// ---- planted faults

const PLANTS: Record<string, () => void> = {
  /** Byte offsets handed out as UTF-16. */
  bytes: () => {
    vi.spyOn(LiveText.prototype as any, "to16").mockImplementation((b: unknown) => b);
  },
  /** No batching: every change delivers at once. */
  "no-batch": () => {
    vi.spyOn(LiveText.prototype as any, "dirty").mockImplementation(function (this: any) {
      if (this.participantWatchers.size + this.markWatchers.size === 0) return;
      this.frameDue = true;
      this.deliver();
    });
  },
  /** A callback on every delivery, changed or not. */
  "always-callback": () => {
    vi.spyOn(LiveText.prototype as any, "deliverTo").mockImplementation(function (this: any, set: any, now: any) {
      const v = now();
      for (const w of [...set]) w.cb(v);
    });
  },
  /** This connection given its own caret. */
  "self-caret": () => {
    const real = LiveText.prototype.participants;
    vi.spyOn(LiveText.prototype, "participants").mockImplementation(function (this: LiveText) {
      return real.call(this).map((p) => (p.isSelf ? { ...p, caret: { anchor: 0, head: 0 }, selections: [{ anchor: 0, head: 0 }] } : p));
    });
  },
  /** Activity by the raw caret: an unmoved re-send at a new version counts as a move. */
  "active-raw": () => {
    vi.spyOn(LiveText.prototype as any, "caretKey").mockImplementation((c: any) => (c ? `${c.version}:${c.anchor}:${c.head}` : "-"));
  },
  /** This connection's own activity compared again: the person's typing calls back. */
  "self-active": () => {
    vi.spyOn(LiveText.prototype as any, "participantsKey").mockImplementation((ps: unknown) => JSON.stringify(ps));
  },
  /** "Insert my text here" without the one-hunk check (and the presence check). */
  "insert-multi": () => {
    vi.spyOn(LiveText.prototype as any, "insertPlan").mockImplementation(function (this: any, id: unknown) {
      const r = this._rejected.find((x: Rejection) => x.id === id);
      return !r || !r.text ? { refused: "unavailable" } : { r, byte: r.region.start };
    });
  },
  /** Local edits lose their undo group (the insertion composes with the queued edit). */
  "insert-group": () => {
    const real = LiveText.prototype.applyLocal;
    vi.spyOn(LiveText.prototype, "applyLocal").mockImplementation(function (this: LiveText, hunks) {
      return real.call(this, hunks, {});
    });
  },
};

/** Each plant and the check that must catch it. */
const CAUGHT_BY: Array<[string, () => void]> = [
  ["bytes", checkOffsets],
  ["bytes", checkMarkRange],
  ["no-batch", checkBatching],
  ["always-callback", checkQuiet],
  ["self-caret", checkOwn],
  ["active-raw", checkActive],
  ["self-active", checkSelfQuiet],
  ["insert-multi", checkMultiHunk],
  ["insert-group", checkInsertHere],
];

const PLANT = process.env.PRESENCE_PLANT ?? "";

beforeEach(() => {
  if (!PLANT) return;
  const p = PLANTS[PLANT];
  if (!p) throw new Error(`unknown PRESENCE_PLANT ${PLANT}`);
  console.log(`PRESENCE_PLANT=${PLANT}: planted (this run must fail)`);
  p();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("participants", () => {
  it("offsets are UTF-16 of the current text across remote edits (ASCII, CJK, emoji; inserts before, after and inside)", () => {
    checkOffsets();
  });
  it("this connection is listed once with no caret; the person's other connection is sameUser and visible", () => {
    checkOwn();
  });
  it("batching: 50 caret moves in one frame give one callback with the latest positions", () => {
    checkBatching();
  });
  it("no callback when nothing changed", () => {
    checkQuiet();
  });
  it("typing alone gives no callback; going away and coming back give one each", () => {
    checkSelfQuiet();
  });
  it("lastActiveAt moves on caret moves and text changes, not on an unmoved re-send", () => {
    checkActive();
  });
  it("an older host (no devices) yields one participant a remote person", () => {
    checkOlderHost();
  });
  it("kind is stable (person, agent, app) and colour is passed through as the roster sent it", () => {
    expect(["person", "agent", "app", "external", "restore", "import", "Agent", "something-new"].map(participantKind)).toEqual([
      "person",
      "agent",
      "app",
      "app",
      "app",
      "app",
      "agent",
      "app",
    ]);
    const w = wire();
    const l = new LiveText({ text: TEXT, version: 1 }, w.t, { client: 11, self: "ada", timers: clock().timers, schedule: frames().schedule });
    w.deliver(
      roster(
        snapshot([
          entry("ada", "Ada", null, [], "person", "#7C3AED"),
          entry("cli", "Agent", null, [], "agent", "hsl(200 50% 40%)"),
          entry("ext", "Outside save", null, [], "external", "teal"),
        ]),
      ),
    );
    expect(l.participants().map((p) => [p.handle, p.kind, p.rawKind, p.colour])).toEqual([
      ["ada", "person", "person", "#7C3AED"],
      ["cli", "agent", "agent", "hsl(200 50% 40%)"],
      ["ext", "app", "external", "teal"],
    ]);
  });
  it("away comes from the roster and is delivered as a change", () => {
    const { w, f, l } = room();
    f.run();
    const got: Participant[][] = [];
    l.onParticipants((ps) => got.push(ps));
    w.deliver(roster({ kind: "away", who: "bob", away: true }));
    f.run();
    expect(got.length).toBe(1);
    expect(of(got[0], "bob", 21).away).toBe(true);
  });
  it("unsubscribe stops callbacks; close stops deliveries", () => {
    const { w, f, l } = room();
    let calls = 0;
    const off = l.onParticipants(() => calls++);
    w.deliver(move("bob", caret(1), 21));
    off();
    f.run();
    expect(calls).toBe(0);
    l.onParticipants(() => calls++);
    w.deliver(move("bob", caret(2), 21));
    l.close();
    f.run();
    expect(calls).toBe(0);
  });
});

describe("the scheduler (LiveOptions.schedule)", () => {
  it("an injected immediate scheduler delivers each change at once", () => {
    const w = wire();
    const l = new LiveText({ text: "abcdef\n", version: 1 }, w.t, { client: 11, self: "ada", timers: clock().timers, schedule: (d) => d() });
    w.deliver(roster(snapshot([entry("ada", "Ada", null, []), entry("bob", "Bob", null, [])])));
    const got: Array<number | null> = [];
    l.onParticipants((ps) => got.push(of(ps, "bob", 21).caret?.anchor ?? null));
    for (const i of [1, 2, 3]) w.deliver(move("bob", caret(i), 21));
    expect(got).toEqual([1, 2, 3]);
  });
  it("the default without requestAnimationFrame: one 16 ms timer on LiveOptions.timers", () => {
    vi.stubGlobal("requestAnimationFrame", undefined);
    const sets: Array<{ f: () => void; ms: number }> = [];
    const timers: LiveTimers = { set: (f, ms) => sets.push({ f, ms }), clear: () => {}, now: () => 0 };
    const w = wire();
    const l = new LiveText({ text: "abcdef\n", version: 1 }, w.t, { client: 11, self: "ada", timers });
    w.deliver(roster(snapshot([entry("ada", "Ada", null, []), entry("bob", "Bob", null, [])])));
    const got: Participant[][] = [];
    l.onParticipants((ps) => got.push(ps));
    for (const i of [1, 2, 3, 4, 5]) w.deliver(move("bob", caret(i), 21));
    expect(sets.map((s) => s.ms)).toEqual([16]);
    expect(got.length).toBe(0);
    sets[0].f();
    expect(got.length).toBe(1);
    expect(of(got[0], "bob", 21).caret).toEqual({ anchor: 5, head: 5 });
  });
  it("the default with requestAnimationFrame: one frame request, no timer", () => {
    const rafs: Array<() => void> = [];
    vi.stubGlobal("requestAnimationFrame", (cb: () => void) => rafs.push(cb));
    const sets: number[] = [];
    const timers: LiveTimers = { set: (_f, ms) => sets.push(ms), clear: () => {}, now: () => 0 };
    const w = wire();
    const l = new LiveText({ text: "abcdef\n", version: 1 }, w.t, { client: 11, self: "ada", timers });
    w.deliver(roster(snapshot([entry("ada", "Ada", null, []), entry("bob", "Bob", null, [])])));
    const got: Participant[][] = [];
    l.onParticipants((ps) => got.push(ps));
    for (const i of [1, 2, 3]) w.deliver(move("bob", caret(i), 21));
    expect(rafs.length).toBe(1);
    expect(sets).toEqual([]);
    rafs[0]();
    expect(got.length).toBe(1);
    expect(of(got[0], "bob", 21).caret).toEqual({ anchor: 3, head: 3 });
  });
});

describe("marks", () => {
  it("a rejected change appears with its UTF-16 range and reason, moved by remote edits", () => {
    checkMarkRange();
  });
  it("dismiss removes it (and onMarks says so, once)", () => {
    const { f, l, id } = rejected("ab日本cd\n", [{ start: 8, end: 8, insert: "漢字" }]);
    f.run();
    const got: Mark[][] = [];
    l.onMarks((m) => got.push(m));
    l.dismiss(id);
    expect(l.marks()).toEqual([]);
    f.run();
    f.run();
    expect(got).toEqual([[]]);
  });
  it("reapply follows the existing rule: it puts back what placeRejection places", () => {
    const text = "ab日本cd\n";
    const { w, l, id } = rejected(text, [{ start: 8, end: 8, insert: "漢字" }]);
    w.deliver(change(2, 0, "😀"));
    const m = l.marks()[0];
    expect([m.canReapply, m.placement, m.presence]).toEqual([true, "ok", "absent"]);
    const p = l.placeRejection(id);
    if (!p || !("hunks" in p)) throw new Error("not placed");
    expect(p.hunks).toEqual([{ start: b8("😀" + text, ix("😀" + text, "cd")), end: b8("😀" + text, ix("😀" + text, "cd")), insert: "漢字" }]);
    expect(l.reapply(id)).not.toBeNull();
    expect(l.text).toBe("😀ab日本漢字cd\n");
    expect(l.marks()).toEqual([]);
  });
  it("a change whose reply was lost: Re-apply and insert only once the record shows it absent", () => {
    const w = wire();
    const l = new LiveText({ text: "abc\n", version: 1 }, w.t, { client: 1, self: "p1", timers: clock().timers, schedule: frames().schedule });
    l.undo(); // op 1, unanswered when the connection goes: a later change from client 1 may be it
    l.applyLocal([{ start: 3, end: 3, insert: "X" }]); // op 2, in flight
    l.disconnected();
    l.reconnected({ client: 2 }); // since: op 3
    w.deliver({ type: "events", op: 3, events: [] }); // not listed: MPC-E000
    const m0 = l.marks()[0];
    expect(m0).toMatchObject({ reason: { code: "MPC-E000" }, mayHaveApplied: true, presence: "absent", canReapply: true, canInsertHere: true });
    // a change from client 1 lands: the undo's or the edit's, the record can't tell
    w.deliver(change(2, 0, "Z", "p1", 1));
    const m1 = l.marks()[0];
    expect(m1).toMatchObject({ id: m0.id, presence: "unknown", canReapply: false, canInsertHere: false });
    expect(l.insertHerePreview(m0.id)).toEqual({ refused: "cant-tell" });
    expect(l.insertHere(m0.id)).toEqual({ refused: "cant-tell" });
    expect(l.text).toBe("Zabc\n");
    expect(l.marks().length).toBe(1);
  });
  it("a change whose reply was lost and that lands leaves the marks", () => {
    const w = wire();
    const f = frames();
    const l = new LiveText({ text: "abc\n", version: 1 }, w.t, { client: 1, self: "p1", timers: clock().timers, schedule: f.schedule });
    l.applyLocal([{ start: 3, end: 3, insert: "X" }]); // op 1
    l.disconnected();
    l.reconnected({ client: 2 }); // since: op 2
    w.deliver({ type: "events", op: 2, events: [] });
    f.run();
    const got: Mark[][] = [];
    l.onMarks((m) => got.push(m));
    expect(l.marks().length).toBe(1);
    w.deliver(change(2, 3, "X", "p1", 1));
    f.run();
    expect(l.marks()).toEqual([]);
    expect(got).toEqual([[]]);
  });
  it("insertHere inserts exactly where the preview said, once, as its own undo group", () => {
    checkInsertHere();
  });
  it("insertHere is refused for a multi-hunk change", () => {
    checkMultiHunk();
  });
});

describe.skipIf(PLANT !== "")("planted faults", () => {
  it("each check passes on the real thing", () => {
    for (const [, check] of CAUGHT_BY) expect(check).not.toThrow();
  });
  for (const [plant, check] of CAUGHT_BY) {
    it(`${plant}: ${check.name} fails`, () => {
      PLANTS[plant]();
      try {
        expect(check).toThrow();
      } finally {
        vi.restoreAllMocks();
      }
      expect(check).not.toThrow();
    });
  }
});
