// A DOM-free test host: what a host editor does with the library, minus the drawing (HOST.md).
// `TestHost` owns one `LiveText` over one `LiveTransport` and follows the host rules exactly:
// the connect and retry loop (`connect({resume, prev})`, then `reconnected(welcome)`; the
// close table's `canRetry` decides, never the transport's flag), `disconnected()` while no
// connection is open (the transport drops, never buffers), Re-apply with presence first for a
// change whose reply was lost, `noteReapplied` after it, and `reload()` on `onReload`. It has
// no undo history of its own: undo and redo are requests to the server.
//
// `run(seed)` is the convergence harness over three of them and the fake server: random typing
// (applyLocal, the caret moved after it as an editor moves it), outside edits, undo, and sockets
// dropped (the edit in flight arriving or lost with them), each host reconnecting with a new
// client ID and resuming. A run fails loudly on a divergence, on an unsettled edit, on an
// MPC-E000 for an edit the server applied (strict host), and on Re-apply putting text in twice
// or placing it other than by the server's rule.

import { expect } from "vitest";
import { apply, byteToIndex, utf8Len, type Hunk } from "../src/hunk";
import { LiveText, setDefaultLiveTimers, type LiveTimers, type Rejection } from "../src/live";
import { Bias, transform } from "../src/position";
import { canRetry, type CloseEvent, type ErrorReport, type LiveTransport, type Rights, type Welcome } from "../src/wire";
import { FakeLiveHost, microtasks, netTimers, type FakeLiveTransport } from "./fake-live";
import { FakeServer, Net, rng } from "./fake-server";
import { serverPlacement } from "./plants";
import { randomEdit, randomText } from "./random-edits";

export type HostState = "connecting" | "live" | "reconnecting" | "closed";

/** What Re-apply did (`TestHost.reapply`): a host's own result type, the kit's rule. */
export type ReapplyStatus =
  | { status: "applied" }
  | { status: "already-present" }
  | { status: "unknown"; reason: "cant-tell" | "conflict" | "cant-place" }
  | { status: "unavailable" };

/** A rejection as the host shows it: `mayHaveApplied` when its reply was lost (MPC-E000). */
export const mayHaveApplied = (r: Rejection) => r.error?.id === "MPC-E000" && r.asOf !== undefined;

export interface TestHostEvents {
  /** The replica took a local edit (typed, or re-applied): each takes the next op id. */
  local(): void;
  ack(e: { ops: number[]; version: number }): void;
  rejected(r: Rejection): void;
  reload(e: { reason: string; error: ErrorReport | null }): void;
  error(e: ErrorReport): void;
  closed(e: CloseEvent): void;
}

export interface TestHostOptions {
  root?: string;
  /** Timers and randomness (the retry jitter); the fake clock in tests. */
  timers: LiveTimers & { random(): number };
  /** Retry after a drop, jittered between these; `false`: a drop closes. */
  reconnect?: { minMs: number; maxMs: number } | false;
  on?: Partial<TestHostEvents>;
}

export class TestHost {
  live: LiveText | null = null;
  state: HostState = "connecting";
  rights: Rights = "none";
  /** Rejections seen landing after all (`onLanded`): Re-apply says already-present. */
  readonly landed = new Set<number>();
  private ac: AbortController | null = null;
  private connecting = false;
  private retryTimer: unknown = null;
  private attempts = 0;
  private destroyed = false;
  private readonly offs: Array<() => void> = [];

  constructor(
    readonly t: LiveTransport,
    private readonly o: TestHostOptions,
  ) {
    // one transport for the host's whole life: its frame and close listeners outlive connections
    this.offs.push(t.onClose((e) => this.onClose(e)));
    this.attempt();
  }

  get text(): string {
    return this.live?.text ?? "";
  }
  get version(): number {
    return this.live?.version ?? 0;
  }
  get pending(): number {
    return this.live?.pending ?? 0;
  }
  get rejected(): readonly Rejection[] {
    return this.live?.rejected ?? [];
  }

