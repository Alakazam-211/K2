// `PresenceChannel.claims()`: the live claims on elements folded from the roster, each with its
// actor and times (wire 3). A wire-3 host's events carry
// the actor; a wire-2 host's don't, and the claimant's roster row gives it. The kit's own stream
// (`roster_events_claims_elems.json`) folds to the claims its final snapshot names.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { PresenceChannel, type ClaimView } from "../src/presence";
import type { ClientFrame, RosterEntry, RosterEvent, ServerFrame, Transport, Welcome } from "../src/wire";

const GOLDEN = join(dirname(fileURLToPath(import.meta.url)), "golden");

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

const welcome = (over: Partial<Welcome> = {}): Welcome => ({ root: "source", version: 1, client: 7, self: "ada", rights: "edit", ...over });
const now = { schedule: (deliver: () => void) => deliver() };

const atlasRow: RosterEntry = { handle: "agent:atlas", name: "Atlas", colour: "#6d5bd0", kind: "agent", role: "editor", away: false, cursor: null };
const snapshot = (members: RosterEntry[]): RosterEvent => ({ kind: "snapshot", version: 40, watching: 0, members });

function channel() {
  const w = wire();
  const c = new PresenceChannel(w.t, welcome(), now);
  const seen: (readonly ClaimView[])[] = [];
  c.onClaims((cl) => seen.push(cl));
  const ev = (event: RosterEvent) => w.deliver({ type: "roster_event", event });
  return { c, seen, ev };
}

describe("watching", () => {
  it("follows the snapshot and `watching {count}`, says when it changes, and watchers get no row (GOLD case H)", () => {
    const { c, ev } = channel();
    const told: number[] = [];
    c.onWatching((n) => told.push(n));
    expect(c.watching).toBe(0);
    ev(snapshot([atlasRow]));
    ev({ kind: "watching", count: 1 });
    ev({ kind: "watching", count: 1 });
    expect(c.watching).toBe(1);
    expect(c.peers().map((p) => p.handle)).toEqual(["agent:atlas"]);
    ev({ kind: "watching", count: 0 });
    ev({ ...snapshot([atlasRow]), watching: 2 } as RosterEvent);
    expect(told).toEqual([1, 0, 2]);
  });

  it("the kit's stream: roster_events_focus_range.json counts its watcher", () => {
    const g = JSON.parse(readFileSync(join(GOLDEN, "roster_events_focus_range.json"), "utf8")) as { events: RosterEvent[]; final: RosterEvent };
    const live = channel();
    for (const e of g.events) live.ev(e);
    const late = channel();
    late.ev(g.final);
    expect(live.c.watching).toBe(1);
    expect(late.c.watching).toBe(1);
  });
});

describe("claims() (wire 3)", () => {
  it("a claim's start and end with the actor and times (GOLD c-11): listed, then gone", () => {
    const { c, seen, ev } = channel();
    ev(snapshot([atlasRow]));
    const actor = { id: "agent:atlas", name: "Atlas", kind: "agent", colour: "#6d5bd0" };
    ev({
      kind: "claim_started_elems",
      who: "agent:atlas",
      claim: "claim-11",
      root: "shapes",
      keys: ["s256-2"],
      note: "rewriting the title",
      hold: false,
      expires_ms: 130900,
      version: 40,
      actor,
      started_ms: 100900,
    });
    expect(c.claims()).toEqual([
      { claim: "claim-11", actor, root: "shapes", keys: ["s256-2"], note: "rewriting the title", hold: false, startedMs: 100900, expiresMs: 130900 },
    ]);
    ev({ kind: "claim_ended", who: "agent:atlas", claim: "claim-11", how: "edited", version: 41, actor, at_ms: 101200, at_rev: 41 });
    expect(c.claims()).toEqual([]);
    expect(seen.map((s) => s.length)).toEqual([0, 1, 0]);
  });

  it("from a wire-2 host (no actor, no times) the claimant's roster row gives the actor; startedMs is null", () => {
    const { c, ev } = channel();
    ev(snapshot([atlasRow]));
    ev({ kind: "claim_started_elems", who: "agent:atlas", claim: "claim-1", root: "slides", keys: ["s259", "s258"], note: null, hold: false, expires_ms: 5, version: 1 });
    const [cl] = c.claims();
    expect(cl.actor).toEqual({ id: "agent:atlas", name: "Atlas", kind: "agent", colour: "#6d5bd0" });
    expect(cl.startedMs).toBeNull();
    expect(cl.keys).toEqual(["s259", "s258"]);
  });

  it("a late subscriber's snapshot names the live claims (elem_claims), the member's row the actor", () => {
    const { c, ev } = channel();
    ev(snapshot([{ ...atlasRow, elem_claims: [{ claim: "claim-13", root: "slides", keys: ["s260"], note: "adding a Risks slide", hold: true, expires_ms: 166700, started_ms: 106700 }] }]));
    expect(c.claims()).toEqual([
      {
        claim: "claim-13",
        actor: { id: "agent:atlas", name: "Atlas", kind: "agent", colour: "#6d5bd0" },
        root: "slides",
        keys: ["s260"],
        note: "adding a Risks slide",
        hold: true,
        startedMs: 106700,
        expiresMs: 166700,
      },
    ]);
  });

  it("the kit's stream folds to the claims its final snapshot names, with the actor", () => {
    const g = JSON.parse(readFileSync(join(GOLDEN, "roster_events_claims_elems.json"), "utf8")) as { self: string; events: RosterEvent[]; final: RosterEvent };
    const live = channel();
    for (const e of g.events) live.ev(e);
    const late = channel();
    late.ev(g.final);
    const brief = (cs: readonly ClaimView[]) => cs.map((x) => [x.claim, x.actor.name, x.actor.kind, x.root, x.keys.join(","), x.hold, x.startedMs !== null]);
    expect(brief(live.c.claims())).toEqual(brief(late.c.claims()));
    expect(live.c.claims().length).toBeGreaterThan(0);
    // every started event carried its actor (wire 3)
    for (const e of g.events) if (e.kind === "claim_started_elems") expect(e.actor?.name).toBe("Bot");
  });
});
