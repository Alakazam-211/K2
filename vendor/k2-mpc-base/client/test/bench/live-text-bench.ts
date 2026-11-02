// M2 measurement (docs/design/live-editor-m2.md, topic 2): what `LiveText` costs per edit as the
// document grows. Not a test, and not product code: the suite never collects it (its name has no
// `.test.`), and it does nothing unless `M2_BENCH` is set. Run it on a build node, alone:
//
//   M2_BENCH=1 npx esbuild test/bench/live-text-bench.ts --bundle --platform=node --format=esm \
//     --outfile=bench-out/live-text-bench.mjs && M2_BENCH=1 node --expose-gc bench-out/live-text-bench.mjs
//
// `M2_BENCH_QUICK=1` runs the small sizes only. Every line it prints starts with `BENCH`.
//
// `M2_BENCH_GATE=1` runs only the M2 targets and exits 1 if one is missed (the gate runs it,
// pinned to one core): p95 per acknowledged keystroke under 1 ms at 1 MB and under 2 ms at 4 MB
// (ASCII and CJK/emoji, typing at the end, the middle and the start), and a `since` reply of 500
// events under 100 ms at 1 MB. (The editor binding's byte-index lines left with the editor.)
// Every gate line carries the node's load; a missed target is re-run once and fails only if the
// re-run misses too (`BENCH RETRY` marks a pass on the re-run). Limits never change with load.
// Before each attempt the gate waits for a quiet machine (`BENCH LOADED`): when other work used
// more than half the core the bench is pinned to (`M2_BENCH_CORE_BUSY_MAX`, default 0.5, sampled
// from /proc/stat), or the 1-minute load is above half the machine's CPUs (`M2_BENCH_LOAD_MAX`),
// it re-checks every 15 s for up to `M2_BENCH_LOADED_WAIT_S` (default 300) seconds. A target that
// never got a quiet machine ends `LOADED`, not as a miss: the run then prints `BENCH GATE LOADED`
// and exits 3 (a miss exits 1 and wins over loaded).
//
// What it measures, for documents of 10 KB to 4 MB (UTF-8), ASCII and CJK/emoji:
// - local typing at the end, the middle and the start, each keystroke acknowledged before the
//   next (applyLocal + setCursor, then the ack), and typing while one edit is in flight (the
//   rest compose in the queue);
// - a burst of remote changes, with and without a local edit pending;
// - a `since` reply of 500 events;
// - the primitives each of those calls (where the time goes), a CPU profile of the 1 MB runs,
//   and GC count and time per scenario;
// - a bench-only model of a fix (a flat string with a chunked UTF-8 index), as a floor, not a
//   design.

import { Session } from "node:inspector";
import { readFileSync } from "node:fs";
import os from "node:os";
import { PerformanceObserver } from "node:perf_hooks";
import { apply, byteToIndex, compose, utf8Len, validate, type Hunk } from "../../src/hunk";
import { LiveText } from "../../src/live";
import { Rope } from "../../src/rope";
import type { ChangeEvent, ClientFrame, ServerFrame } from "../../src/wire";

const QUICK = !!process.env.M2_BENCH_QUICK;
const SIZES: Array<[string, number]> = QUICK
  ? [
      ["10KB", 10_000],
      ["100KB", 100_000],
    ]
  : [
      ["10KB", 10_000],
      ["100KB", 100_000],
      ["1MB", 1_000_000],
      ["4MB", 4_000_000],
    ];
const KINDS = ["ascii", "cjk"] as const;
type Kind = (typeof KINDS)[number];

// ---- helpers

const now = () => Number(process.hrtime.bigint()) / 1e6; // ms

