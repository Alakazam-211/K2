// Cells presence and cells claims in a live grid (`PresenceChannel`): `setCells` sends the `cells`
// frame only to a host that lists `"cells"`, refuses more than 16 ranges itself, sets its own
// focus to the cells' container, and loses its cells when its focus moves away; `claimCells`
// sends one range under `"cell_claims"` and resolves on `claimed`; the roster's `cells` and
// `claim_started_cells` fold into `peers()` and `claims()`, the claim's actor from its event.
//
// Then the kit's roster golden (`test/golden/roster_events_cells.json`, written by the kit's own
// run): folded flush by flush as the person, the agent's cells after each rev equal the shared
// vector's (`test/vectors/grid_view.tsv`), and so do the claim's view and its end. The golden is
// required: without it the row fails.
//
// Plant caught by name: `ClaimCellsActorMissing` (`withClaimCellsActorMissing`).

import { existsSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { CELLS_FEATURE, CELLS_RANGES_MAX, CELL_CLAIMS_FEATURE } from "../src/grid";
import { CLAIMS_FEATURE, ClaimRejected, FOCUS_FEATURE, PresenceChannel, type PresenceRefusal } from "../src/presence";
import { ERR, WIRE_VERSION, type CellRange, type Cells, type ClaimActor, type ClientFrame, type RosterEntry, type RosterEvent, type Welcome } from "../src/wire";
import { canonCells, parseRange, readGridVector, wire } from "./grid-vector";
import { withClaimCellsActorMissing } from "./plants";

const GOLDEN = join(dirname(fileURLToPath(import.meta.url)), "golden", "roster_events_cells.json");
const V = readGridVector();
const AGENT = "agent:atlas";

const welcome = (over: Partial<Welcome> = {}): Welcome => ({
  root: "rows",
  version: 20,
  client: 7,
  self: "person:ada",
  rights: "edit",
  features: ["presence", FOCUS_FEATURE, CELLS_FEATURE, CLAIMS_FEATURE, CELL_CLAIMS_FEATURE],
  wire: WIRE_VERSION,
  ...over,
});

const now = { schedule: (deliver: () => void) => deliver() };

const C4D15 = parseRange("@sheet:1/row:r3,0,@sheet:1/col:c3,0:@sheet:1/row:r14,0,@sheet:1/col:c4,0");
const cells = (container = "@sheet:1", ranges: CellRange[] = [C4D15]): Cells => ({ container, active: ranges[0]?.start ?? null, ranges });
const reqOf = (f: ClientFrame | undefined) => (f as { req: number }).req;

describe("setCells", () => {
  it("sends `cells` with its own request number to a host that lists `cells`; the same value twice is one frame; null clears", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    expect(c.cellsSupported).toBe(true);
    c.setCells(cells(), "sheets");
    c.setCells(cells(), "sheets");
    c.setCells(null);
    expect(sent).toEqual([
      { type: "cells", req: 1, cells: cells() },
      { type: "cells", req: 2, cells: null },
    ]);
  });

  it("a host without `cells` gets nothing; a later one that lists it gets the focus, then the cells", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome({ features: ["presence", FOCUS_FEATURE] }), now);
    expect(c.cellsSupported).toBe(false);
    c.setCells(cells(), "sheets");
    expect(sent).toEqual([]);
    c.reconnected(welcome());
    expect(sent).toEqual([
      { type: "focus", req: 1, focus: { root: "sheets", key: "@sheet:1" } },
      { type: "cells", req: 2, cells: cells() },
    ]);
  });

  it("sets its own focus to the container: no `focus` frame for it, and none for the same focus after", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    c.setCells(cells(), "sheets");
    c.setFocus({ root: "sheets", key: "@sheet:1" });
    expect(sent.map((f) => f.type)).toEqual(["cells"]);
    // the cells stay: a reconnect sends both
    sent.length = 0;
    c.reconnected(welcome());
    expect(sent.map((f) => f.type)).toEqual(["focus", "cells"]);
  });

  it("without a root it keeps the current focus's root; with no focus, none is set", () => {
    const a = wire();
    const c = new PresenceChannel(a.t, welcome(), now);
    c.setFocus({ root: "sheets", key: "@sheet:2" });
    c.setCells(cells());
    a.sent.length = 0;
    c.reconnected(welcome());
    expect(a.sent).toEqual([
      { type: "focus", req: 3, focus: { root: "sheets", key: "@sheet:1" } },
      { type: "cells", req: 4, cells: cells() },
    ]);
    const b = wire();
    const d = new PresenceChannel(b.t, welcome(), now);
    d.setCells(cells());
    b.sent.length = 0;
    d.reconnected(welcome());
    expect(b.sent.map((f) => f.type)).toEqual(["cells"]);
  });

  it("a focus on another container clears its own cells (the host drops them too)", () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    c.setCells(cells(), "sheets");
    c.setFocus({ root: "sheets", key: "@sheet:2" });
    expect(sent.map((f) => f.type)).toEqual(["cells", "focus"]);
    sent.length = 0;
    c.reconnected(welcome());
    expect(sent).toEqual([{ type: "focus", req: 3, focus: { root: "sheets", key: "@sheet:2" } }]);
    // the same cells again are new: they go out
    c.setCells(cells(), "sheets");
    expect(sent.at(-1)).toEqual({ type: "cells", req: 4, cells: cells() });
  });

  it(`more than ${CELLS_RANGES_MAX} ranges are refused here, unsent (MPC-E310); exactly ${CELLS_RANGES_MAX} go`, () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const refused: PresenceRefusal[] = [];
    c.onRefused((r) => refused.push(r));
    c.setCells(cells("@sheet:1", Array.from({ length: CELLS_RANGES_MAX + 1 }, () => C4D15)), "sheets");
    expect(sent).toEqual([]);
    expect(refused.map((r) => [r.frame, r.error.id])).toEqual([["cells", ERR.SELECTION_TOO_LARGE]]);
    c.setCells(cells("@sheet:1", Array.from({ length: CELLS_RANGES_MAX }, () => C4D15)), "sheets");
    expect(sent.map((f) => f.type)).toEqual(["cells"]);
  });

  it("a refused `cells` (a corner not live, MPC-E114) is heard and forgotten: not sent again after a reconnect", () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome({ features: ["presence", CELLS_FEATURE] }), now);
    const refused: PresenceRefusal[] = [];
    c.onRefused((r) => refused.push(r));
    c.setCells(cells());
    deliver({ type: "presence_refused", req: reqOf(sent[0]), error: { id: ERR.BAD_POSITION, message: "bad position" } });
    expect(refused).toEqual([{ frame: "cells", error: { id: ERR.BAD_POSITION, message: "bad position" } }]);
    sent.length = 0;
    c.reconnected(welcome({ features: ["presence", CELLS_FEATURE] }));
    expect(sent).toEqual([]);
  });
});

