// Three DOM-free hosts (`live-host.ts`: the host rules over `LiveText`, no editor) through the
// convergence harness: random runs, slow links with more drops, and the fixed seeds of past
// failures. The library half of what a host editor's own convergence tests check.

import { describe, expect, it } from "vitest";
import { run, type RunOptions } from "./live-host";
import { withHunkOnlyRecognition, withoutPresenceCheck, withRegionPlacement } from "./plants";
import { runrec } from "./runrec";

/**
 * Seeds that exercise a planted fault, run every time whatever `LIVE_RUNS` says. Each must
 * pass, and fail with its fault planted (`live-seed-scan.test.ts` finds them):
 * - `hunk-only`: LiveText recognising its own change in a `since` reply by hunks alone takes
 *   someone else's identical change for its own ("diverged: the server applied a change the
 *   client saw conflict"; first seen as seed 51, before the wire contract's client IDs);
 * - `no-presence-check`: on a host without the reconnect rule an edit on a dropped socket lands
 *   after the resync (a false MPC-E000, which the client recognises by its client ID);
 *   without that recognition and Re-apply's check, Re-apply puts the text in twice;
 * - `region-placement`: a lost change with more than one hunk, re-applied: the harness's oracle
 *   (the server's rule over the server's own log) must agree with where Re-apply put it, and
 *   M1's placement (all the text at the region's start) must fail it.
 */
type Fixed = { seed: number; opts?: RunOptions; fault: "hunk-only" | "no-presence-check" | "region-placement" };
const SLOW: RunOptions = { steps: 50, drops: 6, maxLatency: 120 };
const FIXED_SEEDS: Fixed[] = [
  // Picked by the seed scan on these hosts (scratch job 59694b8d, kit 223f6d4a; each holds with
  // kind and without): region-placement 1-200, 42 seeds, the first: 5,7,8,11,12; hunk-only
  // 1-2500: 157,1473,2279; no-rule 1-200, 17 seeds, the first: 23,29,36,39,41; slow no-rule
  // 7000-7150: 7002,7023,7033,7048,7072. The faults and their shapes are the ones the mounted
  // editors' seeds were found for (client 0.11.1 and before); 7, 8 and 7023 carried over.
  // the multi-hunk race: a lost change with two hunks, placed by the server's rule
  { seed: 7, opts: { reapplyAll: true }, fault: "region-placement" },
  { seed: 8, opts: { reapplyAll: true }, fault: "region-placement" },
  // LiveText taking someone else's identical change for its own
  { seed: 157, fault: "hunk-only" },
  { seed: 1473, fault: "hunk-only" },
  // the race on a host without the rule
  { seed: 23, fault: "no-presence-check" },
  { seed: 29, fault: "no-presence-check" },
  { seed: 7002, opts: SLOW, fault: "no-presence-check" },
  { seed: 7023, opts: SLOW, fault: "no-presence-check" },
];

