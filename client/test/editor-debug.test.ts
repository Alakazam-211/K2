// @vitest-environment jsdom
// Debug aid (skipped unless EDITOR_DEBUG=<seed>): a no-rule run with Re-apply on every
// rejection, printing what presence saw for each lost change.

import { describe, it, vi } from "vitest";
import { LiveText } from "../src/live";
import { run } from "./editor-harness";

const seed = process.env.EDITOR_DEBUG;

describe.skipIf(!seed)("debug", () => {
  it("prints presence for each lost change", async () => {
    const orig = LiveText.prototype.presence;
    const spy = vi.spyOn(LiveText.prototype, "presence").mockImplementation(function (this: LiveText, id: number) {
      const out = orig.call(this, id);
      const r = this.rejected.find((x) => x.id === id);
      console.log(`DEBUG presence id ${id} -> ${out} | text ${JSON.stringify(this.text)} | version ${this.version} | rej ${JSON.stringify({ text: r?.text, error: r?.error?.id, region: r?.region, asOf: r?.asOf })}`);
      return out;
    });
    try {
      const slow = process.env.EDITOR_DEBUG_SLOW ? { steps: 50, drops: 6, maxLatency: 120 } : {};
      await run(Number(seed), { ...slow, host: "no-rule", noRuleLeave: "keepalive", fastNotice: true, lateOldFrames: 200, reapplyAll: true, trace: true });
      console.log("DEBUG run passed");
    } catch (e) {
      console.log(`DEBUG run failed: ${String((e as Error).message).split("\n")[0]}`);
    } finally {
      spy.mockRestore();
    }
  });
});