describe("claimCells", () => {
  it("sends the claim frame with `cells` in place of `root` and `keys`, and resolves on `claimed`", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    expect(c.cellClaimsSupported).toBe(true);
    const p = c.claimCells({ container: "@sheet:1", ranges: [C4D15] }, { note: "updating Q2 figures", hold: true, ttlMs: 60000 });
    expect(sent).toEqual([{ type: "claim", req: 1, cells: { container: "@sheet:1", ranges: [C4D15] }, ttl_ms: 60000, note: "updating Q2 figures", hold: true }]);
    expect("root" in sent[0] || "keys" in sent[0]).toBe(false);
    deliver({ type: "claimed", req: 1, claim: "claim-3" });
    await expect(p).resolves.toBe("claim-3");
  });

  it("a host without `cell_claims` gets nothing: unsupported, even when it takes claims on elements", async () => {
    const { t, sent } = wire();
    const c = new PresenceChannel(t, welcome({ features: ["presence", CLAIMS_FEATURE] }), now);
    await expect(c.claimCells({ container: "@sheet:1", ranges: [C4D15] })).rejects.toMatchObject({ name: "ClaimRejected", reason: "unsupported" });
    expect(sent).toEqual([]);
    // and the other way round: cell claims alone take no claim on elements
    const b = wire();
    const d = new PresenceChannel(b.t, welcome({ features: [CELL_CLAIMS_FEATURE] }), now);
    await expect(d.claim("rows", ["@sheet:1/row:r3"])).rejects.toMatchObject({ reason: "unsupported" });
    expect(b.sent).toEqual([]);
  });

  it("other than one range is refused here, unsent (MPC-E304); the host's refusal rejects with its error", async () => {
    const { t, sent, deliver } = wire();
    const c = new PresenceChannel(t, welcome(), now);
    const refused: PresenceRefusal[] = [];
    c.onRefused((r) => refused.push(r));
    for (const ranges of [[], [C4D15, C4D15]]) {
      await expect(c.claimCells({ container: "@sheet:1", ranges })).rejects.toMatchObject({ reason: "refused", error: { id: ERR.BAD_CLAIM } });
    }
    await expect(c.claimCells({ container: "@sheet:1", ranges: [C4D15] }, { ttlMs: 0 })).rejects.toMatchObject({ error: { id: ERR.BAD_CLAIM } });
    expect(sent).toEqual([]);
    expect(refused.map((r) => [r.frame, r.error.id])).toEqual([
      ["claim", ERR.BAD_CLAIM],
      ["claim", ERR.BAD_CLAIM],
      ["claim", ERR.BAD_CLAIM],
    ]);
    const p = c.claimCells({ container: "@sheet:1", ranges: [C4D15] });
    const taken = { id: "MPC-E301", message: "cells claimed by Atlas" };
    deliver({ type: "presence_refused", req: reqOf(sent[0]), error: taken });
    const why = await p.catch((e: unknown) => e);
    expect(why).toBeInstanceOf(ClaimRejected);
    expect(why).toMatchObject({ reason: "refused", error: taken });
    expect(refused.at(-1)).toEqual({ frame: "claim", error: taken });
  });
});

