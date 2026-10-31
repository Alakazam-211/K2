// The mounted-editor convergence harness: three live editors (CodeMirror, jsdom) and the fake
// server; random typing through the editors, outside edits, undo, and sockets dropped (the
// edit in flight arriving or lost with them), each editor reconnecting with a new client ID
// and resuming. A run fails loudly on a divergence, on an editor that disagrees with its
// replica, on an unsettled edit, or on an MPC-E000 for an edit the server applied.

import { expect, vi } from "vitest";
import "./jsdom-shims";
import { mountLiveEditor, type LiveEditorHandle } from "../src/editor/mount";
import { byteToIndex, coalesce, utf8Len, type Hunk } from "../src/hunk";
import { Bias, transform } from "../src/position";
import { LiveText } from "../src/live";
import type { ChangeEvent } from "../src/wire";
import { FakeLiveHost, microtasks, netTimers, type FakeLiveTransport } from "./fake-live";
import { FakeServer, Net, rng } from "./fake-server";
import { randomEdit, randomText } from "./random-edits";

const UNDO_REFUSALS = new Set(["MPC-E201", "MPC-E202", "MPC-E203"]);

interface Ed {
  h: LiveEditorHandle;
  t: FakeLiveTransport;
  ops: Map<number, "pending" | "acked" | "rejected">;
  typed: number;
  /** Each MPC-E000: the edit's wire op, and whether it was marked `mayHaveApplied`. */
  e000: Array<{ id: number; op: number; may: boolean }>;
  diverged: string[];
  errors: string[];
}

export interface RunOptions {
  steps?: number;
  drops?: number;
  maxLatency?: number;
  /**
   * `strict` (default): the host follows the reconnect rule (WIRE.md: `prev` leaves the old
   * connection first). `no-rule`: it doesn't (a dropped socket's last edit can land after the
   * resync; `noRuleLeave` says when the host leaves it).
   */
  host?: "strict" | "no-rule";
  noRuleLeave?: "close-seen" | "keepalive";
  /** A dropped socket's close reaches the client at once (it reconnects and resyncs while
   * frames it sent are still on their way: the reconnect race, when the host has no rule). */
  fastNotice?: boolean;
  /** Frames still on a dropped (half-open) socket arrive this much later. */
  lateOldFrames?: number;
  /** Debugging: the server's log and each editor's frames, printed at the end. */
  trace?: boolean;
  /** At the end, Re-apply every rejection (as a person would), then settle again. */
  reapplyAll?: boolean;
}

export interface RunResult {
  typed: number;
  rejected: number;
  reconnects: number;
  drops: number;
  /** MPC-E000s for edits the server applied after all. */
  falseE000: number;
  /** Re-apply results, by status. */
  reapplied: Record<string, number>;
  /** Frames the host dropped unapplied from connections `prev` replaced. */
  lateDiscarded: number;
}

export async function run(seed: number, opts: RunOptions = {}): Promise<RunResult> {
  const r = rng(seed);
  const net = new Net();
  const server = new FakeServer(net, randomText(r, 10));
  if (opts.host === "no-rule") {
    server.leaveOldOnReconnect = false;
    server.noRuleLeave = opts.noRuleLeave ?? "close-seen";
  }
  const maxLatency = opts.maxLatency ?? 1 + r.int(30);
  const host = new FakeLiveHost(net, server, () => r.int(maxLatency + 1));
  const eds: Ed[] = [];
  try {
    return await runIn(seed, opts, r, net, server, maxLatency, host, eds);
  } finally {
    if (opts.trace) {
      for (const a of server.log) console.log(`TRACE log v${a.version} ${a.pid} client ${a.client} ${JSON.stringify(a.hunks)} | before ${JSON.stringify(a.before)}`);
      for (const [k, v] of server.appliedAt) console.log(`TRACE applied ${k} at v${v}`);
      console.log(`TRACE final ${JSON.stringify(server.text)} v${server.version}`);
    }
    for (const e of eds) e.h.destroy();
    document.body.replaceChildren();
  }
}

