// @vitest-environment jsdom
// Mounted editors through the convergence harness (`editor-harness.ts`): random runs, slow
// links with more drops, and the fixed seeds of past failures.

import { describe, expect, it } from "vitest";
import { run, withHunkOnlyRecognition, withoutPresenceCheck, withoutReapplyGuard, type RunOptions } from "./editor-harness";
import { runrec } from "./runrec";

/**
 * Seeds that exercise a planted fault, run every time whatever `EDITOR_RUNS` says. Each must
 * pass, and fail with its fault planted (`editor-seed-scan.test.ts` finds them):
 * - `hunk-only`: LiveText recognising its own change in a `since` reply by hunks alone takes
 *   someone else's identical change for its own ("diverged: the server applied a change the
 *   client saw conflict"; first seen as seed 51, before the wire contract's client IDs);
 * - `no-presence-check`: on a host without the reconnect rule an edit on a dropped socket lands
 *   after the resync (a false MPC-E000, which the client recognises by its client ID);
 *   without that recognition and Re-apply's check, and without Re-apply's tie guard (the late
 *   edit lands at its very place, which the guard sees as a tie), Re-apply puts the text in
 *   twice. With the check out but the tie guard in, the run still passes: two layers.
 * Seed 27's shape (a kept edit and a second drop) and the race itself are scripted in
 * `editor.test.ts`.
 */
type Fixed = { seed: number; opts?: RunOptions; fault: "hunk-only" | "no-presence-check" };
const SLOW: RunOptions = { steps: 50, drops: 6, maxLatency: 120 };
const FIXED_SEEDS: Fixed[] = [
  // found by the scan (job bcc38413, seeds 1-1500)
  { seed: 688, fault: "hunk-only" },
  { seed: 1231, fault: "hunk-only" },
  { seed: 1381, fault: "hunk-only" },
  // the race on a host without the rule (jobs f5166c71, 1bb87d8b, after the multi-hunk
  // refusal: slow 7011's lost edit had two hunks, so it no longer doubles even with the guard
  // out, and slow 7014 replaced it); 85 and slow 7008 doubled with the text-matching check
  // that came before the client-ID one
  { seed: 10, fault: "no-presence-check" },
  { seed: 85, fault: "no-presence-check" },
  { seed: 7014, opts: SLOW, fault: "no-presence-check" },
  { seed: 7008, opts: SLOW, fault: "no-presence-check" },
];

describe("mounted editors converge (jsdom)", () => {
  it("the fixed seeds: each passes, and fails under its planted fault", async () => {
    for (const { seed, opts, fault } of FIXED_SEEDS) {
      const s = await run(seed, opts);
      expect(s.typed, `seed ${seed} typed nothing`).toBeGreaterThan(0);
      if (fault === "hunk-only") {
        await expect(withHunkOnlyRecognition(() => run(seed, opts)), `seed ${seed} under hunk-only recognition`).rejects.toThrow();
      } else {
        const o: RunOptions = { ...opts, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true };
        const g = await run(seed, o);
        expect(g.falseE000, `seed ${seed}: no false MPC-E000 without the rule`).toBeGreaterThan(0);
        await expect(withoutPresenceCheck(() => run(seed, o)), `seed ${seed} without Re-apply's check (the tie guard in)`).resolves.toBeDefined();
        await expect(
          withoutPresenceCheck(() => withoutReapplyGuard("tie", () => run(seed, o))),
          `seed ${seed} without Re-apply's check and its tie guard`,
        ).rejects.toThrow(/twice/);
      }
    }
    runrec("client_editor_fixed_seeds", { seeds: FIXED_SEEDS.length });
  }, 300_000);

  it("on a host without the reconnect rule, Re-apply on every rejection never doubles text", async () => {
    const t = { runs: 0, typed: 0, falseE000: 0, applied: 0, alreadyPresent: 0, unknown: 0 };
    const add = (s: Awaited<ReturnType<typeof run>>) => {
      t.runs++;
      t.typed += s.typed;
      t.falseE000 += s.falseE000;
      t.applied += s.reapplied["applied"] ?? 0;
      t.alreadyPresent += s.reapplied["already-present"] ?? 0;
      t.unknown += s.reapplied["unknown"] ?? 0;
    };
    for (let seed = 1; seed <= 60; seed++) add(await run(seed, { host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true }));
    for (let seed = 7000; seed < 7030; seed++) add(await run(seed, { ...SLOW, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true }));
    // the same late frames on a host with the rule: dropped unapplied, never a false MPC-E000
    let strictLate = 0;
    for (let seed = 1; seed <= 30; seed++) strictLate += (await run(seed, { fastNotice: true, lateOldFrames: 200 })).lateDiscarded;
    expect(strictLate, "no late frame reached a host with the rule: the comparison doesn't bite").toBeGreaterThan(0);
    console.log(`no-rule host: ${t.runs} runs, ${t.falseE000} false MPC-E000, re-apply: ${t.applied} applied, ${t.alreadyPresent} already present, ${t.unknown} unknown`);
    expect(t.falseE000, "the race never happened: the test doesn't bite").toBeGreaterThan(0);
    runrec("client_editor_no_rule_reapply", t);
  }, 900_000);

  it("three editors, random typing, outside edits, undo and dropped sockets", async () => {
    const runs = Number(process.env.EDITOR_RUNS ?? 120);
    const t = { runs: 0, typed: 0, rejected: 0, reconnects: 0, drops: 0 };
    for (let seed = 1; seed <= runs; seed++) {
      const s = await run(seed);
      t.runs++;
      t.typed += s.typed;
      t.rejected += s.rejected;
      t.reconnects += s.reconnects;
      t.drops += s.drops;
    }
    expect(t.typed).toBeGreaterThan(runs * 15);
    expect(t.reconnects).toBeGreaterThan(runs);
    console.log(`editor runs ${t.runs}: ${t.typed} edits typed, ${t.rejected} rejected, ${t.drops} drops, ${t.reconnects} reconnects`);
    runrec("client_editor_converge", t);
  }, 600_000);

  it("on slow links with more drops", async () => {
    const t = { runs: 0, typed: 0, rejected: 0, reconnects: 0, drops: 0 };
    for (let seed = 7000; seed < 7040; seed++) {
      const s = await run(seed, SLOW);
      t.runs++;
      t.typed += s.typed;
      t.rejected += s.rejected;
      t.reconnects += s.reconnects;
      t.drops += s.drops;
    }
    runrec("client_editor_slow_drops", t);
  }, 600_000);
});
