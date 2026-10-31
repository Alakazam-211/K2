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
import { LiveText, type Rejection } from "../src/live";
import type { ChangeEvent, ClientFrame, ServerFrame, Transport } from "../src/wire";
import { withRegionPlacement } from "./editor-harness";

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