function rng(seed: number) {
  let s = seed >>> 0;
  return () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

const WORDS = ["lorem", "ipsum", "dolor", "sit", "amet", "consectetur", "adipiscing", "elit", "sed", "do", "eiusmod", "tempor"];
const CJK = ["日本語", "编辑器", "協作", "文字列", "🙂", "👩‍💻", "テキスト", "한국어", "汉字", "🚀", "共同編集", "表情🎉"];

/** A document of about `bytes` UTF-8 bytes: lines of about 70 bytes. */
function makeDoc(bytes: number, kind: Kind, seed = 1): string {
  const r = rng(seed);
  const out: string[] = [];
  let n = 0;
  let line = 0;
  while (n < bytes) {
    const w = kind === "ascii" || r() < 0.2 ? WORDS[Math.floor(r() * WORDS.length)] : CJK[Math.floor(r() * CJK.length)];
    const sep = line > 70 ? "\n" : " ";
    line = sep === "\n" ? 0 : line + utf8Len(w) + 1;
    out.push(w, sep);
    n += utf8Len(w) + 1;
  }
  return out.join("");
}

/** A UTF-8 offset on a character boundary near the middle of `s`. */
function midByte(s: string): number {
  let i = s.length >> 1;
  const c = s.charCodeAt(i);
  if (c >= 0xdc00 && c < 0xe000) i--;
  return utf8Len(s.slice(0, i));
}

function lineStarts(s: string): number[] {
  const out = [0];
  let b = 0;
  for (let i = 0; i < s.length; i++) {
    const c = s.charCodeAt(i);
    if (c < 0x80) b += 1;
    else if (c < 0x800) b += 2;
    else if (c >= 0xd800 && c < 0xdc00) {
      b += 4;
      i++;
    } else b += 3;
    if (c === 10) out.push(b);
  }
  return out;
}

function stats(xs: number[]) {
  const s = [...xs].sort((a, b) => a - b);
  const q = (p: number) => s[Math.min(s.length - 1, Math.floor(p * s.length))];
  const mean = s.reduce((a, b) => a + b, 0) / s.length;
  return { p50: q(0.5), p95: q(0.95), max: s[s.length - 1], mean, n: s.length };
}
const f3 = (x: number) => (x < 10 ? x.toFixed(3) : x.toFixed(1));
const fmt = (xs: number[]) => {
  const s = stats(xs);
  return `p50 ${f3(s.p50)} p95 ${f3(s.p95)} max ${f3(s.max)} ms`;
};

/** A transport that records frames and lets the bench play the server. */
function wire() {
  const sent: ClientFrame[] = [];
  let cb: ((f: ServerFrame) => void) | null = null;
  return {
    t: {
      send: (f: ClientFrame) => void sent.push(f),
      onFrame: (c: (f: ServerFrame) => void) => {
        cb = c;
        return () => {
          cb = null;
        };
      },
    },
    sent,
    deliver: (f: ServerFrame) => cb!(f),
    lastEdit: () => {
      for (let i = sent.length - 1; i >= 0; i--) {
        const f = sent[i];
        if (f.type === "edit") return f;
      }
      throw new Error("no edit sent");
    },
  };
}

const ev = (version: number, hunks: Hunk[]): ChangeEvent => ({
  version,
  pid: "p2",
  client: 2,
  via: "live",
  root: "source",
  hunks,
  at_ms: 0,
});

// ---- GC accounting

const gcs: number[] = [];
const obs = new PerformanceObserver((list) => {
  for (const e of list.getEntries()) gcs.push(e.duration);
});
obs.observe({ entryTypes: ["gc"] });
const tick = () => new Promise((r) => setImmediate(r));
const gc = () => (globalThis as { gc?: () => void }).gc?.();

async function measured<T>(f: () => T): Promise<{ v: T; gc: string; heapMB: number }> {
  gc();
  await tick();
  gcs.length = 0;
  const h0 = process.memoryUsage().heapUsed;
  const v = f();
  const h1 = process.memoryUsage().heapUsed;
  await tick();
  const n = gcs.length;
  const ms = gcs.reduce((a, b) => a + b, 0);
  return { v, gc: `gc ${n} (${ms.toFixed(1)} ms)`, heapMB: (h1 - h0) / 1e6 };
}

// ---- scenarios

/** Typing one character at a time at `where`; `acked`: each keystroke acknowledged before the
 * next, else everything after the first composes in the queue behind it. */
function typing(doc: string, where: "end" | "middle" | "start", n: number, acked: boolean) {
  const w = wire();
  const l = new LiveText({ text: doc, version: 1 }, w.t, { client: 1 });
  let p = where === "start" ? 0 : where === "end" ? utf8Len(doc) : midByte(doc);
  let v = 1;
  const local: number[] = [];
  const ack: number[] = [];
  const total: number[] = [];
  const warm = Math.min(30, n);
  for (let i = 0; i < warm + n; i++) {
    const t0 = now();
    l.applyLocal([{ start: p, end: p, insert: "x" }], { group: "input-1" });
    l.setCursor(p + 1, p + 1);
    const t1 = now();
    if (acked) {
      const f = w.lastEdit();
      if (f.type !== "edit") throw new Error("not an edit");
      w.deliver({ type: "ack", op: f.op, version: ++v, hunks: f.hunks });
    }
    const t2 = now();
    p += 1;
    if (i >= warm) {
      local.push(t1 - t0);
      ack.push(t2 - t1);
      total.push(t2 - t0);
    }
  }
  let settle = 0;
  if (!acked) {
    // the first edit's ack, then the composed rest's
    const t0 = now();
    for (let k = 0; k < 2 && l.pending > 0; k++) {
      const f = w.lastEdit();
      if (f.type !== "edit") throw new Error("not an edit");
      w.deliver({ type: "ack", op: f.op, version: ++v, hunks: f.hunks });
    }
    settle = now() - t0;
    if (l.pending !== 0) throw new Error("queue not settled");
  }
  if (l.text.length !== doc.length + warm + n) throw new Error("typing lost text");
  return { local, ack, total, settle };
}

/** `n` remote one-character insertions at random line starts; `pending`: with a local edit in
 * flight (never acknowledged) that every one is carried through. */
function remoteBurst(doc: string, n: number, pending: boolean) {
  const w = wire();
  const l = new LiveText({ text: doc, version: 1 }, w.t, { client: 1 });
  const mid = midByte(doc);
  l.setCursor(mid, mid);
  if (pending) l.applyLocal([{ start: mid, end: mid, insert: "L" }]);
  const starts = lineStarts(doc);
  const r = rng(7);
  const times: number[] = [];
  let v = 1;
  const warm = 20;
  for (let i = 0; i < warm + n; i++) {
    const k = Math.floor(r() * starts.length);
    const at = starts[k];
    for (let j = k + 1; j < starts.length; j++) starts[j] += 1;
    const t0 = now();
    w.deliver({ type: "change", event: ev(++v, [{ start: at, end: at, insert: "y" }]) });
    const t1 = now();
    if (i >= warm) times.push(t1 - t0);
  }
  if (l.version !== v) throw new Error(`burst: at ${l.version}, expected ${v}`);
  return times;
}

/** A `since` reply of `n` events (each a one-character insertion at a line start). `around`:
 * runs the timed delivery (a profiler wraps it). */
function sinceReplay(doc: string, n: number, around: (f: () => void) => void = (f) => f()): number {
  const w = wire();
  const l = new LiveText({ text: doc, version: 1 }, w.t, { client: 1 });
  const mid = midByte(doc);
  l.setCursor(mid, mid);
  w.deliver({ type: "change", event: ev(3, [{ start: 0, end: 0, insert: "g" }]) }); // a gap: since
  const since = w.sent.find((f) => f.type === "since");
  if (!since || since.type !== "since") throw new Error("no since sent");
  const starts = lineStarts(doc);
  const r = rng(11);
  const events: ChangeEvent[] = [];
  for (let i = 0; i < n; i++) {
    const k = Math.floor(r() * starts.length);
    const at = starts[k];
    for (let j = k + 1; j < starts.length; j++) starts[j] += 1;
    events.push(ev(2 + i, [{ start: at, end: at, insert: "y" }]));
  }
  let ms = 0;
  around(() => {
    const t0 = now();
    w.deliver({ type: "events", op: since.op, events });
    ms = now() - t0;
    if (l.version !== 1 + n) throw new Error(`since: at ${l.version}`);
  });
  return ms;
}

// ---- primitives

function prim(name: string, reps: number, f: () => unknown): string {
  for (let i = 0; i < 3; i++) f();
  const xs: number[] = [];
  for (let i = 0; i < reps; i++) {
    const t0 = now();
    f();
    xs.push(now() - t0);
  }
  return `${name} ${f3(stats(xs).p50)}`;
}

function primitives(doc: string, reps: number): string[] {
  const mid = midByte(doc);
  const h: Hunk[] = [{ start: mid, end: mid, insert: "x" }];
  const copy = () => (doc.slice(0, 5) + doc.slice(5)).concat(""); // equal text, another string
  const other = copy();
  other.charCodeAt(0); // flat
  const out: string[] = [];
  out.push(prim("utf8Len", reps, () => utf8Len(doc)));
  // diagnostics: the same scan on a string known to be flat, and native counts
  const flat = Buffer.from(doc, "utf8").toString("utf8");
  out.push(prim("utf8Len(flat copy)", reps, () => utf8Len(flat)));
  out.push(prim("Buffer.byteLength", reps, () => Buffer.byteLength(doc, "utf8")));
  const enc = new TextEncoder();
  const buf = new Uint8Array(doc.length * 3 + 8);
  out.push(prim("TextEncoder.encodeInto", reps, () => enc.encodeInto(doc, buf).written));
  out.push(prim("byteToIndex(mid)", reps, () => byteToIndex(doc, mid)));
  out.push(prim("validate(1 hunk mid)", reps, () => validate(doc, h)));
  out.push(prim("apply(1 hunk mid)", reps, () => apply(doc, h)));
  out.push(
    prim("flatten(after apply)", reps, () => {
      const s = doc.slice(0, doc.length >> 1) + "x" + doc.slice(doc.length >> 1);
      return s.charCodeAt(1);
    }),
  );
  out.push(prim("equal(two copies)", reps, () => doc === other));
  out.push(prim("compose(1+1 mid)", reps, () => compose(doc, h, [{ start: mid + 1, end: mid + 1, insert: "y" }])));
  return out;
}

/** Bench-only model of "a flat string with a chunked UTF-8 index": a keystroke is the string
 * splice (flattened by the next read), one chunk re-measured, and the later chunks' byte starts
 * shifted. */
function chunkModel(doc: string, n: number, chunk = 2048): number[] {
  let s = doc;
  const starts: number[] = []; // UTF-8 start of each chunk of `chunk` UTF-16 units
  for (let i = 0, b = 0; i < s.length; i += chunk) {
    starts.push(b);
    b += utf8Len(s.slice(i, i + chunk));
  }
  let at = s.length >> 1;
  const xs: number[] = [];
  for (let i = 0; i < 30 + n; i++) {
    const t0 = now();
    s = s.slice(0, at) + "x" + s.slice(at);
    const c = Math.floor(at / chunk);
    utf8Len(s.slice(c * chunk, (c + 1) * chunk)); // re-measure the chunk (and flatten s)
    for (let k = c + 1; k < starts.length; k++) starts[k] += 1;
    const t1 = now();
    at += 1;
    if (i >= 30) xs.push(t1 - t0);
  }
  return xs;
}

// ---- the CPU profile

function post<T>(s: Session, method: string, params?: object): Promise<T> {
  const call = s.post as unknown as (m: string, p: object, cb: (e: Error | null, r: unknown) => void) => void;
  return new Promise((res, rej) => call.call(s, method, params ?? {}, (e, r) => (e ? rej(e) : res(r as T))));
}

interface Profile {
  nodes: Array<{ id: number; callFrame: { functionName: string; url: string; lineNumber: number } }>;
  samples: number[];
  timeDeltas: number[];
}

async function profiled(label: string, f: () => void): Promise<void> {
  const s = new Session();
  s.connect();
  await post(s, "Profiler.enable");
  await post(s, "Profiler.setSamplingInterval", { interval: 100 });
  await post(s, "Profiler.start");
  f();
  const { profile } = await post<{ profile: Profile }>(s, "Profiler.stop");
  s.disconnect();
  const name = new Map<number, string>();
  for (const n of profile.nodes) name.set(n.id, n.callFrame.functionName || `(${n.callFrame.url ? "anonymous" : "native"})`);
  const self = new Map<string, number>();
  let total = 0;
  profile.samples.forEach((id, i) => {
    const dt = profile.timeDeltas[i] ?? 0;
    const k = name.get(id) ?? "?";
    self.set(k, (self.get(k) ?? 0) + dt);
    total += dt;
  });
  const top = [...self.entries()]
    .filter(([k]) => k !== "(idle)" && k !== "(program)")
    .sort((a, b) => b[1] - a[1])
    .slice(0, 10)
    .map(([k, v]) => `${k} ${((100 * v) / total).toFixed(0)}%`);
  console.log(`BENCH profile ${label}: ${top.join(", ")}`);
}

// ---- main

async function main() {
  console.log(
    `BENCH env: node ${process.version}, ${os.cpus().length} CPUs (${os.cpus()[0]?.model ?? "?"}), ${os.platform()} ${os.arch()}, gc ${typeof (globalThis as { gc?: unknown }).gc === "function" ? "exposed" : "not exposed"}`,
  );
  // warm the JIT on a small document first
  typing(makeDoc(10_000, "ascii"), "middle", 200, true);
  remoteBurst(makeDoc(10_000, "cjk"), 200, true);

  for (const [label, bytes] of SIZES) {
    for (const kind of KINDS) {
      const doc = makeDoc(bytes, kind);
      const tag = `${label} ${kind} (${utf8Len(doc)} B, ${doc.length} units)`;
      const big = bytes >= 1_000_000;
      const keys = big ? 150 : 300;
      for (const where of ["end", "middle", "start"] as const) {
        const m = await measured(() => typing(doc, where, keys, true));
        console.log(
          `BENCH typing-acked ${tag} ${where}: total ${fmt(m.v.total)} | applyLocal ${fmt(m.v.local)} | ack ${fmt(m.v.ack)} | ${m.gc}`,
        );
      }
      {
        const m = await measured(() => typing(doc, "middle", keys, false));
        console.log(`BENCH typing-queued ${tag} middle: applyLocal ${fmt(m.v.local)} | settle ${f3(m.v.settle)} ms | ${m.gc}`);
      }
      for (const pending of [false, true]) {
        const m = await measured(() => remoteBurst(doc, big ? 150 : 300, pending));
        console.log(`BENCH remote-burst ${tag}${pending ? " +pending" : ""}: per change ${fmt(m.v)} | ${m.gc}`);
      }
      {
        const reps = big ? 2 : 4;
        const xs: number[] = [];
        let g = "";
        for (let i = 0; i < reps; i++) {
          const m = await measured(() => sinceReplay(doc, 500));
          xs.push(m.v);
          g = m.gc;
        }
        const s = stats(xs);
        console.log(`BENCH since-500 ${tag}: total p50 ${f3(s.p50)} ms (${f3(s.p50 / 500)} ms/event), max ${f3(s.max)} ms | last ${g}`);
      }
      console.log(`BENCH prims ${tag}: ${primitives(doc, big ? 20 : 100).join(" | ")}`);
      {
        const ch = await measured(() => chunkModel(doc, keys));
        console.log(`BENCH model-chunked ${tag}: ${fmt(ch.v)} | ${ch.gc}`);
      }
      if (bytes === 1_000_000) {
        await profiled(`typing-acked ${tag} middle`, () => void typing(doc, "middle", 150, true));
        await profiled(`remote-burst ${tag} +pending`, () => void remoteBurst(doc, 150, true));
      }
    }
  }
  obs.disconnect();
  console.log("BENCH done");
}

/** The node's load (1, 5 and 15 minutes) and CPU count, for every gate line. */
const load = () => `load ${os.loadavg().map((x) => x.toFixed(2)).join("/")} on ${os.cpus().length} CPUs`;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** The one core this process is pinned to (`taskset -c N`), or null when it may run on several. */
function pinnedCore(): number | null {
  try {
    const m = readFileSync("/proc/self/status", "utf8").match(/^Cpus_allowed_list:\s*(\S+)/m);
    return m && /^\d+$/.test(m[1]) ? Number(m[1]) : null;
  } catch {
    return null;
  }
}

/** Busy and total jiffies of one core, from /proc/stat (null where there is none). */
function coreTimes(core: number): { busy: number; total: number } | null {
  try {
    const line = readFileSync("/proc/stat", "utf8").split("\n").find((l) => l.startsWith(`cpu${core} `));
    if (!line) return null;
    const v = line.trim().split(/\s+/).slice(1).map(Number);
    const total = v.reduce((a, b) => a + b, 0);
    return { busy: total - (v[3] ?? 0) - (v[4] ?? 0), total };
  } catch {
    return null;
  }
}

/** The share of `core` other work used over `ms`; this process sleeps meanwhile, so it is all theirs. */
async function coreBusy(core: number, ms: number): Promise<number | null> {
  const a = coreTimes(core);
  await sleep(ms);
  const b = coreTimes(core);
  if (!a || !b || b.total <= a.total) return null;
  return (b.busy - a.busy) / (b.total - a.total);
}

const CORE_BUSY_MAX = Number(process.env.M2_BENCH_CORE_BUSY_MAX ?? 0.5);
const LOAD_MAX = Number(process.env.M2_BENCH_LOAD_MAX ?? os.cpus().length / 2);
const LOADED_WAIT_MS = Number(process.env.M2_BENCH_LOADED_WAIT_S ?? 300) * 1000;

/**
 * Wait for a quiet machine before measuring `what`: the pinned core no more than `CORE_BUSY_MAX`
 * busy with other work, and the 1-minute load no more than `LOAD_MAX`. Logs `BENCH LOADED` on each
 * busy check; true once quiet, false if it stayed busy past `LOADED_WAIT_MS`.
 */
async function quiet(what: string): Promise<boolean> {
  const core = pinnedCore();
  const start = Date.now();
  for (;;) {
    const busy = core === null ? null : await coreBusy(core, 2000);
    const l1 = os.loadavg()[0];
    const coreOk = busy === null || busy <= CORE_BUSY_MAX;
    const loadOk = l1 <= LOAD_MAX;
    if (coreOk && loadOk) return true;
    const why = [coreOk ? null : `cpu${core} ${(busy! * 100).toFixed(0)}% busy (max ${(CORE_BUSY_MAX * 100).toFixed(0)}%)`, loadOk ? null : `1-minute load ${l1.toFixed(2)} (max ${LOAD_MAX})`]
      .filter(Boolean)
      .join(", ");
    const waited = Date.now() - start;
    if (waited >= LOADED_WAIT_MS) {
      console.log(`BENCH LOADED ${what}: still busy after ${(waited / 1000).toFixed(0)} s: ${why} | ${load()}`);
      return false;
    }
    console.log(`BENCH LOADED ${what}: ${why}; waiting | ${load()}`);
    await sleep(Math.min(15_000, Math.max(0, LOADED_WAIT_MS - waited)));
  }
}

/**
 * The M2 targets: each measured, compared and printed with the node's load. A miss is re-run once
 * and fails only if the re-run misses too; a re-run that passes prints `BENCH RETRY`, so a busy
 * node shows in the log instead of hiding. Limits never change with load. Exit 1 on any miss.
 */
async function gate() {
  console.log(`BENCH env: node ${process.version}, ${os.cpus().length} CPUs (${os.cpus()[0]?.model ?? "?"}), ${os.platform()} ${os.arch()}`);
  console.log(`BENCH load at start: ${load()}`);
  console.log(`BENCH quiet rule: pinned core ${pinnedCore() ?? "none"} busy <= ${CORE_BUSY_MAX}, 1-minute load <= ${LOAD_MAX}, waiting up to ${LOADED_WAIT_MS / 1000} s`);
  typing(makeDoc(10_000, "ascii"), "middle", 200, true); // warm the JIT
  remoteBurst(makeDoc(10_000, "cjk"), 200, true);
  const misses: string[] = [];
  const loaded: string[] = [];
  const target = async (what: string, limit: number, run: () => Promise<number>) => {
    const tries: Array<{ got: number; before: string; after: string }> = [];
    for (let attempt = 1; attempt <= 2; attempt++) {
      if (!(await quiet(what))) {
        console.log(`BENCH GATE ${what}${attempt > 1 ? " (re-run)" : ""}: LOADED (not measured)`);
        loaded.push(what);
        return;
      }
      const before = load();
      const got = await run();
      const after = load();
      tries.push({ got, before, after });
      const ok = got < limit;
      console.log(`BENCH GATE ${what}${attempt > 1 ? " (re-run)" : ""}: ${f3(got)} ms (limit ${limit} ms) ${ok ? "PASS" : "FAIL"} | ${before} -> ${after}`);
      if (ok) {
        if (attempt > 1) console.log(`BENCH RETRY ${what}: first ${f3(tries[0].got)} ms (${tries[0].before}) missed; re-run ${f3(got)} ms (${before}) passed`);
        return;
      }
    }
    misses.push(what);
  };
  for (const [label, bytes, limit] of [["1MB", 1_000_000, 1], ["4MB", 4_000_000, 2]] as const) {
    for (const kind of KINDS) {
      const doc = makeDoc(bytes, kind);
      for (const where of ["end", "middle", "start"] as const) {
        await target(`keystroke p95 ${label} ${kind} ${where}`, limit, async () => stats((await measured(() => typing(doc, where, 300, true))).v.total).p95);
      }
      const q = await measured(() => typing(doc, "middle", 300, false));
      console.log(`BENCH info typing-queued ${label} ${kind} middle: applyLocal ${fmt(q.v.local)} | ${q.gc} | ${load()}`);
      const rb = await measured(() => remoteBurst(doc, 300, true));
      console.log(`BENCH info remote-burst ${label} ${kind} +pending: per change ${fmt(rb.v)} | ${rb.gc} | ${load()}`);
      if (bytes === 1_000_000) {
        await target(`since-500 total (worst of 5) ${label} ${kind}`, 100, async () => {
          const xs: number[] = [];
          for (let i = 0; i < 5; i++) xs.push((await measured(() => sinceReplay(doc, 500))).v);
          return stats(xs).max;
        });
      }
    }
  }
  console.log(`BENCH load at end: ${load()}`);
  if (misses.length) console.log(`BENCH GATE FAIL: ${misses.join("; ")}`);
  if (loaded.length) console.log(`BENCH GATE LOADED: ${loaded.join("; ")} (the machine stayed busy; not a miss)`);
  if (!misses.length && !loaded.length) console.log("BENCH GATE PASS");
  obs.disconnect();
  if (misses.length) process.exit(1);
  if (loaded.length) process.exit(3);
}

/** `M2_BENCH_SINCE=1`: the 500-event \`since\` at 1 MB alone: 20 runs (p50, p95, max) and a CPU
 * profile, for working on that path. */
async function sinceOnly() {
  for (const kind of KINDS) {
    const doc = makeDoc(1_000_000, kind);
    sinceReplay(doc, 500); // warm
    const xs: number[] = [];
    for (let i = 0; i < 20; i++) xs.push((await measured(() => sinceReplay(doc, 500))).v);
    console.log(`BENCH since-500 1MB ${kind} x20: ${fmt(xs)} | ${load()}`);
    // the rope's own share: the same 500 insertions applied to a rope alone, and validated
    const starts = lineStarts(doc);
    const r = rng(11);
    const evs: Hunk[][] = [];
    for (let i = 0; i < 500; i++) {
      const k = Math.floor(r() * starts.length);
      const at = starts[k];
      for (let j = k + 1; j < starts.length; j++) starts[j] += 1;
      evs.push([{ start: at, end: at, insert: "y" }]);
    }
    for (let rep = 0; rep < 3; rep++) {
      let rope = Rope.of(doc);
      // (the flat rope before step d had `chunks`, the two-level one has `blocks`)
      const nChunks = (x: Rope) => {
        const y = x as unknown as { blocks?: Array<{ chunks: string[] }>; chunks?: string[] };
        return y.blocks ? y.blocks.reduce((n, b) => n + b.chunks.length, 0) : (y.chunks?.length ?? 0);
      };
      const n0 = nChunks(rope);
      let tv = 0;
      let ta = 0;
      for (const hs of evs) {
        const t0 = now();
        rope.validate(hs);
        const t1 = now();
        rope = rope.apply(hs);
        ta += now() - t1;
        tv += t1 - t0;
      }
      console.log(`BENCH rope alone 1MB ${kind}: 500 validate ${f3(tv)} ms, 500 apply ${f3(ta)} ms (chunks ${n0} -> ${nChunks(rope)})`);
    }
    // where the delivery's time goes: each method's cumulative time (nested calls count in both)
    {
      const spent = new Map<string, number>();
      const undo: Array<() => void> = [];
      const wrap = (proto: Record<string, unknown>, name: string, label: string) => {
        const orig = proto[name] as (...a: unknown[]) => unknown;
        proto[name] = function (this: unknown, ...a: unknown[]) {
          const t0 = now();
          try {
            return orig.apply(this, a);
          } finally {
            spent.set(label, (spent.get(label) ?? 0) + now() - t0);
          }
        };
        undo.push(() => (proto[name] = orig));
      };
      const L = LiveText.prototype as unknown as Record<string, unknown>;
      const R = Rope.prototype as unknown as Record<string, unknown>;
      for (const m of ["onEvents", "onEvent", "recarry", "advance", "checkLanded", "sendCursor", "moveRejected", "emit"]) wrap(L, m, `LiveText.${m}`);
      for (const m of ["apply", "validate", "byteToIndex", "equals"]) wrap(R, m, `Rope.${m}`);
      const total = sinceReplay(doc, 500);
      for (const u of undo) u();
      console.log(`BENCH since-500 1MB ${kind} by method (total ${f3(total)} ms): ${[...spent].sort((x, y) => y[1] - x[1]).map(([k, v]) => `${k} ${f3(v)}`).join(", ")}`);
    }
    // the profile covers the delivery only (not building the document or the events)
    let run: (() => void) | null = null;
    sinceReplay(doc, 500, (f) => {
      run = f;
    });
    await profiled(`since-500 1MB ${kind} (delivery only)`, () => run!());
  }
}

if (process.env.M2_BENCH_SINCE) {
  sinceOnly().catch((e) => {
    console.error("BENCH failed", e);
    process.exit(1);
  });
} else if (process.env.M2_BENCH_GATE) {
  gate().catch((e) => {
    console.error("BENCH failed", e);
    process.exit(1);
  });
} else if (process.env.M2_BENCH) {
  main().catch((e) => {
    console.error("BENCH failed", e);
    process.exit(1);
  });
} else {
  console.log("live-text-bench: set M2_BENCH=1 to run (see the header)");
}