// Long by design (randomised runs, 3-20 s on a build node). vitest 2 never timed out a synchronous
// test; vitest 3 and later apply the 5 s default to them too, so these suites name their own limit.
describe("three hosts converge (no DOM)", { timeout: 60_000 }, () => {
  it("the fixed seeds: each passes, and fails under its planted fault (with kind, and without: an older kit)", async () => {
    for (const kind of [true, false]) {
      for (const { seed, opts: given, fault } of FIXED_SEEDS) {
        const opts: RunOptions = { ...given, kind };
        const s = await run(seed, opts);
        expect(s.typed, `seed ${seed} typed nothing`).toBeGreaterThan(0);
        if (fault === "hunk-only") {
          await expect(withHunkOnlyRecognition(() => run(seed, opts)), `seed ${seed} under hunk-only recognition`).rejects.toThrow();
        } else if (fault === "region-placement") {
          expect(s.reapplied["applied"] ?? 0, `seed ${seed}: nothing re-applied`).toBeGreaterThan(0);
          await expect(withRegionPlacement(() => run(seed, opts)), `seed ${seed} under M1's region placement`).rejects.toThrow(/server's rule/);
        } else {
          const o: RunOptions = { ...opts, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true };
          const g = await run(seed, o);
          expect(g.falseE000, `seed ${seed}: no false MPC-E000 without the rule`).toBeGreaterThan(0);
          await expect(withoutPresenceCheck(() => run(seed, o)), `seed ${seed} without Re-apply's check`).rejects.toThrow(/twice/);
        }
      }
    }
    runrec("client_live_fixed_seeds", { seeds: FIXED_SEEDS.length, kinds: 2 });
  }, 600_000);

  // Each run below goes twice: with `kind` on change events (a kit v0.10.1 host, where an undo
  // from a lost change's client ID is told apart from the change) and without (an older kit:
  // the rule before kinds). The line printed with each says which; `unknown` is what the kinds
  // save.
  for (const kind of [true, false]) {
    const k = kind ? "with kind" : "without kind (an older kit)";
    const rec = kind ? "" : "_nokind";

    it(`on a host without the reconnect rule, Re-apply on every rejection never doubles text (${k})`, async () => {
      const t = { runs: 0, typed: 0, falseE000: 0, applied: 0, alreadyPresent: 0, unknown: 0 };
      const add = (s: Awaited<ReturnType<typeof run>>) => {
        t.runs++;
        t.typed += s.typed;
        t.falseE000 += s.falseE000;
        t.applied += s.reapplied["applied"] ?? 0;
        t.alreadyPresent += s.reapplied["already-present"] ?? 0;
        t.unknown += s.reapplied["unknown"] ?? 0;
      };
      for (let seed = 1; seed <= 60; seed++) add(await run(seed, { host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true, kind }));
      for (let seed = 7000; seed < 7030; seed++) add(await run(seed, { ...SLOW, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true, kind }));
      // the same late frames on a host with the rule: dropped unapplied, never a false MPC-E000
      let strictLate = 0;
      for (let seed = 1; seed <= 30; seed++) strictLate += (await run(seed, { fastNotice: true, lateOldFrames: 200, kind })).lateDiscarded;
      expect(strictLate, "no late frame reached a host with the rule: the comparison doesn't bite").toBeGreaterThan(0);
      console.log(`no-rule host (${k}): ${t.runs} runs, ${t.falseE000} false MPC-E000, re-apply: ${t.applied} applied, ${t.alreadyPresent} already present, ${t.unknown} unknown`);
      expect(t.falseE000, "the race never happened: the test doesn't bite").toBeGreaterThan(0);
      runrec(`client_live_no_rule_reapply${rec}`, t);
    }, 900_000);

    it(`three hosts, random typing, outside edits, undo and dropped sockets (${k})`, async () => {
      const runs = Number(process.env.LIVE_RUNS ?? 120);
      const t = { runs: 0, typed: 0, rejected: 0, reconnects: 0, drops: 0 };
      for (let seed = 1; seed <= runs; seed++) {
        const s = await run(seed, { kind });
        t.runs++;
        t.typed += s.typed;
        t.rejected += s.rejected;
        t.reconnects += s.reconnects;
        t.drops += s.drops;
      }
      expect(t.typed).toBeGreaterThan(runs * 15);
      expect(t.reconnects).toBeGreaterThan(runs);
      console.log(`host runs ${t.runs} (${k}): ${t.typed} edits typed, ${t.rejected} rejected, ${t.drops} drops, ${t.reconnects} reconnects`);
      runrec(`client_live_converge${rec}`, t);
    }, 600_000);

    it(`on slow links with more drops (${k})`, async () => {
      const t = { runs: 0, typed: 0, rejected: 0, reconnects: 0, drops: 0 };
      for (let seed = 7000; seed < 7040; seed++) {
        const s = await run(seed, { ...SLOW, kind });
        t.runs++;
        t.typed += s.typed;
        t.rejected += s.rejected;
        t.reconnects += s.reconnects;
        t.drops += s.drops;
      }
      console.log(`slow runs ${t.runs} (${k}): ${t.typed} edits typed, ${t.rejected} rejected, ${t.drops} drops, ${t.reconnects} reconnects`);
      runrec(`client_live_slow_drops${rec}`, t);
    }, 600_000);
  }
});
