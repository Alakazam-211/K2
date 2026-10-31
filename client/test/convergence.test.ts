// Headless convergence (no DOM): three clients and a fake server applying the E-LIVE rules,
// random edits at random latencies. Every run must converge (every client's text is the
// server's), and nothing typed is lost: each local edit is either acknowledged or handed to
// `onRejected`, exactly once.

import { describe, expect, it } from "vitest";
import { apply } from "../src/hunk";
import { LiveText, type Rejection } from "../src/live";
import { FakeServer, Net, rng } from "./fake-server";
import { randomEdit, randomText, type Kind } from "./random-edits";
import { runrec } from "./runrec";

interface Client {
  live: LiveText;
  ops: Map<number, "pending" | "acked" | "rejected">;
  shadow: string;
  rejections: Rejection[];
}

function run(
  seed: number,
  opts: {
    steps?: number;
    maxLatency?: number;
    window?: number;
    only?: Kind;
    serverEdits?: boolean;
    /** Host-to-client frames dropped with this probability, and overtaking each other. */
    lossy?: number;
  } = {},
) {
  const r = rng(seed);
  const net = new Net();
  const server = new FakeServer(net, randomText(r, 8));
  if (opts.window !== undefined) server.window = opts.window;
  const maxLatency = opts.maxLatency ?? 1 + r.int(40);
  const clients: Client[] = [];
  for (let i = 1; i <= 3; i++) {
    const faults = opts.lossy ? { drop: opts.lossy, reorder: true, next: r.next } : undefined;
    const live = new LiveText(server.snapshot(), server.connect(i, () => r.int(maxLatency + 1), faults), {
      client: i,
    });
    const c: Client = { live, ops: new Map(), shadow: live.text, rejections: [] };
    live.onChange((e) => {
      c.shadow = apply(c.shadow, e.hunks);
    });
    live.onAck((e) => {
      for (const op of e.ops) {
        expect(c.ops.get(op), `client ${i} op ${op} acked twice`).toBe("pending");
        c.ops.set(op, "acked");
      }
    });
    live.onRejected((rej) => {
      c.rejections.push(rej);
      for (const op of rej.ops) {
        expect(c.ops.get(op), `client ${i} op ${op} settled twice`).toBe("pending");
        c.ops.set(op, "rejected");
      }
    });
    live.onReload((e) => {
      // only an expired base may force a reload; a divergence is a bug
      expect(e.reason, `client ${i}, seed ${seed}`).not.toMatch(/^diverged/);
      // a reload starts again from the server's text (pending changes were rejected)
      live.reset(server.snapshot());
    });
    clients.push(c);
  }
  const steps = opts.steps ?? 30;
  for (let k = 0; k < steps; k++) {
    const roll = r.next();
    if (roll < 0.75) {
      const c = r.pick(clients);
      const hs = randomEdit(r, c.live.text, opts.only);
      if (hs.length) {
        const group = r.next() < 0.5 ? "g" : `own-${k}`;
        const id = c.live.applyLocal(hs, { group });
        c.ops.set(id, "pending");
      }
    } else if (roll < 0.85) {
      if (opts.serverEdits !== false) server.serverEdit(randomEdit(r, server.text, opts.only)); // an agent or an outside edit
    } else if (roll < 0.9 && !opts.only) {
      r.pick(clients).live.undo();
    }
    for (let s = r.int(4); s > 0 && net.step(); s--);
    for (const c of clients) expect(c.shadow).toBe(c.live.text);
  }
  while (net.step()) for (const c of clients) expect(c.shadow).toBe(c.live.text);
  // lost frames: the host's reconnect asks again (`resync`) until everyone has caught up
  for (let round = 0; round < 50; round++) {
    const behind = clients.filter((c) => c.live.pending > 0 || c.live.version !== server.version);
    if (behind.length === 0) break;
    for (const c of behind) c.live.resync();
    while (net.step()) for (const c of clients) expect(c.shadow).toBe(c.live.text);
  }
  for (const [i, c] of clients.entries()) {
    expect(c.live.pending, `client ${i + 1} still has pending changes`).toBe(0);
    expect(c.live.text, `client ${i + 1} text, seed ${seed}`).toBe(server.text);
    expect(c.live.version).toBe(server.version);
    for (const [op, st] of c.ops) expect(st, `client ${i + 1} op ${op}`).not.toBe("pending");
  }
  const typed = clients.reduce((n, c) => n + c.ops.size, 0);
  const rejected = clients.reduce((n, c) => n + [...c.ops.values()].filter((s) => s === "rejected").length, 0);
  return { typed, rejected, refused: server.refused };
}