  /** Type `hunks` (on the current text), the caret after the last one, as an editor does. */
  type(hunks: Hunk[]): void {
    const live = this.live;
    if (!live || this.state === "closed" || this.rights !== "edit") return;
    live.applyLocal(hunks);
    const at = endOf(hunks, hunks[hunks.length - 1]);
    live.setCursor(at, at);
  }

  undo(): void {
    if (this.live && this.state === "live") this.live.undo();
  }

  /** Re-apply: presence first for a lost reply, then the server's rule (`placeRejection`). */
  reapply(id: number): ReapplyStatus {
    const live = this.live;
    if (!live || this.rights !== "edit" || this.state === "closed") return { status: "unavailable" };
    if (this.landed.has(id)) return { status: "already-present" };
    const r = live.rejected.find((x) => x.id === id);
    if (!r) return { status: "unavailable" };
    const lost = mayHaveApplied(r);
    if (lost) {
      const p = live.presence(id);
      if (p === "present") {
        live.dismiss(id);
        return { status: "already-present" };
      }
      if (p !== "absent") return { status: "unknown", reason: "cant-tell" };
    }
    const p = live.placeRejection(id);
    if (p === null) return { status: "unavailable" };
    if ("refused" in p) return { status: "unknown", reason: p.refused };
    if (p.hunks.length === 0) {
      live.dismiss(id);
      return { status: "already-present" };
    }
    live.reapply(id);
    if (lost) live.noteReapplied(id);
    return { status: "applied" };
  }

  /** Retry now instead of waiting for the backoff (or resync when live). */
  reconnectNow(): void {
    if (this.destroyed) return;
    if (this.state === "live") return this.live?.resync();
    if (this.state !== "reconnecting" || this.connecting) return;
    this.clearRetry();
    this.attempt();
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.ac?.abort();
    this.clearRetry();
    for (const off of this.offs.splice(0)) off();
    this.live?.close();
    this.t.close();
  }

  // ---- the connection

  private attempt(): void {
    if (this.destroyed || this.state === "closed") return;
    this.ac?.abort();
    const ac = new AbortController();
    this.ac = ac;
    this.connecting = true;
    const live = this.live;
    // the previous connection's client ID, so the host leaves it first (WIRE.md, Reconnecting)
    const prev = live?.client;
    // no connection to send on until the welcome: edits wait in LiveText's queue
    live?.disconnected();
    this.state = live ? "reconnecting" : "connecting";
    let p: Promise<Welcome>;
    try {
      p = this.t.connect({ resume: live ? live.version : undefined, ...(prev !== undefined ? { prev } : {}), signal: ac.signal });
    } catch (e) {
      p = Promise.reject(e);
    }
    p.then(
      (w) => {
        if (ac.signal.aborted || this.destroyed) return;
        this.connecting = false;
        this.onWelcome(w);
      },
      (e) => {
        if (ac.signal.aborted || this.destroyed) return;
        this.connecting = false;
        const ce = e as CloseEvent;
        if (ce && typeof ce.reason === "string" && (!canRetry(ce.reason) || this.o.reconnect === false)) return this.close(ce);
        if (this.o.reconnect === false) return this.close({ reason: "network", retry: false });
        this.retryLater();
      },
    );
  }

  private onWelcome(w: Welcome): void {
    const root = this.o.root ?? "source";
    if (w.root !== root) return this.close({ reason: "declined", retry: false });
    this.attempts = 0;
    this.rights = w.rights;
    if (!this.live) {
      if (w.text === undefined) return this.close({ reason: "declined", retry: false });
      this.start(w);
      this.state = "live";
      return;
    }
    this.state = "live";
    // LiveText takes the new client ID, sends caret and visibility again, and catches up: with
    // `text`, from it (pending changes go to `rejected`); without, with `since`
    this.live.reconnected(w);
  }

  private start(w: Welcome): void {
    const live = new LiveText({ text: w.text!, version: w.version }, this.t, { root: w.root, self: w.self, client: w.client });
    this.live = live;
    const on = this.o.on ?? {};
    this.offs.push(
      live.onChange((c) => {
        if (c.local) on.local?.();
      }),
      live.onAck((e) => on.ack?.(e)),
      live.onRejected((r) => on.rejected?.(r)),
      live.onLanded((l) => this.landed.add(l.rejection.id)),
      live.onReload((e) => {
        on.reload?.(e);
        if (this.state === "live") live.reload();
      }),
      live.onError((e) => on.error?.(e)),
    );
  }