describe("cells and cells claims in the roster", () => {
  const atlasRow: RosterEntry = { handle: AGENT, name: "Atlas", colour: "#6d5bd0", kind: "agent", role: "editor", away: false, cursor: null };
  const adaRow: RosterEntry = { handle: "person:ada", name: "Ada", colour: "#2f9e8f", kind: "person", role: "editor", away: false, cursor: null };

  function channel() {
    const w = wire();
    const c = new PresenceChannel(w.t, welcome(), now);
    const ev = (event: RosterEvent) => w.deliver({ type: "roster_event", event });
    return { c, ev };
  }

  it("a snapshot's cells and cell_claims, then `cells` events, fold into peers() and claims()", () => {
    const { c, ev } = channel();
    const held = { claim: "claim-2", container: "@sheet:1", ranges: [C4D15], note: null, hold: false, expires_ms: 90000, started_ms: 30000 };
    ev({ kind: "snapshot", version: 20, watching: 0, members: [adaRow, { ...atlasRow, cells: cells(), cell_claims: [held] }] });
    const atlas = () => c.peers().find((p) => p.handle === AGENT)!;
    expect(atlas().cells).toEqual(cells());
    expect(c.peers().find((p) => p.handle === "person:ada")!.cells).toBeNull();
    expect(c.claims()).toEqual([
      {
        claim: "claim-2",
        actor: { id: AGENT, name: "Atlas", kind: "agent", colour: "#6d5bd0" },
        root: "",
        keys: [],
        note: null,
        hold: false,
        startedMs: 30000,
        expiresMs: 90000,
        cells: { container: "@sheet:1", ranges: [C4D15] },
      },
    ]);
    ev({ kind: "cells", who: AGENT, cells: cells("@sheet:2", []) });
    expect(atlas().cells).toEqual(cells("@sheet:2", []));
    ev({ kind: "cells", who: AGENT, cells: null });
    expect(atlas().cells).toBeNull();
  });

  it("per connection: a device's cells are its own", () => {
    const { c, ev } = channel();
    ev({ kind: "snapshot", version: 20, watching: 0, members: [{ ...adaRow, handle: "person:bo", devices: [{ client: 1, cursor: null, cells: cells() }, { client: 2, cursor: null }] }] });
    ev({ kind: "cells", who: "person:bo", cells: cells("@sheet:2", []), client: 2 });
    expect(c.peers().filter((p) => p.handle === "person:bo").map((p) => [p.client, p.cells?.container])).toEqual([
      [1, "@sheet:1"],
      [2, "@sheet:2"],
    ]);
  });

  it("claim_started_cells: the actor from the event (not the roster row), root \"\" and no keys; claim_ended ends it", () => {
    const { c, ev } = channel();
    ev({ kind: "snapshot", version: 20, watching: 0, members: [adaRow, atlasRow] });
    const actor: ClaimActor = { id: AGENT, name: "Atlas (as it claimed)", kind: "agent", colour: "#123456" };
    ev({ kind: "claim_started_cells", who: AGENT, claim: "claim-4", container: "@sheet:1", ranges: [C4D15], note: "updating Q2 figures", hold: true, expires_ms: 80000, version: 20, actor, started_ms: 20000 });
    expect(c.claims()).toEqual([
      { claim: "claim-4", actor, root: "", keys: [], note: "updating Q2 figures", hold: true, startedMs: 20000, expiresMs: 80000, cells: { container: "@sheet:1", ranges: [C4D15] } },
    ]);
    // a copy: changing what claims() gave changes nothing
    c.claims()[0].cells!.ranges.length = 0;
    expect(c.claims()[0].cells!.ranges).toEqual([C4D15]);
    ev({ kind: "claim_ended", who: AGENT, claim: "claim-4", how: "released", version: 25, actor, at_ms: 25000, at_rev: null, lines: { root: "rows", first: 4, last: 16 } });
    expect(c.claims()).toEqual([]);
  });

  it("plant ClaimCellsActorMissing (the view's actor not the event's) is caught", () => {
    const run = () => {
      const { c, ev } = channel();
      ev({ kind: "snapshot", version: 20, watching: 0, members: [atlasRow] });
      const actor: ClaimActor = { id: AGENT, name: "Atlas", kind: "agent", colour: "#6d5bd0" };
      ev({ kind: "claim_started_cells", who: AGENT, claim: "claim-4", container: "@sheet:1", ranges: [C4D15], note: null, hold: false, expires_ms: 1, version: 20, actor, started_ms: 0 });
      return JSON.stringify(c.claims()[0]?.actor) === JSON.stringify(actor);
    };
    expect(run()).toBe(true);
    expect(withClaimCellsActorMissing(run)).toBe(false);
  });
});

