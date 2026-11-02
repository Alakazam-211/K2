// A scan for seeds that exercise the planted faults, to pick `live-convergence.test.ts`'s fixed
// seeds. Skipped unless `LIVE_SCAN` is set: `LIVE_SCAN=1-300 [LIVE_SCAN_SLOW=1]
// [LIVE_SCAN_ONLY=hunk-only|no-rule|region] npx vitest run test/live-seed-scan.test.ts`.
// Each seed runs with `kind` on change events and without (an older kit); a seed is picked for a
// fault only when it holds for both. One line per seed that shows something (`SCAN`):
// - `strict`: the run on a host with the rule (must pass);
// - `hunk-only`: the run fails when LiveText recognises its own change by hunks alone;
// - `no-rule`: on a host without the reconnect rule, how many MPC-E000s were for edits the
//   server applied after all, and whether Re-apply on every rejection, with its check planted
//   out, puts text in twice (it must not with the check);
// - `region`: Re-apply on every rejection applies something, and M1's region placement fails
//   the server's-rule oracle.

import { describe, expect, it } from "vitest";
import { run, type RunOptions } from "./live-host";
import { withHunkOnlyRecognition, withoutPresenceCheck, withRegionPlacement } from "./plants";

const range = process.env.LIVE_SCAN;
const only = process.env.LIVE_SCAN_ONLY;
const base: RunOptions = process.env.LIVE_SCAN_SLOW ? { steps: 50, drops: 6, maxLatency: 120 } : {};
const NO_RULE: RunOptions = { host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true };

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
    const picks: Record<string, number[]> = { "hunk-only": [], "no-presence-check": [], "region-placement": [], "strict-fail": [], "guard-fail": [] };
    for (let seed = from; seed <= to; seed++) {
      const per = { strict: true, hunk: true, noRule: true, region: true, strictFail: false, guardFail: false };
      const lines: string[] = [];
      for (const kind of [true, false]) {
        const k = { ...base, kind };
        const strict = await fails(() => run(seed, k));
        const hunkOnly = only && only !== "hunk-only" ? null : await fails(() => withHunkOnlyRecognition(() => run(seed, k)));
        let falseE000 = 0;
        const guarded =
          only && only !== "no-rule"
            ? null
            : await fails(async () => {
                falseE000 = (await run(seed, { ...k, ...NO_RULE })).falseE000;
              });
        const unguarded = falseE000 > 0 ? await fails(() => withoutPresenceCheck(() => run(seed, { ...k, ...NO_RULE }))) : null;
        let applied = 0;
        let regionOk: string | null = null;
        let region: string | null = null;
        if (!only || only === "region") {
          regionOk = await fails(async () => {
            applied = (await run(seed, { ...k, reapplyAll: true })).reapplied["applied"] ?? 0;
          });
          region = applied > 0 && !regionOk ? await fails(() => withRegionPlacement(() => run(seed, { ...k, reapplyAll: true }))) : null;
        }
        if (strict) per.strictFail = true;
        if (guarded || regionOk) per.guardFail = true;
        if (!(hunkOnly && !strict)) per.hunk = false;
        if (!(unguarded && /twice/.test(unguarded) && !guarded && !strict)) per.noRule = false;
        if (!(region && /server's rule/.test(region) && !strict)) per.region = false;
        if (strict || hunkOnly || falseE000 || guarded || region || regionOk)
          lines.push(
            `kind ${kind}: strict ${strict ?? "ok"} | hunk-only ${hunkOnly ?? "ok"} | no-rule false E000 ${falseE000}, guarded ${guarded ?? "ok"}, unguarded ${unguarded ?? "ok"} | reapplyAll ${regionOk ?? "ok"} applied ${applied}, region ${region ?? "ok"}`,
          );
      }
      if (per.strictFail) picks["strict-fail"].push(seed);
      if (per.guardFail) picks["guard-fail"].push(seed);
      if (per.hunk && (!only || only === "hunk-only")) picks["hunk-only"].push(seed);
      if (per.noRule && (!only || only === "no-rule")) picks["no-presence-check"].push(seed);
      if (per.region && (!only || only === "region")) picks["region-placement"].push(seed);
      for (const l of lines) console.log(`SCAN seed ${seed} ${l}`);
    }
    for (const [k, v] of Object.entries(picks)) console.log(`SCAN ${from}-${to}${base.steps ? " slow" : ""} ${k}: ${v.join(",") || "none"}`);
    expect(to).toBeGreaterThanOrEqual(from);
  }, 3_600_000);
});