async function runIn(
  seed: number,
  opts: RunOptions,
  r: ReturnType<typeof rng>,
  net: Net,
  server: FakeServer,
  maxLatency: number,
  host: FakeLiveHost,
  eds: Ed[],
) {
  for (let i = 0; i < 3; i++) {
    const el = document.createElement("div");
    document.body.append(el);
    const t = host.transport(`p${i + 1}`);
    if (opts.trace) {
      t.onFrame((f) => {
        if (f.type === "events") console.log(`TRACE t=${net.now} p${i + 1} events ${JSON.stringify(f.events.map((x) => [x.version, x.client, x.hunks]))}`);
      });
      const send = t.send.bind(t);
      t.send = (f) => {
        if (f.type === "edit" || f.type === "since") console.log(`TRACE t=${net.now} p${i + 1} conn ${t.conn} sends ${JSON.stringify(f)}`);
        send(f);
      };
    }
    const h = mountLiveEditor(el, {
      transport: t,
      mode: r.next() < 0.5 ? "markdown" : "plain",
      timers: netTimers(net, r.next),
      reconnect: { minMs: 1, maxMs: 30 },
    });
    const e: Ed = { h, t, ops: new Map(), typed: 0, e000: [], diverged: [], errors: [] };
    // one local change event per edit the replica took (each takes the next op id)
    h.on("change", (c) => {
      if (!c.local) return;
      e.typed++;
      e.ops.set(e.typed, "pending");
    });
    h.on("ack", (a) => {
      for (const op of a.ops) {
        expect(e.ops.get(op), `seed ${seed} editor ${i} op ${op} acked twice`).toBe("pending");
        e.ops.set(op, "acked");
      }
    });
    h.on("rejected", (rej) => {
      for (const op of rej.ops) {
        expect(e.ops.get(op), `seed ${seed} editor ${i} op ${op} settled twice`).toBe("pending");
        e.ops.set(op, "rejected");
      }
      if (rej.error?.id === "MPC-E000") {
        // the edit in flight is the last one handed over; whether the server applied it is
        // read at the end (on a host without the reconnect rule it can land later)
        const last = [...t.sent].reverse().find((f) => f.type === "edit") as { op: number } | undefined;
        if (last) e.e000.push({ id: rej.id, op: last.op, may: rej.mayHaveApplied });
        if (opts.trace) console.log(`TRACE t=${net.now} p${i + 1} E000 ${JSON.stringify({ id: rej.id, op: last?.op, text: rej.text, region: rej.region, asOf: rej.asOf })}`);
      }
    });
    h.on("reload", (x) => {
      if (x.reason.startsWith("diverged")) e.diverged.push(x.reason);
    });
    h.on("error", (x) => {
      if (!UNDO_REFUSALS.has(x.id)) e.errors.push(`${x.id} ${x.message}`);
    });
    eds.push(e);
  }
  const check = () => {
    for (const [i, e] of eds.entries()) {
      const doc = e.h.view.state.doc.toString();
      expect(doc, `seed ${seed}: editor ${i} shows other text than its replica`).toBe(e.h.text);
    }
  };
  const step = async (n: number) => {
    for (let s = 0; s < n && net.step(); s++) await microtasks();
    await microtasks();
    check();
  };
  await step(1_000);
  const steps = opts.steps ?? 40;
  let drops = 0;
  for (let k = 0; k < steps; k++) {
    const roll = r.next();
    const e = r.pick(eds);
    if (roll < 0.7) {
      if (e.h.state !== "closed" && e.h.rights === "edit") {
        const text = e.h.text;
        const hs = randomEdit(r, text);
        if (hs.length) {
          const changes = hs.map((x) => ({ from: byteToIndex(text, x.start), to: byteToIndex(text, x.end), insert: x.insert }));
          const typing = r.next() < 0.5;
          e.h.view.dispatch({ changes, userEvent: typing ? "input.type" : "delete" });
        }
      }
    } else if (roll < 0.82) {
      server.serverEdit(randomEdit(r, server.text));
    } else if (roll < 0.9 && drops < (opts.drops ?? 3)) {
      if (e.t.conn !== null) {
        const arrives = r.next() < 0.5;
        const notice = r.int(maxLatency + 1);
        e.t.drop(arrives, opts.fastNotice ? 1 : notice, opts.lateOldFrames ?? 0);
        drops++;
      }
    } else if (roll < 0.94) {
      if (e.h.state === "live") e.h.undo();
    }
    await step(r.int(5));
  }
  // let everything land: reconnects, resyncs, flushes
  const land = async () => {
    await step(1_000_000);
    for (let round = 0; round < 50; round++) {
      const behind = eds.filter((e) => e.h.state !== "live" || e.h.pending > 0 || e.h.version !== server.version);
      if (behind.length === 0) break;
      for (const e of behind) e.h.reconnectNow();
      await step(1_000_000);
    }
  };
  await land();
  const applied = (e: Ed, op: number) => server.appliedOps.has(`${e.t.handle}:${op}`);
  /**
   * The oracle for doubling, from the server's own log (not the client's mapping): the edit
   * landed, and some text it inserted is still exactly where it landed. Re-applying it then
   * puts that text in twice.
   */
  const intact = (e: Ed, op: number): boolean => {
    const v = server.appliedAt.get(`${e.t.handle}:${op}`);
    if (v === undefined) return false;
    const landed = server.log.find((a) => a.version === v)!;
    const later = server.log.filter((a) => a.version > v);
    let at = 0; // bytes the landed change shifted the hunks after it by
    for (const h of landed.hunks) {
      const start = h.start + at;
      at += utf8Len(h.insert) - (h.end - h.start);
      if (!h.insert) continue;
      let a = start;
      let b = start + utf8Len(h.insert);
      for (const l of later) {
        a = transform(a, l.hunks, Bias.After);
        b = transform(b, l.hunks, Bias.Before);
      }
      const from = byteToIndex(server.text, a);
      const to = byteToIndex(server.text, b);
      if (from >= 0 && to >= 0 && server.text.slice(from, to) === h.insert) return true;
    }
    return false;
  };
  // a person re-applying every rejection, late: never puts text in twice
  const reapplied: Record<string, number> = {};
  const doubled: string[] = [];
  if (opts.reapplyAll) {
    for (const [i, e] of eds.entries()) {
      for (const rej of [...e.h.rejected]) {
        const res = e.h.reapply(rej.id);
        if (opts.trace) console.log(`TRACE reapply p${i + 1} id ${rej.id} -> ${res.status} | text ${JSON.stringify(e.h.text)}`);
        reapplied[res.status] = (reapplied[res.status] ?? 0) + 1;
        const lost = e.e000.find((x) => x.id === rej.id);
        if (lost && res.status === "applied" && intact(e, lost.op)) doubled.push(`editor ${i} op ${lost.op}`);
      }
    }
    await land();
  }
  let falseE000 = 0;
  for (const [i, e] of eds.entries()) {
    const where = `seed ${seed}, editor ${i}`;
    const wrong = e.e000.filter((x) => applied(e, x.op));
    falseE000 += wrong.length;
    if (opts.host !== "no-rule") {
      expect(wrong.map((x) => `op ${x.op}`), `${where}: MPC-E000 for an edit the server applied`).toEqual([]);
    } else {
      // without the rule it can happen: it must be marked
      expect(wrong.filter((x) => !x.may).map((x) => `op ${x.op}`), `${where}: an unmarked MPC-E000 for an applied edit`).toEqual([]);
    }
    expect(e.diverged, where).toEqual([]);
    expect(e.errors, where).toEqual([]);
    expect(e.h.state, where).toBe("live");
    expect(e.h.pending, where).toBe(0);
    expect(e.h.view.state.doc.toString(), `${where}: text`).toBe(server.text);
    expect(e.h.version, where).toBe(server.version);
    for (const [op, st] of e.ops) expect(st, `${where}: op ${op} never settled`).not.toBe("pending");
  }
  expect(doubled, `seed ${seed}: Re-apply put text in twice`).toEqual([]);
  const typed = eds.reduce((n, e) => n + e.typed, 0);
  const rejected = eds.reduce((n, e) => n + [...e.ops.values()].filter((s) => s === "rejected").length, 0);
  const reconnects = eds.reduce((n, e) => n + e.t.connects - 1, 0);
  return { typed, rejected, reconnects, drops, falseE000, reapplied, lateDiscarded: server.lateDiscarded };
}

