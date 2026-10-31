// @vitest-environment jsdom
// A scan for seeds that exercise the planted faults, to pick the fixed seeds. Skipped unless
// `EDITOR_SCAN` is set: `EDITOR_SCAN=1-300 [EDITOR_SCAN_SLOW=1] npx vitest run
// test/editor-seed-scan.test.ts`. One line per seed that shows something:
// - `hunk-only`: the run fails when LiveText recognises its own change by hunks alone;
// - `no-rule`: on a host without the reconnect rule, how many MPC-E000s were for edits the
//   server applied after all, and whether Re-apply on every rejection, with its check planted
//   out, puts text in twice (it must not with the check);
// - `strict`: the run on a host with the rule (must pass).

import { describe, expect, it } from "vitest";
import { run, withHunkOnlyRecognition, withoutPresenceCheck, type RunOptions } from "./editor-harness";

const range = process.env.EDITOR_SCAN;
const only = process.env.EDITOR_SCAN_ONLY; // "hunk-only" or "no-rule"
const base: RunOptions = process.env.EDITOR_SCAN_SLOW ? { steps: 50, drops: 6, maxLatency: 120 } : {};

const fails = async (f: () => Promise<unknown>) => {
  try {
    await f();
    return null;
  } catch (e) {
    return String((e as Error).message).split("\n")[0].slice(0, 140);
  }
};

describe.skipIf(!range)("seed scan", () => {
  it("seeds the planted faults fail on", async () => {
    const [from, to] = range!.split("-").map(Number);
    const picks: Record<string, number[]> = { "hunk-only": [], "no-presence-check": [], "strict-fail": [], "guard-fail": [] };
    for (let seed = from; seed <= to; seed++) {
      const strict = await fails(() => run(seed, base));
      const hunkOnly = only === "no-rule" ? null : await fails(() => withHunkOnlyRecognition(() => run(seed, base)));
      let falseE000 = 0;
      const guarded = only === "hunk-only" ? null : await fails(async () => {
        falseE000 = (await run(seed, { ...base, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true })).falseE000;
      });
      const unguarded = falseE000 > 0 ? await fails(() => withoutPresenceCheck(() => run(seed, { ...base, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true }))) : null;
      if (strict) picks["strict-fail"].push(seed);
      if (guarded) picks["guard-fail"].push(seed);
      if (hunkOnly && !strict) picks["hunk-only"].push(seed);
      if (unguarded && !guarded && !strict) picks["no-presence-check"].push(seed);
      if (strict || hunkOnly || falseE000 || guarded)
        console.log(
          `SCAN seed ${seed}: strict ${strict ?? "ok"} | hunk-only ${hunkOnly ?? "ok"} | no-rule false E000 ${falseE000}, guarded ${guarded ?? "ok"}, unguarded ${unguarded ?? "ok"}`,
        );
    }
    for (const [k, v] of Object.entries(picks)) console.log(`SCAN ${from}-${to}${base.steps ? " slow" : ""} ${k}: ${v.join(",") || "none"}`);
    expect(to).toBeGreaterThanOrEqual(from);
  }, 3_600_000);
});
