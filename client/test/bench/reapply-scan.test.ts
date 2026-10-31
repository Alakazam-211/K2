// @vitest-environment jsdom
// M2 measurement (docs/design/live-editor-m2.md, topic 1): what Re-apply through the server's
// rebase rule would do with every rejection of the mounted-editor convergence runs, beside what
// M1 does. It changes nothing: it watches M1's Re-apply (the harness's `reapplyAll`) and, just
// before each call, works out the M2 answer from the replica's own record:
//
// - the edit as sent (its hunks and base, taken when it went out);
// - presence first, exactly as M1 (`landed`, `already-present`, `cant-tell` stay as they are);
// - then `rebaseOurs(sent, history since base)`: `placed`, `conflict`, or `no-reach` (the
//   history doesn't go back to the base) or `no-sent-form` (never sent: a queued edit);
// - an oracle: the same rebase over the fake server's own log must give the same hunks.
//
// Per host it prints a summary (`SCAN2 SUMMARY`), the bar the guards and M2 are held to:
// - Re-apply's results: applied, already-present, and refused by reason;
// - every applied Re-apply against `rebaseOurs` from the sent base: the same text, a different
//   text, or applied where the rule refuses (both of the last two must be 0: the run fails);
// - the changes Re-apply refused that the rule would place (non-empty), by reason: what M2 must
//   give back, with 0 differences.
// Before the single-hunk guards (v0.9.8.2) M1 applies where the rule refuses, so the run fails
// there by design.
//
// Skipped unless `M2_REAPPLY_SCAN=from-to` is set:
//   M2_REAPPLY_SCAN=1-300 npx vitest run test/bench/reapply-scan.test.ts
// Every line it prints starts with `SCAN2`.

import { describe, expect, it, vi } from "vitest";
import "../jsdom-shims";
import { mountLiveEditor } from "../../src/editor/mount";
import { apply, validate, type Hunk } from "../../src/hunk";
import { LiveText, type Rejection } from "../../src/live";
import { Carried, rebaseOurs } from "../../src/rebase";
import type { LiveTransport } from "../../src/wire";
import { run, type RunOptions } from "../editor-harness";
import { FakeServer } from "../fake-server";

const range = process.env.M2_REAPPLY_SCAN;

type Sent = { hunks: Hunk[]; base: number };
interface Internals {
  _rejected: Rejection[];
  history: Array<{ version: number; hunks: Hunk[]; client: number | undefined }>;
  historyFrom: number;
  serverText: { toString(): string };
  version: number;
  text: string;
  inflight: { hunks: Hunk[]; base?: number; op?: number; doomed?: Rejection } | null;
  queue: Array<{ hunks: Hunk[] }>;
  presence(id: number): "present" | "absent" | "unknown";
}

const key = (hs: readonly Hunk[]) => JSON.stringify(hs.map((h) => [h.start, h.end, h.insert]));