/**
 * Run `f` with Re-apply's check planted out: `presence` always says `absent` and a late landing
 * isn't recognised, so a change that landed after all is put in again (the fault the client
 * guard prevents).
 */
export async function withoutPresenceCheck<T>(f: () => Promise<T>): Promise<T> {
  const spy = vi.spyOn(LiveText.prototype, "presence").mockReturnValue("absent");
  // and a late landing goes unrecognised
  const landed = vi.spyOn(LiveText.prototype as unknown as { checkLanded(): void }, "checkLanded").mockImplementation(() => {});
  try {
    return await f();
  } finally {
    spy.mockRestore();
    landed.mockRestore();
  }
}

/**
 * Run `f` with one of Re-apply's guards out (`LiveText.reapplyGuard` says `ok` instead of `g`):
 * the planted fault for that guard.
 */
export async function withoutReapplyGuard<T>(g: "not-insertion" | "reloaded" | "tie" | "deleted-around", f: () => Promise<T>): Promise<T> {
  const orig = LiveText.prototype.reapplyGuard;
  const spy = vi.spyOn(LiveText.prototype, "reapplyGuard").mockImplementation(function (this: LiveText, id: number) {
    const r = orig.call(this, id);
    return r === g ? "ok" : r;
  });
  try {
    return await f();
  } finally {
    spy.mockRestore();
  }
}

/**
 * Run `f` with LiveText recognising its own change in a `since` reply by its hunks alone, as it
 * did before client IDs (the planted fault seed 51 caught: someone else's identical change
 * taken for its own).
 */
export async function withHunkOnlyRecognition<T>(f: () => Promise<T>): Promise<T> {
  type E = { op?: number; hunks: Hunk[] };
  type L = { stash: { op: number; version: number; hunks: Hunk[] } | null };
  const spy = vi.spyOn(LiveText.prototype as unknown as { isOwn(ev: ChangeEvent, e: E): boolean }, "isOwn");
  spy.mockImplementation(function (this: unknown, ev: ChangeEvent, e: E) {
    const self = this as L;
    if (self.stash && self.stash.op === e.op) return self.stash.hunks.length > 0 && ev.version === self.stash.version;
    const hs = coalesce(ev.hunks);
    return hs.length === e.hunks.length && hs.every((h, i) => h.start === e.hunks[i].start && h.end === e.hunks[i].end && h.insert === e.hunks[i].insert);
  });
  try {
    return await f();
  } finally {
    spy.mockRestore();
  }
}