// ---------- the kit's roster golden ----------

type Flush = { rev: number; events: RosterEvent[] };
type CellsGolden = { self: string; client: number; flushes: Flush[] };

/** The kit's golden; a clear failure when it isn't there yet. */
function loadGolden(): CellsGolden {
  if (!existsSync(GOLDEN)) {
    throw new Error(
      "test/golden/roster_events_cells.json is missing: the kit writes it from its own run (the roster events of the shared vector's session). This row needs it and fails until it is committed.",
    );
  }
  return JSON.parse(readFileSync(GOLDEN, "utf8")) as CellsGolden;
}

/** Fold the golden into a channel as its person, flush by flush, and every place it differs
 * from the vector (empty: none). */
function goldenProblems(g: CellsGolden): string[] {
  const problems: string[] = [];
  const { t, deliver } = wire();
  const c = new PresenceChannel(t, { root: "rows", version: 0, client: g.client, self: g.self, rights: "edit" }, now);
  const byRev = new Map(V.revs.map((r) => [r.rev, r]));
  const seen = new Set<number>();
  let claimSeen = false;
  let ended = false;
  const agentCells = (at: string): string => {
    const rows = c.peers().filter((p) => p.handle === AGENT);
    if (rows.length !== 1) {
      problems.push(`${at}: ${rows.length} rows for the agent`);
      return "";
    }
    return canonCells(rows[0].cells);
  };
  for (const fl of g.flushes) {
    for (const event of fl.events) deliver({ type: "roster_event", event });
    const started = fl.events.find((e): e is Extract<RosterEvent, { kind: "claim_started_cells" }> => e.kind === "claim_started_cells");
    if (started) {
      claimSeen = true;
      const want = V.claim!;
      const views = c.claims().filter((x) => x.cells);
      if (views.length !== 1) problems.push(`after the claim: ${views.length} claims on cells`);
      const v = views[0];
      if (v) {
        if (v.cells!.container !== want.container) problems.push(`the claim's container: ${v.cells!.container}`);
        if (canonCells({ container: "", active: null, ranges: v.cells!.ranges }) !== canonCells({ container: "", active: null, ranges: [want.range] })) problems.push(`the claim's range: ${JSON.stringify(v.cells!.ranges)}`);
        if (v.note !== want.note) problems.push(`the claim's note: ${v.note}`);
        if (v.hold !== want.hold) problems.push(`the claim's hold: ${v.hold}`);
        if (v.root !== "" || v.keys.length !== 0) problems.push("the claim names a root or keys");
        const a = v.actor;
        if (a.id !== AGENT || a.kind !== "agent" || !a.name || !a.colour) problems.push(`the claim's actor: ${JSON.stringify(a)}`);
        if (JSON.stringify(a) !== JSON.stringify(started.actor)) problems.push(`the claim's actor isn't the event's: ${JSON.stringify(a)}`);
      }
      const got = agentCells("after the claim");
      if (got && got !== canonCells(V.claimCells)) problems.push(`after the claim, the agent's cells: ${got}`);
      continue;
    }
    const r = byRev.get(fl.rev);
    if (!r || fl.rev <= V.rev) continue;
    seen.add(fl.rev);
    if (r.agentCells) {
      const got = agentCells(`rev ${fl.rev}`);
      if (got && got !== canonCells(r.agentCells)) problems.push(`rev ${fl.rev}, the agent's cells: ${got} for ${canonCells(r.agentCells)}`);
    }
    if (r.claimLines) {
      ended = true;
      if (c.claims().some((x) => x.cells)) problems.push(`rev ${fl.rev}: the claim on cells is still live`);
      const end = fl.events.find((e) => e.kind === "claim_ended");
      const lines = end?.kind === "claim_ended" ? end.lines : undefined;
      if (!lines || lines.first !== r.claimLines.first || lines.last !== r.claimLines.last) problems.push(`rev ${fl.rev}: claim_ended's lines ${JSON.stringify(lines)}`);
    } else if (!ended && claimSeen && !c.claims().some((x) => x.cells)) problems.push(`rev ${fl.rev}: the claim on cells ended early`);
  }
  if (!claimSeen) problems.push("no flush with claim_started_cells");
  for (const r of V.revs) if (r.agentCells && !seen.has(r.rev)) problems.push(`no flush after rev ${r.rev}`);
  return problems;
}

describe("the kit's roster golden (test/golden/roster_events_cells.json)", () => {
  it("is there, as the person, with a snapshot first", () => {
    const g = loadGolden();
    expect(g.self).toBe("person:ada");
    expect(Number.isSafeInteger(g.client)).toBe(true);
    expect(g.flushes[0].events[0].kind).toBe("snapshot");
  });

  it("folds flush by flush: the agent's cells after each rev, the claim's view and its end, as the vector says", () => {
    expect(goldenProblems(loadGolden())).toEqual([]);
  });

  it("plant ClaimCellsActorMissing is caught on it", () => {
    const g = loadGolden();
    expect(goldenProblems(g)).toEqual([]);
    const planted = withClaimCellsActorMissing(() => goldenProblems(g));
    expect(planted.some((p) => p.includes("actor"))).toBe(true);
  });
});