  private onClose(e: CloseEvent): void {
    if (this.destroyed || this.state === "closed") return;
    this.ac?.abort();
    this.connecting = false;
    this.live?.disconnected();
    // the close table decides (`canRetry`), never the transport's own `retry`
    if (!canRetry(e.reason) || this.o.reconnect === false) return this.close(e);
    this.state = this.live ? "reconnecting" : "connecting";
    this.retryLater();
  }

  private retryLater(): void {
    if (this.destroyed || this.state === "closed") return;
    this.clearRetry();
    const r = this.o.reconnect || { minMs: 1000, maxMs: 15000 };
    const cap = Math.min(r.maxMs, r.minMs * 2 ** Math.min(this.attempts, 20));
    const delay = Math.max(r.minMs, cap / 2 + (this.o.timers.random() * cap) / 2);
    this.attempts++;
    this.state = this.live ? "reconnecting" : "connecting";
    this.retryTimer = this.o.timers.set(() => {
      this.retryTimer = null;
      this.attempt();
    }, delay);
  }

  private clearRetry(): void {
    if (this.retryTimer !== null) this.o.timers.clear(this.retryTimer);
    this.retryTimer = null;
  }

  private close(e: CloseEvent): void {
    if (this.state === "closed") return;
    this.ac?.abort();
    this.clearRetry();
    this.live?.disconnected();
    this.state = "closed";
    this.o.on?.closed?.(e);
  }
}

/** The byte just after `last`'s insertion in the text after all of `hunks` (sorted, on the
 * text before them). */
function endOf(hunks: Hunk[], last: Hunk): number {
  let shift = 0;
  for (const h of hunks) {
    if (h === last) break;
    shift += utf8Len(h.insert) - (h.end - h.start);
  }
  return last.start + shift + utf8Len(last.insert);
}

// ---- the convergence harness

const UNDO_REFUSALS = new Set(["MPC-E201", "MPC-E202", "MPC-E203"]);

