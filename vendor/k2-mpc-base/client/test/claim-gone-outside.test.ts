// A claim on an element an outside save removed (WIRE.md, "Claims on elements"): the kit's own
// stream (`test/golden/roster_events_claim_gone_outside.json`, written by
// `crates/base/tests/wire_golden.rs`) ends the claim with `how: "gone_outside_save"` and `keys`,
// its element addresses as they were. `PresenceChannel` drops the claim, as for any end, and
// `onClaimEnded` hands on the reason and the addresses, so a view can name what went. A reason
// this client doesn't know is read as `deleted`.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PresenceChannel, claimEndReason, type ClaimEnded } from "../src/presence";
import type { ClientFrame, RosterEvent, ServerFrame, Transport, Welcome } from "../src/wire";

const GOLDEN = join(dirname(fileURLToPath(import.meta.url)), "golden");
type Golden = { self: string; events: RosterEvent[]; final: RosterEvent };
const load = () => JSON.parse(readFileSync(join(GOLDEN, "roster_events_claim_gone_outside.json"), "utf8")) as Golden;

function wire() {
  const ls = new Set<(f: ServerFrame) => void>();
  const sent: ClientFrame[] = [];
  const t: Transport = {
    send: (f) => void sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  return { t, sent, deliver: (f: ServerFrame) => [...ls].forEach((l) => l(f)) };
}

const welcome: Welcome = { root: "items", version: 1, client: 7, self: "ada", rights: "edit" };
const now = { schedule: (deliver: () => void) => deliver() };

describe("a claim gone in an outside save", () => {
  it("the kit's stream ends the claim gone_outside_save, with its element addresses, and the channel drops it", () => {
    const g = load();
    const w = wire();
    const c = new PresenceChannel(w.t, welcome, now);
    const ended: ClaimEnded[] = [];
    c.onClaimEnded((e) => ended.push(e));
    const started = g.events.filter((e) => e.kind === "claim_started_elems");
    expect(started.length).toBe(1);
    let held = 0;
    for (const event of g.events) {
      w.deliver({ type: "roster_event", event });
      if (event.kind === "claim_started_elems") held = c.claims().length;
    }
    expect(held).toBe(1);
    expect(c.claims()).toEqual([]);
    expect(ended).toEqual([{ claim: (started[0] as { claim: string }).claim, who: "bot", how: "gone_outside_save", keys: ["@item:i2"] }]);
    // the wire's own fields
    const end = g.events.find((e) => e.kind === "claim_ended") as { how: string; keys?: string[]; at_rev?: number | null };
    expect(end.how).toBe("gone_outside_save");
    expect(end.keys).toEqual(["@item:i2"]);
    expect(typeof end.at_rev).toBe("number");
  });

  it("a reason this client doesn't know is read as deleted; the ones it knows as they are", () => {
    for (const how of ["released", "edited", "expired", "left", "reset", "deleted", "gone_outside_save"]) expect(claimEndReason(how)).toBe(how);
    expect(claimEndReason("renamed_on_the_moon")).toBe("deleted");
  });

  it("an end with no keys (a claim on lines, or an older kit) hands on none", () => {
    const w = wire();
    const c = new PresenceChannel(w.t, welcome, now);
    const ended: ClaimEnded[] = [];
    c.onClaimEnded((e) => ended.push(e));
    w.deliver({ type: "roster_event", event: { kind: "claim_ended", who: "bot", claim: "c9", how: "released", version: 3 } });
    expect(ended).toEqual([{ claim: "c9", who: "bot", how: "released", keys: [] }]);
  });
});