describe.skipIf(!range)("M2 Re-apply scan", () => {
  it("classifies every Re-apply of the convergence runs", async () => {
    const [from, to] = range!.split("-").map(Number);

    // the edit as sent, per entry; the entry behind each rejection
    const sentOf = new WeakMap<object, Sent>();
    const metaOf = new WeakMap<Rejection, { sent: Sent | null }>();
    const P = LiveText.prototype as unknown as Record<string, (...a: unknown[]) => unknown>;
    const origFlush = P.flush;
    vi.spyOn(P, "flush").mockImplementation(function (this: Internals) {
      origFlush.call(this);
      const e = this.inflight;
      if (e && e.op !== undefined && e.base !== undefined && !sentOf.has(e)) sentOf.set(e, { hunks: e.hunks.map((h) => ({ ...h })), base: e.base });
    } as never);
    const origRejection = P.rejection;
    vi.spyOn(P, "rejection").mockImplementation(function (this: Internals, e: unknown, err: unknown) {
      const r = origRejection.call(this, e, err) as Rejection;
      metaOf.set(r, { sent: sentOf.get(e as object) ?? null });
      return r;
    } as never);
    // the server of the current run (for the oracle)
    let server: FakeServer | null = null;
    const origConnect = FakeServer.prototype.connect;
    vi.spyOn(FakeServer.prototype, "connect").mockImplementation(function (this: FakeServer, ...a: Parameters<FakeServer["connect"]>) {
      server = this;
      return origConnect.apply(this, a);
    } as never);

    // the editor's prototype, from a throwaway mount
    const stub = {
      send() {},
      onFrame: () => () => {},
      onClose: () => () => {},
      close() {},
      connect: () => new Promise(() => {}),
    } as unknown as LiveTransport;
    const el = document.createElement("div");
    document.body.append(el);
    const throwaway = mountLiveEditor(el, { transport: stub });
    const proto = Object.getPrototypeOf(throwaway) as { reapply(id: number): { status: string; reason?: string } };
    throwaway.destroy();
    el.remove();

    type Row = Record<string, number>;
    const tally = new Map<string, Row>();
    const bump = (k: string, col: string) => {
      const row = tally.get(k) ?? {};
      row[col] = (row[col] ?? 0) + 1;
      tally.set(k, row);
    };
    type Summary = { results: Record<string, number>; same: number; differ: number; ruleRefuses: number; rulePlaces: Record<string, number> };
    const summary = new Map<string, Summary>();
    let oracleAgree = 0;
    let oracleDiffer = 0;
    let fitFail = 0;
    const multiPlacedSeeds = new Set<string>();
    const multiE000PlacedSeeds = new Set<string>();
    const firstHunkDiffersSeeds = new Set<string>();
    let seed = 0;
    let config = "";

    const origReapply = proto.reapply;
    vi.spyOn(proto, "reapply").mockImplementation(function (this: { live: Internals; landedIds: Set<number> }, id: number) {
      const live = this.live;
      const rej = live._rejected.find((r) => r.id === id);
      const textBefore = live.text;
      let m2Hunks: Hunk[] | null = null;
      let m2: string;
      let shape = "gone";
      if (this.landedIds.has(id) || !rej) {
        m2 = "already-present";
      } else {
        const multi = rej.hunks.length > 1;
        const err = rej.error?.id ?? "none";
        const lost = (rej as Rejection & { mayHaveApplied?: boolean }).mayHaveApplied;
        shape = `${multi ? "multi " : "single"} ${err}${lost ? "(lost)" : ""}`;
        m2 = "";
        if (lost) {
          const p = live.presence(id);
          if (p === "present") m2 = "already-present";
          else if (p === "unknown") m2 = "cant-tell";
        }
        if (!m2) {
          const sent = metaOf.get(rej)?.sent ?? null;
          if (!sent) m2 = "no-sent-form";
          else if (sent.base < live.historyFrom) m2 = "no-reach";
          else {
            const since = live.history.filter((h) => h.version > sent.base);
            const contiguous = since.every((h, i) => h.version === sent.base + 1 + i) && (since.length === 0 ? sent.base === live.version : since[since.length - 1].version === live.version);
            if (!contiguous) m2 = "no-reach(gap)";
            else {
              const r = rebaseOurs(sent.hunks, since.map((h) => h.hunks));
              if ("conflict" in r) m2 = "conflict";
              else {
                // the oracle: the server's own log since the base
                if (server) {
                  const log = server.log.filter((a) => a.version > sent.base && a.version <= live.version).map((a) => a.hunks);
                  const r2 = rebaseOurs(sent.hunks, log);
                  if ("ok" in r2 && key(r2.ok) === key(r.ok)) oracleAgree++;
                  else oracleDiffer++;
                }
                if (validate(live.serverText.toString(), r.ok) !== null) fitFail++;
                // through the replica's own pending changes (ours after, as the server will)
                let c = Carried.of(r.ok);
                let blocked = false;
                const chain = [...(live.inflight && !live.inflight.doomed ? [live.inflight] : []), ...live.queue];
                for (const e of chain) {
                  const t = c.through(e.hunks, 0);
                  if ("conflict" in t) {
                    blocked = true;
                    break;
                  }
                  c = t.ok;
                }
                if (blocked) m2 = "conflict(pending)";
                else {
                  const hs = c.hunks();
                  m2Hunks = hs;
                  m2 = hs.length === 0 ? "placed(empty)" : chain.length ? "placed(pending)" : "placed";
                  if (multi) multiPlacedSeeds.add(`${config}:${seed}`);
                  if (multi && lost) multiE000PlacedSeeds.add(`${config}:${seed}`);
                  // M1's planted "first hunk place": all the typed text at the region's start
                  if (multi && chain.length === 0) {
                    const m2Text = apply(live.serverText.toString(), hs);
                    const first = rej.text ? apply(live.text, [{ start: rej.region.start, end: rej.region.start, insert: rej.text }]) : live.text;
                    if (first !== m2Text) firstHunkDiffersSeeds.add(`${config}:${seed}`);
                  }
                  // single-hunk: does M2's text differ from M1's (text put back at the region)?
                  if (!multi && chain.length === 0) {
                    const m2Text = apply(live.serverText.toString(), hs);
                    const m1Text = rej.text ? apply(live.text, [{ start: rej.region.start, end: rej.region.start, insert: rej.text }]) : live.text;
                    if (m1Text !== m2Text) bump(`${config} single text differs`, rej.hunks[0].start < rej.hunks[0].end ? (rej.text ? "replacement" : "deletion") : "insertion");
                  }
                }
              }
            }
          }
        }
      }
      const res = origReapply.call(this as never, id);
      const sum = summary.get(config) ?? { results: {}, same: 0, differ: 0, ruleRefuses: 0, rulePlaces: {} };
      summary.set(config, sum);
      const key2 = res.reason ? `${res.status}/${res.reason}` : res.status;
      sum.results[key2] = (sum.results[key2] ?? 0) + 1;
      if (res.status === "applied") {
        if (m2Hunks === null) {
          sum.ruleRefuses++;
          console.log(`SCAN2 APPLIED-BUT-RULE-REFUSES ${config} seed ${seed}: ${shape}, rule ${m2}`);
        } else if (live.text === apply(textBefore, m2Hunks)) sum.same++;
        else {
          sum.differ++;
          console.log(`SCAN2 APPLIED-DIFFERS ${config} seed ${seed}: ${shape}`);
        }
      } else if (res.status === "unknown" && m2Hunks !== null && m2Hunks.length > 0) {
        const k = `${res.reason ?? "?"}${rej && rej.hunks.length === 1 && rej.hunks[0].start === rej.hunks[0].end ? " (insertion)" : ""}`;
        sum.rulePlaces[k] = (sum.rulePlaces[k] ?? 0) + 1;
      }
      bump(`${config} ${shape}`, `M1 ${res.status}${res.reason ? `/${res.reason}` : ""} -> M2 ${m2}`);
      bump(`${config} ALL`, `M2 ${m2}`);
      return res;
    } as never);

    const configs: Array<[string, RunOptions]> = [
      ["strict", { reapplyAll: true }],
      ["strict-slow", { steps: 50, drops: 6, maxLatency: 120, reapplyAll: true }],
      ["no-rule-slow", { steps: 50, drops: 6, maxLatency: 120, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true }],
    ];
    let failed = 0;
    for (const [name, opts] of configs) {
      config = name;
      for (seed = from; seed <= to; seed++) {
        try {
          await run(seed, opts);
        } catch (e) {
          failed++;
          console.log(`SCAN2 ${name} seed ${seed} failed: ${String((e as Error).message).split("\n")[0].slice(0, 160)}`);
        }
      }
    }
    for (const [k, row] of [...tally.entries()].sort()) {
      const cols = Object.entries(row)
        .sort((a, b) => b[1] - a[1])
        .map(([c, n]) => `${c}: ${n}`)
        .join(" | ");
      console.log(`SCAN2 ${k} :: ${cols}`);
    }
    const fmt = (o: Record<string, number>) =>
      Object.entries(o)
        .sort((a, b) => b[1] - a[1])
        .map(([k, n]) => `${k} ${n}`)
        .join(", ") || "none";
    let badApplied = 0;
    for (const [host, sum] of summary) {
      const placesTotal = Object.values(sum.rulePlaces).reduce((a, b) => a + b, 0);
      console.log(`SCAN2 SUMMARY ${host}: results: ${fmt(sum.results)}`);
      console.log(`SCAN2 SUMMARY ${host}: applied vs rebaseOurs from the sent base: same ${sum.same}, different ${sum.differ}, applied where the rule refuses ${sum.ruleRefuses}`);
      console.log(`SCAN2 SUMMARY ${host}: refused that the rule would place: ${placesTotal} (${fmt(sum.rulePlaces)})`);
      badApplied += sum.differ + sum.ruleRefuses;
    }
    console.log(`SCAN2 oracle (client history vs server log): agree ${oracleAgree}, differ ${oracleDiffer}; placed hunks not fitting the server text: ${fitFail}`);
    console.log(`SCAN2 seeds with a multi-hunk rejection placed: ${[...multiPlacedSeeds].slice(0, 40).join(",")}`);
    console.log(`SCAN2 seeds with a multi-hunk lost (E000) rejection placed: ${[...multiE000PlacedSeeds].slice(0, 40).join(",")}`);
    console.log(`SCAN2 seeds where "first hunk place" differs from M2: ${[...firstHunkDiffersSeeds].slice(0, 40).join(",")}`);
    console.log(`SCAN2 runs failed: ${failed} (seeds ${from}-${to}, ${configs.length} configs)`);
    expect(oracleDiffer).toBe(0);
    expect(badApplied, "an applied Re-apply differs from the server's rule").toBe(0);
  }, 3_600_000);
});