describe("three clients converge", () => {
  it("over 1,200 random runs, and nothing typed is lost", () => {
    let typed = 0;
    let rejected = 0;
    let refused = 0;
    const runs = Number(process.env.LIVE_RUNS ?? 1200);
    for (let seed = 1; seed <= runs; seed++) {
      const s = run(seed);
      typed += s.typed;
      rejected += s.rejected;
      refused += s.refused;
    }
    // competing text (rule 4 (b)) does happen in this mix of replacements at high latency;
    // P2 (no bounce without competing text) is checked below on insert-only and delete-only work
    expect(typed).toBeGreaterThan(runs * 10);
    expect(rejected).toBeGreaterThan(0);
    console.log(`runs ${runs}: ${typed} edits typed, ${rejected} rejected (${refused} refused by the server)`);
    runrec("client_converge", { runs, typed, rejected, refused });
  });

  it("P2: typing anywhere, or deleting anything, never bounces", () => {
    let typed = 0;
    let runs = 0;
    let rejected = 0;
    let refused = 0;
    for (let seed = 20000; seed < 20400; seed++) {
      for (const only of ["insert", "delete"] as const) {
        const s = run(seed, { only, maxLatency: 60 });
        expect(s.rejected, `${only}-only, seed ${seed}`).toBe(0);
        expect(s.refused, `${only}-only, seed ${seed}`).toBe(0);
        typed += s.typed;
        rejected += s.rejected;
        refused += s.refused;
        runs++;
      }
    }
    expect(typed).toBeGreaterThan(4000);
    runrec("client_p2", { runs, typed, rejected, refused });
  });

  it("when host frames are lost or overtake each other (gaps filled with since; resync settles)", () => {
    const t = { runs: 0, typed: 0, rejected: 0, refused: 0 };
    for (let seed = 30000; seed < 30300; seed++) {
      const s = run(seed, { steps: 30, maxLatency: 60, lossy: 0.08 });
      t.runs++;
      t.typed += s.typed;
      t.rejected += s.rejected;
      t.refused += s.refused;
    }
    runrec("client_lossy", t);
  });

  it("with slow links (many changes since each base)", () => {
    const t = { runs: 0, typed: 0, rejected: 0, refused: 0 };
    for (let seed = 5000; seed < 5100; seed++) {
      const s = run(seed, { steps: 40, maxLatency: 200 });
      t.runs++;
      t.typed += s.typed;
      t.rejected += s.rejected;
      t.refused += s.refused;
    }
    runrec("client_slow_links", t);
  });

  it("when bases expire (MPC-E112: reload, pending changes rejected, then converge)", () => {
    const t = { runs: 0, typed: 0, rejected: 0, refused: 0 };
    for (let seed = 9000; seed < 9100; seed++) {
      const s = run(seed, { steps: 30, maxLatency: 100, window: 2 });
      t.runs++;
      t.typed += s.typed;
      t.rejected += s.rejected;
      t.refused += s.refused;
    }
    runrec("client_bases_expire", t);
  });
});

describe("clarification r2.1: the same multi-author word", () => {
  it("two people typing inside one word never bounce", () => {
    const net = new Net();
    // "hello" written by two people: the window holds authored hunks, never widened to words
    const server = new FakeServer(net, "");
    server.serverEdit([{ start: 0, end: 0, insert: "hel" }]);
    server.serverEdit([{ start: 3, end: 3, insert: "lo world" }]);
    const a = new LiveText(server.snapshot(), server.connect(1, () => 5));
    const b = new LiveText(server.snapshot(), server.connect(2, () => 5));
    const rejected: Rejection[] = [];
    a.onRejected((x) => rejected.push(x));
    b.onRejected((x) => rejected.push(x));
    // concurrent keystrokes in "hello": at 2 and 3, then both at 4
    a.applyLocal([{ start: 2, end: 2, insert: "X" }]);
    b.applyLocal([{ start: 3, end: 3, insert: "Y" }]);
    a.applyLocal([{ start: 5, end: 5, insert: "1" }]);
    b.applyLocal([{ start: 5, end: 5, insert: "2" }]);
    // and a backspace each on the same character
    a.applyLocal([{ start: 0, end: 1, insert: "" }]);
    b.applyLocal([{ start: 0, end: 1, insert: "" }]);
    while (net.step());
    expect(rejected).toEqual([]);
    expect(server.refused).toBe(0);
    expect(a.text).toBe(server.text);
    expect(b.text).toBe(server.text);
    expect(server.text.startsWith("e")).toBe(true);
    for (const ch of ["X", "Y", "1", "2"]) expect(server.text).toContain(ch);
  });
});
