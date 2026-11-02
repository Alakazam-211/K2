// Re-apply by the server's rule (`LiveText.placeRejection`), against the kit's own rebase
// vectors (`test/vectors/live_rebase.json`, written by the Rust kit): a replica at the case's
// base sends `ours`, it is refused, the case's changes since arrive, and Re-apply must give
// exactly the kit's result (or refuse where the kit refuses). Both orders: the changes since
// before the refusal (carried in flight, doomed on a conflict) and after it. Then the history's
// reach, and placement through the person's own pending changes. A planted fault (M1's region
// placement) must fail the vectors.

import { existsSync, readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";
import { apply, coalesce, type Hunk } from "../src/hunk";
import { LiveText, type Landed, type Rejection } from "../src/live";
import type { ChangeEvent, ClientFrame, ServerFrame, Transport } from "../src/wire";
import { KIND_IGNORED, kindsAs, NONE_AS_UNDO, OTHER_AS_UNDO, UNDO_AS_EDIT, withRegionPlacement } from "./plants";

type W = [number, number, string];
const hunks = (ws: W[]): Hunk[] => ws.map(([start, end, insert]) => ({ start, end, insert }));
const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
const ev = (version: number, hs: Hunk[], client = 2): ChangeEvent => ({ version, pid: "p2", client, via: "tab", root: "source", hunks: hs, at_ms: 0 });

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

interface Case {
  base: string;
  ours: W[];
  since: W[][];
  server: { ok: W[] } | { conflict: unknown };
}

const files = [
  new URL("./vectors/live_rebase.json", import.meta.url),
  new URL("../../crates/base/tests/vectors/live_rebase.json", import.meta.url),
].filter((p) => existsSync(p));
const cases: Case[] = JSON.parse(readFileSync(files[0], "utf8")).cases;

/** One case through a replica; `order`: the changes since before or after the refusal. */
function replay(c: Case, order: "since-first" | "refused-first"): { text: string; after: string; placed: ReturnType<LiveText["placeRejection"]>; expected: string | null } {
  const w = wire();
  const l = new LiveText({ text: c.base, version: 1 }, w.t, { client: 1 });
  const rej: Rejection[] = [];
  l.onRejected((r) => rej.push(r));
  l.applyLocal(hunks(c.ours));
  const since = c.since.map(hunks);
  let after = c.base;
  const deliverSince = () =>
    since.forEach((hs, k) => {
      w.deliver({ type: "change", event: ev(2 + k, hs) });
      after = apply(after, hs);
    });
  const refuse = () => {
    const doomed = order === "since-first" && "conflict" in c.server;
    w.deliver({ type: "refused", op: 1, error: doomed ? { id: "MPC-E111", message: "write conflict" } : { id: "MPC-E901", message: "rate" } });
  };
  if (order === "since-first") {
    deliverSince();
    refuse();
  } else {
    refuse();
    deliverSince();
  }
  expect(rej.length, `${JSON.stringify(c)} ${order}: one rejection`).toBe(1);
  const placed = l.placeRejection(rej[0].id);
  l.reapply(rej[0].id);
  return { text: l.text, after, placed, expected: "ok" in c.server ? apply(after, hunks(c.server.ok)) : null };
}

/** Cases whose changes since are canonical (as the window holds them: touching hunks joined). */
const canonical = cases.filter((c) => c.ours.length > 0 && c.since.every((s) => JSON.stringify(coalesce(hunks(s))) === JSON.stringify(hunks(s))));

describe("Re-apply against the kit's rebase vectors", () => {
  it(`every case with a change of ours (${canonical.length} of ${cases.length}, its changes since canonical): the kit's result, or refused where the kit refuses`, () => {
    expect(canonical.length).toBeGreaterThanOrEqual(150);
    let ok = 0;
    let refused = 0;
    let multi = 0;
    for (const c of canonical) {
      for (const order of ["since-first", "refused-first"] as const) {
        const r = replay(c, order);
        if (r.expected === null) {
          expect(r.placed, `${JSON.stringify(c)} ${order}: refused`).toEqual({ refused: "conflict" });
          expect(r.text, `${JSON.stringify(c)} ${order}: nothing applied`).toBe(r.after);
          refused++;
        } else {
          expect(r.text, `${JSON.stringify(c)} ${order}: the kit's result`).toBe(r.expected);
          ok++;
          if (c.ours.length > 1) multi++;
        }
      }
    }
    expect(ok).toBeGreaterThan(0);
    expect(refused).toBeGreaterThan(0);
    expect(multi, "cases with several hunks of ours placed").toBeGreaterThan(20);
  });

  it("planted fault (M1's region placement): the vectors catch it", async () => {
    let wrong = 0;
    await withRegionPlacement(async () => {
      for (const c of canonical) {
        if (!("ok" in c.server)) continue;
        const r = replay(c, "refused-first");
        if (r.text !== r.expected) wrong++;
      }
    });
    expect(wrong, "the fault went unnoticed").toBeGreaterThan(10);
  });
});

describe("Re-apply's reach and the person's own pending changes", () => {
  function refusedAt(n: number) {
    const w = wire();
    const l = new LiveText({ text: "abc", version: 1 }, w.t, { client: 1 });
    const rej: Rejection[] = [];
    l.onRejected((r) => rej.push(r));
    l.applyLocal([h(3, 3, "X")]);
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E901", message: "rate" } });
    for (let k = 0; k < n; k++) w.deliver({ type: "change", event: ev(2 + k, [h(0, 0, "z")]) });
    return { l, id: rej[0].id };
  }

  it("the history reaches back 512 changes: placed after 512, cant-place after 513", () => {
    const a = refusedAt(512);
    expect(a.l.placeRejection(a.id)).toEqual({ hunks: [h(515, 515, "X")] });
    const b = refusedAt(513);
    expect(b.l.placeRejection(b.id)).toEqual({ refused: "cant-place" });
    expect(b.l.reapply(b.id)).toBeNull();
    expect(b.l.rejected.map((r) => r.id)).toEqual([b.id]); // kept
  });

  it("after a reset: cant-place (never placed by its region)", () => {
    const a = refusedAt(1);
    a.l.reset({ text: "zabc!", version: 9 });
    expect(a.l.placeRejection(a.id)).toEqual({ refused: "cant-place" });
  });

  it("through the person's own pending changes, ours after (they go to the server first)", () => {
    const w = wire();
    const l = new LiveText({ text: "abc", version: 1 }, w.t, { client: 1 });
    const rej: Rejection[] = [];
    l.onRejected((r) => rej.push(r));
    l.applyLocal([h(1, 1, "X")]);
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E901", message: "rate" } });
    l.applyLocal([h(1, 1, "Y")]); // in flight now, at the same place
    expect(l.placeRejection(rej[0].id)).toEqual({ hunks: [h(2, 2, "X")] });
    l.reapply(rej[0].id);
    expect(l.text).toBe("aYXbc");
    // acked as the server applies them: Y, then X after it
    w.deliver({ type: "ack", op: 2, version: 2, hunks: [h(1, 1, "Y")] });
    const xFrame = w.sent.filter((f) => f.type === "edit").at(-1)!;
    expect(xFrame).toMatchObject({ type: "edit", base: 2, hunks: [h(2, 2, "X")] });
  });

  it("the deprecated reapplyGuard answers from placeRejection (ok, or the refusal)", () => {
    const a = refusedAt(1);
    expect(a.l.reapplyGuard(a.id)).toBe("ok");
    const b = refusedAt(513);
    expect([b.l.reapplyGuard(b.id), b.l.reapplyGuard(999)]).toEqual(["cant-place", null]);
  });

  it("a conflict with the person's own pending change: refused", () => {
    const w = wire();
    const l = new LiveText({ text: "abcdef", version: 1 }, w.t, { client: 1 });
    const rej: Rejection[] = [];
    l.onRejected((r) => rej.push(r));
    l.applyLocal([h(1, 3, "Q")]); // a replacement...
    w.deliver({ type: "refused", op: 1, error: { id: "MPC-E901", message: "rate" } });
    l.applyLocal([h(0, 4, "")]); // ...whose text the person deleted since (pending)
    expect(l.placeRejection(rej[0].id)).toEqual({ refused: "conflict" });
    expect(l.reapply(rej[0].id)).toBeNull();
  });
});

/**
 * A lost change (`MPC-E000`) and the changes that come later from the client ID it went out on,
 * by their `kind` (kit v0.10.1). The replica types "X" at the end of "abc" (client 1), asks for
 * an undo (`undoInFlight`: unanswered when the connection goes), reconnects as client 2, and the
 * resync doesn't list "X". Then late changes under client 1: `undo` deletes the "a" (the undo
 * that was in flight), `edit` is "X" itself landing. A kind of `null` stands for an older kit's
 * event (no `kind`).
 */
describe("a lost change's landing, by the kinds of the changes from its client ID", () => {
  type K = ChangeEvent["kind"] | null;
  function lost(undoInFlight = true) {
    const w = wire();
    const l = new LiveText({ text: "abc", version: 1 }, w.t, { client: 1 });
    const rej: Rejection[] = [];
    const landed: Landed[] = [];
    l.onRejected((r) => rej.push(r));
    l.onLanded((x) => landed.push(x));
    l.applyLocal([h(3, 3, "X")]);
    if (undoInFlight) l.undo();
    l.reconnected({ client: 2 });
    const since = w.sent.filter((f) => f.type === "since").at(-1) as { op: number };
    w.deliver({ type: "events", op: since.op, events: [] }); // not listed: MPC-E000
    expect(rej.map((r) => r.error?.id)).toEqual(["MPC-E000"]);
    expect(rej[0].asOf).toEqual({ version: 1, client: 1, undoInFlight });
    const id = rej[0].id;
    let v = 1;
    let text = "abc";
    /** A change from client 1 (the lost change's) arrives: its undo or the edit itself. */
    const late = (what: "undo" | "edit", kind: K) => {
      const hs = what === "undo" ? [h(0, 1, "")] : [h(text.length, text.length, "X")];
      v++;
      text = apply(text, hs);
      const event: ChangeEvent = { version: v, pid: "p1", client: 1, via: "tab", root: "source", hunks: hs, at_ms: 0, ...(kind === null ? {} : { kind }) };
      w.deliver({ type: "change", event });
      return v;
    };
    const count = () => l.text.split("X").length - 1;
    return { l, w, id, landed, late, count };
  }

  // each case throws when its rule is broken (the planted faults below must make it throw)
  const cases = {
    "(a) a late undo, the edit never: absent, and Re-apply applies once": () => {
      const s = lost();
      s.late("undo", "undo");
      expect(s.landed).toEqual([]);
      expect(s.l.presence(s.id)).toBe("absent");
      expect(s.l.reapply(s.id)).not.toBeNull();
      expect(s.l.text).toBe("bcX");
      expect(s.w.sent.filter((f) => f.type === "edit").at(-1)).toMatchObject({ base: 2, hunks: [h(2, 2, "X")] });
      expect(s.l.reapply(s.id)).toBeNull(); // once
    },
    "(a) a late redo counts as an undo: absent": () => {
      const s = lost();
      s.late("undo", "redo");
      expect(s.l.presence(s.id)).toBe("absent");
    },
    "(b) a late edit: landed at its version, gone from rejected, nothing to re-apply": () => {
      const s = lost();
      const at = s.late("edit", "edit");
      expect(s.landed.map((x) => [x.rejection.id, x.version])).toEqual([[s.id, at]]);
      expect(s.l.rejected).toEqual([]);
      expect(s.l.reapply(s.id)).toBeNull();
      expect(s.count()).toBe(1);
    },
    "(c) the undo, then the edit: landed once, at the edit's version, no doubled text": () => {
      const s = lost();
      s.late("undo", "undo");
      expect(s.l.presence(s.id)).toBe("absent"); // between the two
      const at = s.late("edit", "edit");
      expect(s.landed.map((x) => x.version)).toEqual([at]);
      expect(s.l.reapply(s.id)).toBeNull();
      expect(s.count()).toBe(1);
    },
    "(c) the edit, then the undo: landed once, at the edit's version, no doubled text": () => {
      const s = lost();
      const at = s.late("edit", "edit");
      s.late("undo", "undo");
      expect(s.landed.map((x) => x.version)).toEqual([at]);
      expect(s.l.reapply(s.id)).toBeNull();
      expect(s.count()).toBe(1);
    },
    "(d) mixed, the undo with a kind and the edit without: unknown, nothing landed": () => {
      const s = lost();
      s.late("undo", "undo");
      s.late("edit", null);
      expect(s.landed).toEqual([]);
      expect(s.l.presence(s.id)).toBe("unknown");
    },
    "(d) mixed, the undo without a kind and the edit with: unknown, nothing landed": () => {
      const s = lost();
      s.late("undo", null);
      s.late("edit", "edit");
      expect(s.landed).toEqual([]);
      expect(s.l.presence(s.id)).toBe("unknown");
    },
    "(d) mixed, the edit without a kind and the undo with: unknown, nothing landed": () => {
      const s = lost();
      s.late("edit", null);
      s.late("undo", "undo");
      expect(s.landed).toEqual([]);
      expect(s.l.presence(s.id)).toBe("unknown");
    },
    "(e) another kind under the client's own ID (a revert): unknown, nothing landed, with or without an undo in flight": () => {
      for (const undo of [true, false]) {
        const s = lost(undo);
        s.late("undo", "revert");
        expect(s.landed).toEqual([]);
        expect(s.l.presence(s.id)).toBe("unknown");
      }
    },
    "(e) an edit beside another kind: unknown, nothing landed": () => {
      const s = lost();
      s.late("undo", "external");
      s.late("edit", "edit");
      expect(s.landed).toEqual([]);
      expect(s.l.presence(s.id)).toBe("unknown");
    },
  };
  for (const [name, f] of Object.entries(cases)) it(name, f);

  it("an older kit (no kind): as before kinds, exactly", () => {
    // an undo in flight: any change from the ID leaves it unknown, and nothing lands
    const a = lost();
    a.late("undo", null);
    expect(a.landed).toEqual([]);
    expect(a.l.presence(a.id)).toBe("unknown");
    const b = lost();
    b.late("edit", null);
    expect(b.landed).toEqual([]);
    expect(b.l.presence(b.id)).toBe("unknown");
    // none in flight: the first change from the ID is the change
    const c = lost(false);
    const at = c.late("edit", null);
    expect(c.landed.map((x) => x.version)).toEqual([at]);
    // nothing from the ID yet: absent
    const d = lost();
    expect(d.l.presence(d.id)).toBe("absent");
  });

  it("with kinds and no undo in flight: the edit lands; an undo under the ID (unexpected) is not the edit", () => {
    const a = lost(false);
    const at = a.late("edit", "edit");
    expect(a.landed.map((x) => x.version)).toEqual([at]);
    const b = lost(false);
    b.late("undo", "undo");
    expect(b.landed).toEqual([]);
    expect(b.l.presence(b.id)).toBe("absent");
  });

  it("past the history's reach: unknown, and a late edit doesn't land", () => {
    const s = lost();
    for (let k = 0; k < 513; k++) {
      const v = 2 + k;
      s.w.deliver({ type: "change", event: { version: v, pid: "p3", client: 3, via: "tab", root: "source", hunks: [h(0, 0, "z")], at_ms: 0, kind: "edit" } });
    }
    expect(s.l.presence(s.id)).toBe("unknown");
    s.w.deliver({ type: "change", event: { version: 515, pid: "p1", client: 1, via: "tab", root: "source", hunks: [h(0, 0, "X")], at_ms: 0, kind: "edit" } });
    expect(s.landed).toEqual([]);
    expect(s.l.presence(s.id)).toBe("unknown");
  });

  // each planted fault must break the cases it names
  const faults: Array<[string, Parameters<typeof kindsAs>[0], string[]]> = [
    ["an undo read as the edit", UNDO_AS_EDIT, ["(a) a late undo, the edit never: absent, and Re-apply applies once", "(a) a late redo counts as an undo: absent", "(c) the undo, then the edit: landed once, at the edit's version, no doubled text"]],
    ["kinds ignored (the older kit's rule)", KIND_IGNORED, ["(a) a late undo, the edit never: absent, and Re-apply applies once", "(b) a late edit: landed at its version, gone from rejected, nothing to re-apply", "(c) the undo, then the edit: landed once, at the edit's version, no doubled text", "(c) the edit, then the undo: landed once, at the edit's version, no doubled text"]],
    ["another kind read as an undo", OTHER_AS_UNDO, ["(e) another kind under the client's own ID (a revert): unknown, nothing landed, with or without an undo in flight", "(e) an edit beside another kind: unknown, nothing landed"]],
    ["no kind beside kinds read as an undo", NONE_AS_UNDO, ["(d) mixed, the undo with a kind and the edit without: unknown, nothing landed", "(d) mixed, the undo without a kind and the edit with: unknown, nothing landed", "(d) mixed, the edit without a kind and the undo with: unknown, nothing landed"]],
  ];
  for (const [name, fault, broken] of faults) {
    it(`planted fault (${name}): ${broken.length} cases catch it`, () => {
      for (const c of broken) expect(() => kindsAs(fault, cases[c as keyof typeof cases]), `${c} missed the fault`).toThrow();
    });
  }
});