interface Ed {
  h: TestHost;
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
  /** Debugging: the server's log and each host's frames, printed at the end. */
  trace?: boolean;
  /** At the end, Re-apply every rejection (as a person would), then settle again. */
  reapplyAll?: boolean;
  /** Change events carry `kind` (a kit v0.10.1 host; the default); `false`: an older kit's,
   * without it. */
  kind?: boolean;
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
  server.emitKind = opts.kind ?? true;
  if (opts.host === "no-rule") {
    server.leaveOldOnReconnect = false;
    server.noRuleLeave = opts.noRuleLeave ?? "close-seen";
  }
  const maxLatency = opts.maxLatency ?? 1 + r.int(30);
  const host = new FakeLiveHost(net, server, () => r.int(maxLatency + 1));
  const eds: Ed[] = [];
  // the replicas' caret re-send throttle on the fake clock too (a run stays deterministic)
  const wasTimers = setDefaultLiveTimers(netTimers(net, () => 0));
  try {
    return await runIn(seed, opts, r, net, server, maxLatency, host, eds);
  } finally {
    setDefaultLiveTimers(wasTimers);
    if (opts.trace) {
      for (const a of server.log) console.log(`TRACE log v${a.version} ${a.pid} client ${a.client} ${JSON.stringify(a.hunks)} | before ${JSON.stringify(a.before)}`);
      for (const [k, v] of server.appliedAt) console.log(`TRACE applied ${k} at v${v}`);
      console.log(`TRACE final ${JSON.stringify(server.text)} v${server.version}`);
    }
    for (const e of eds) e.h.destroy();
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
    const e: Ed = { h: null as unknown as TestHost, t, ops: new Map(), typed: 0, e000: [], diverged: [], errors: [] };
    e.h = new TestHost(t, {
      timers: netTimers(net, r.next),
      reconnect: { minMs: 1, maxMs: 30 },
      on: {
        // one local change per edit the replica took (each takes the next op id)
        local: () => {
          e.typed++;
          e.ops.set(e.typed, "pending");
        },
        ack: (a) => {
          for (const op of a.ops) {
            expect(e.ops.get(op), `seed ${seed} host ${i} op ${op} acked twice`).toBe("pending");
            e.ops.set(op, "acked");
          }
        },
        rejected: (rej) => {
          for (const op of rej.ops) {
            expect(e.ops.get(op), `seed ${seed} host ${i} op ${op} settled twice`).toBe("pending");
            e.ops.set(op, "rejected");
          }
          if (rej.error?.id === "MPC-E000") {
            // the edit in flight is the last one handed over; whether the server applied it is
            // read at the end (on a host without the reconnect rule it can land later)
            const last = [...t.sent].reverse().find((f) => f.type === "edit") as { op: number } | undefined;
            if (last) e.e000.push({ id: rej.id, op: last.op, may: mayHaveApplied(rej) });
            if (opts.trace) console.log(`TRACE t=${net.now} p${i + 1} E000 ${JSON.stringify({ id: rej.id, op: last?.op, text: rej.text, region: rej.region, asOf: rej.asOf })}`);
          }
        },
        reload: (x) => {
          if (x.reason.startsWith("diverged")) e.diverged.push(x.reason);
        },
        error: (x) => {
          if (!UNDO_REFUSALS.has(x.id)) e.errors.push(`${x.id} ${x.message}`);
        },
      },
    });
    eds.push(e);
  }
  const step = async (n: number) => {
    for (let s = 0; s < n && net.step(); s++) await microtasks();
    await microtasks();
  };
  await step(1_000);
  const steps = opts.steps ?? 40;
  let drops = 0;
  for (let k = 0; k < steps; k++) {
    const roll = r.next();
    const e = r.pick(eds);
    if (roll < 0.7) {
      if (e.h.state !== "closed" && e.h.rights === "edit") {
        const hs = randomEdit(r, e.h.text);
        if (hs.length) e.h.type(hs);
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
  const misplaced: string[] = [];
  if (opts.reapplyAll) {
    for (const [i, e] of eds.entries()) {
      for (const rej of [...e.h.rejected]) {
        // the oracle: where the server would put it, from the server's own log (not the replica's
        // history): its change as sent, rebased from its base, then through the pending changes
        const want = serverPlacement(e.h.live!, server, rej.id);
        const textBefore = e.h.text;
        const res = e.h.reapply(rej.id);
        if (res.status === "applied") {
          if (want === null) misplaced.push(`seed ${seed} host ${i} rejection ${rej.id}: applied where the server's rule refuses`);
          else if (e.h.text !== apply(textBefore, want)) misplaced.push(`seed ${seed} host ${i} rejection ${rej.id}: not where the server's rule puts it`);
        }
        if (opts.trace) console.log(`TRACE reapply p${i + 1} id ${rej.id} -> ${res.status} | text ${JSON.stringify(e.h.text)}`);
        reapplied[res.status] = (reapplied[res.status] ?? 0) + 1;
        const lost = e.e000.find((x) => x.id === rej.id);
        if (lost && res.status === "applied" && intact(e, lost.op)) doubled.push(`host ${i} op ${lost.op}`);
      }
    }
    await land();
  }
  let falseE000 = 0;
  for (const [i, e] of eds.entries()) {
    const where = `seed ${seed}, host ${i}`;
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
    expect(e.h.text, `${where}: text`).toBe(server.text);
    expect(e.h.version, where).toBe(server.version);
    for (const [op, st] of e.ops) expect(st, `${where}: op ${op} never settled`).not.toBe("pending");
  }
  expect(doubled, `seed ${seed}: Re-apply put text in twice`).toEqual([]);
  expect(misplaced, `seed ${seed}: Re-apply placed a change other than by the server's rule`).toEqual([]);
  const typed = eds.reduce((n, e) => n + e.typed, 0);
  const rejected = eds.reduce((n, e) => n + [...e.ops.values()].filter((s) => s === "rejected").length, 0);
  const reconnects = eds.reduce((n, e) => n + e.t.connects - 1, 0);
  return { typed, rejected, reconnects, drops, falseE000, reapplied, lateDiscarded: server.lateDiscarded };
}
