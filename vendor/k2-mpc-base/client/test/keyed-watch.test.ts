// `KeyedWatch`: a watcher's copy of a live keyed doc from the kit's keyed watch (WIRE.md, "The
// keyed watch"). The kit's own frames (`test/vectors/keyed_watch.json`, seeded traces written by
// `crates/base/tests/keyed_watch.rs`) replay over a network with latency, with roster and
// presence frames between them, to the kit's own final state byte for byte; a lost frame is a
// gap the watch mends with a fresh snapshot; a host without the feature gets nothing; and the
// goldens (`test/golden/keyed_snapshot.json`, `keyed_frames.json`) parse to the types and replay.
//
// Plants caught by name: `GapNotDetected` (`withGapUndetected`), `FeatureNotGated`
// (`withWatchSentAnyway`); and a host's `SnapshotOffByOne` (a change already in the snapshot
// sent after it) is refused here as inconsistent.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { KEYED_WATCH_FEATURE, KeyedWatch, keyedStateJson, type ResyncReason } from "../src/keyed";
import type { ClientFrame, KeyedPageFrame, OpsChangeFrame, ServerFrame, Transport, Welcome } from "../src/wire";
import { Net, rng } from "./fake-server";
import { withGapUndetected, withWatchSentAnyway } from "./plants";

const HERE = dirname(fileURLToPath(import.meta.url));
const load = <T>(...p: string[]): T => JSON.parse(readFileSync(join(HERE, ...p), "utf8")) as T;

type Case = { seed: number; page_bytes: number; frames: ServerFrame[]; late: ServerFrame[]; final_json: string; final_rev: number };
const VECTORS = load<{ feature: string; cases: Case[] }>("vectors", "keyed_watch.json");
type ViewGolden = { frames: ServerFrame[]; final_json: string; units: Record<string, unknown[]> };
type Golden = { frames: ServerFrame[]; final_json: string; final_rev: number; page_bytes?: number; features?: string[]; view?: ViewGolden };
const SNAPSHOT = load<Golden>("golden", "keyed_snapshot.json");
const FRAMES = load<Golden>("golden", "keyed_frames.json");

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

const welcome = (over: Partial<Welcome> = {}): Welcome => ({
  root: "shapes",
  version: 1,
  client: 7,
  self: "ada",
  rights: "read",
  features: [KEYED_WATCH_FEATURE],
  ...over,
});

const watches = (sent: ClientFrame[]) => sent.filter((f) => f.type === "watch").length;
const reports = (sent: ClientFrame[]) => sent.filter((f): f is Extract<ClientFrame, { type: "report" }> => f.type === "report");
const numbered = (f: ServerFrame) => f.type === "keyed_snapshot" || f.type === "keyed_page" || f.type === "ops_change";
const seqOf = (f: ServerFrame) => (f as { seq: number }).seq;

/** Frames that aren't the watch's: they take no number and must not count. */
function noise(r: ReturnType<typeof rng>): ServerFrame {
  switch (r.int(5)) {
    case 0:
      return { type: "roster_event", event: { kind: "selection", who: "own", selection: { root: "shapes", keys: ["a1"] } } };
    case 1:
      return { type: "roster_event", event: { kind: "pointer", who: "own", pointer: { x: 1, y: 2 } } };
    case 2:
      return { type: "presence_refused", req: 3, error: { id: "MPC-E114", message: "bad position" } };
    case 3:
      return { type: "claimed", req: 4, claim: "claim-9" };
    default:
      return { type: "roster_event", event: { kind: "watching", count: 2 } };
  }
}

/** Every frame of `frames` (and noise between them) over a network with random latency, in
 * order, as a WebSocket delivers them. */
function overNet(frames: ServerFrame[], seed: number, deliver: (f: ServerFrame) => void) {
  const net = new Net();
  const r = rng(seed);
  let at = 0;
  for (const f of frames) {
    at += r.int(30);
    const due = at;
    net.at(due, () => deliver(f));
    if (r.int(3) === 0) {
      const n = noise(r);
      at += 1;
      net.at(at, () => deliver(n));
    }
  }
  while (net.step());
}

describe("the kit's traces (test/vectors/keyed_watch.json)", () => {
  it("are there, and every case changes the doc", () => {
    expect(VECTORS.feature).toBe(KEYED_WATCH_FEATURE);
    expect(VECTORS.cases.length).toBeGreaterThanOrEqual(6);
    for (const c of VECTORS.cases) expect(c.frames.filter((f) => f.type === "ops_change").length).toBeGreaterThan(5);
    // small pages in some: a snapshot of many pages
    expect(VECTORS.cases.some((c) => c.frames.filter((f) => f.type === "keyed_page").length > 6)).toBe(true);
    // every kind of change
    const kinds = new Set(VECTORS.cases.flatMap((c) => c.frames.filter((f): f is OpsChangeFrame => f.type === "ops_change").map((f) => f.change_kind)));
    expect([...kinds].sort()).toEqual(["edit", "external", "redo", "restore", "revert", "undo"]);
  });

  for (const c of VECTORS.cases) {
    it(`seed ${c.seed}: replay over a network, with presence between, to the kit's state byte for byte`, () => {
      const { t, sent, deliver } = wire();
      const w = new KeyedWatch(t, welcome(), { reportEvery: 4 });
      const why: ResyncReason[] = [];
      w.onResync((r) => void why.push(r));
      overNet(c.frames, c.seed, deliver);
      expect(w.ready).toBe(true);
      expect(w.stateJson()).toBe(c.final_json);
      expect(w.rev).toBe(c.final_rev);
      expect(why).toEqual([]);
      expect(watches(sent)).toBe(1);
      // the last number is the last numbered frame's: noise counted for nothing
      const last = c.frames.filter(numbered).at(-1)!;
      expect(w.seq).toBe(seqOf(last));
      // each report names a numbered frame the watch had applied, in order
      const rs = reports(sent).map((r) => r.seq);
      expect(rs.length).toBeGreaterThan(1);
      const seqs = new Set(c.frames.filter(numbered).map(seqOf));
      for (const s of rs) expect(seqs.has(s)).toBe(true);
      expect([...rs].sort((a, b) => a - b)).toEqual(rs);
    });

    it(`seed ${c.seed}: a lost change is a gap, mended by a fresh snapshot`, () => {
      const { t, sent, deliver } = wire();
      const w = new KeyedWatch(t, welcome());
      const why: ResyncReason[] = [];
      w.onResync((r) => void why.push(r));
      const changes = c.frames.map((f, i) => [f, i] as const).filter(([f]) => f.type === "ops_change");
      const lost = changes[rng(c.seed).int(changes.length - 1)][1];
      c.frames.forEach((f, i) => i !== lost && deliver(f));
      expect(why).toEqual(["gap"]);
      expect(watches(sent)).toBe(2);
      expect(w.ready).toBe(false);
      for (const f of c.late) deliver(f);
      expect(w.ready).toBe(true);
      expect(w.stateJson()).toBe(c.final_json);
    });

    it(`seed ${c.seed}: the plant GapNotDetected is caught`, () => {
      const run = () => {
        const { t, deliver } = wire();
        const w = new KeyedWatch(t, welcome());
        const why: ResyncReason[] = [];
        w.onResync((r) => void why.push(r));
        const changes = c.frames.map((f, i) => [f, i] as const).filter(([f]) => f.type === "ops_change");
        const lost = changes[rng(c.seed).int(changes.length - 1)][1];
        c.frames.forEach((f, i) => i !== lost && deliver(f));
        return why;
      };
      expect(run()[0]).toBe("gap");
      // under the plant the skipped seq goes by: never seen as a gap
      expect(withGapUndetected(run)).not.toContain("gap");
    });
  }
});

describe("a host without the feature", () => {
  const { features: _, ...noField } = welcome();
  const cases: [string, Welcome][] = [
    ["no `features` field", noField],
    ["an empty list", welcome({ features: [] })],
    ["only other names", welcome({ features: ["presence", "claims", "KEYED_WATCH", "keyed-watch"] })],
  ];
  const everything = (w0: Welcome) => {
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, w0, { reportEvery: 1 });
    w.resync();
    for (const f of FRAMES.frames) deliver(f);
    w.reconnected({ ...w0, client: 8 });
    return { sent, w };
  };
  for (const [why, w0] of cases) {
    it(`${why}: it never receives a frame, and reads none`, () => {
      const { sent, w } = everything(w0);
      expect(w.supported).toBe(false);
      expect(sent).toEqual([]);
      expect(w.stateJson()).toBeNull();
    });
    it(`${why}: the plant FeatureNotGated is caught`, () => {
      expect(withWatchSentAnyway(() => everything(w0).sent.length)).toBeGreaterThan(0);
    });
  }

  it("a later connection that lists it gets the watch", () => {
    const { t, sent } = wire();
    const w = new KeyedWatch(t, noField);
    expect(sent).toEqual([]);
    w.reconnected(welcome({ client: 9 }));
    expect(sent).toEqual([{ type: "watch" }]);
  });
});

describe("numbering", () => {
  it("presence, roster and claim frames take no number", () => {
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, welcome(), { reportEvery: 1 });
    const r = rng(5);
    for (const f of FRAMES.frames) {
      for (let i = 0; i < 3; i++) deliver(noise(r));
      deliver(f);
    }
    expect(w.stateJson()).toBe(FRAMES.final_json);
    // the numbered frames (an `outside_save` takes no number)
    const numbered = FRAMES.frames.filter((f) => "seq" in f);
    expect(w.seq).toBe(seqOf(numbered.at(-1)!));
    // one report a numbered frame applied (a snapshot's whole counts once), never for noise
    const rs = reports(sent).map((x) => x.seq);
    const changes = numbered.filter((f) => f.type === "ops_change").map(seqOf);
    const head = numbered.filter((f) => f.type !== "ops_change").map(seqOf).at(-1)!;
    expect(rs).toEqual([head, ...changes]);
  });

  it("a change already in the snapshot, sent after it (a host's SnapshotOffByOne), is refused as inconsistent", () => {
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, welcome());
    const why: ResyncReason[] = [];
    w.onResync((r) => void why.push(r));
    const snap = FRAMES.frames.filter((f) => f.type !== "ops_change");
    const changes = FRAMES.frames.filter((f): f is OpsChangeFrame => f.type === "ops_change");
    for (const f of snap) deliver(f);
    for (const f of changes.slice(0, 2)) deliver(f);
    const before = w.stateJson();
    // the second change again, numbered next: at or before the state's version
    deliver({ ...changes[1], seq: changes[1].seq + 1 });
    expect(why).toEqual(["inconsistent"]);
    expect(watches(sent)).toBe(2);
    expect(w.stateJson()).toBe(before);
  });

  it("a page out of its snapshot's order asks again; a new snapshot head is always taken", () => {
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, welcome());
    const why: ResyncReason[] = [];
    w.onResync((r) => void why.push(r));
    const pages = SNAPSHOT.frames.filter((f): f is KeyedPageFrame => f.type === "keyed_page");
    expect(pages.length).toBeGreaterThan(2);
    deliver(SNAPSHOT.frames[0]);
    deliver(pages[1]);
    expect(why).toEqual(["gap"]);
    for (const f of SNAPSHOT.frames) deliver(f);
    expect(w.stateJson()).toBe(SNAPSHOT.final_json);
    expect(watches(sent)).toBe(2);
  });

  it("a reconnect numbers from 1 again and asks for a snapshot", () => {
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, welcome());
    for (const f of FRAMES.frames) deliver(f);
    w.reconnected(welcome({ client: 8 }));
    expect(w.seq).toBe(0);
    expect(watches(sent)).toBe(2);
    for (const f of SNAPSHOT.frames) deliver(f);
    expect(w.stateJson()).toBe(SNAPSHOT.final_json);
  });
});

describe("the goldens", () => {
  const keys = (o: object) => Object.keys(o).sort();
  it("keyed_snapshot.json: a head and its pages, under the cap, reassembled", () => {
    const [head, ...pages] = SNAPSHOT.frames;
    expect(head.type).toBe("keyed_snapshot");
    expect(keys(head)).toEqual(["pages", "schema", "seq", "type", "upto", "view"]);
    expect((head as { view: unknown }).view).toBeNull();
    expect(pages.length).toBe((head as { pages: number }).pages);
    for (const [i, p] of (pages as KeyedPageFrame[]).entries()) {
      expect(p.type).toBe("keyed_page");
      expect(p.page).toBe(i + 1);
      const body = ["live", "hidden", "entries", "text"].filter((k) => k in p);
      expect(body.length).toBe(1);
      expect(keys(p)).toEqual(["page", "pages", "root", "seq", "type", "upto", body[0]].sort());
      expect(Buffer.byteLength(JSON.stringify(p))).toBeLessThanOrEqual(SNAPSHOT.page_bytes!);
    }
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome());
    for (const f of SNAPSHOT.frames) deliver(f);
    expect(w.stateJson()).toBe(SNAPSHOT.final_json);
    expect(w.rev).toBe(SNAPSHOT.final_rev);
  });

  it("keyed_snapshot.json's deck: the view index's snapshot in the head, its units in pages, the state by address", () => {
    const g = SNAPSHOT.view!;
    const head = g.frames[0] as { type: string; view: { schema: unknown; containers: { key: string; unit_count: number }[] } };
    expect(head.type).toBe("keyed_snapshot");
    expect(head.view.schema).toBeTruthy();
    const containers = head.view.containers.map((c) => c.key);
    expect(containers.length).toBeGreaterThan(1);
    expect(containers.every((c) => c.startsWith("@pg:"))).toBe(true);
    expect(g.frames.some((f) => f.type === "keyed_page" && "units" in f)).toBe(true);
    const { t, deliver } = wire();
    const w = new KeyedWatch(t, welcome());
    for (const f of g.frames) deliver(f);
    expect(w.stateJson()).toBe(g.final_json);
    expect(w.view).toEqual(head.view);
    for (const c of head.view.containers) {
      expect(w.units(c.key)).toEqual(g.units[c.key] ?? []);
      expect(w.units(c.key).length).toBe(c.unit_count);
    }
    // every element by its address
    expect(g.final_json.includes('"@it:')).toBe(true);
  });

  it("keyed_frames.json: every change frame's fields, and the replay", () => {
    expect(FRAMES.features).toEqual([KEYED_WATCH_FEATURE]);
    const changes = FRAMES.frames.filter((f): f is OpsChangeFrame => f.type === "ops_change");
    const FIELDS = ["actor", "at_ms", "change_kind", "claim", "client", "derived", "on_behalf_of", "ops", "render_hashes", "rev", "seq", "turn", "type", "verb", "via"];
    for (const c of changes) {
      expect(keys(c)).toEqual(FIELDS);
      expect(keys(c.actor)).toEqual(["colour", "id", "kind", "name"]);
      expect(["person", "agent", "app", "file"]).toContain(c.actor.kind);
      for (const o of c.ops) {
        if (o.kind === "map") expect(typeof o.key === "string" && (o.field === null || typeof o.field === "string")).toBe(true);
        if (o.kind === "map" && o.root === "fields") expect(o.key.startsWith("@sh:")).toBe(true);
        if (o.kind === "list") expect(o.key.startsWith("@sh:")).toBe(true);
        if (o.kind === "list") expect(["visible", "hidden"]).toContain(o.effect);
      }
    }
    // what the scenario holds: an agent's turn of verbs under a claim, a file actor, a deleted element's move
    expect(changes.some((c) => c.verb === "shape move" && c.turn !== null && c.claim !== null && c.actor.kind === "agent")).toBe(true);
    expect(changes.some((c) => c.actor.kind === "file" && c.actor.colour === null && c.change_kind === "external")).toBe(true);
    expect(changes.some((c) => c.change_kind === "undo")).toBe(true);
    expect(changes.some((c) => c.ops.some((o) => o.kind === "list" && o.op === "move" && o.effect === "hidden"))).toBe(true);
    expect(changes.some((c) => c.ops.some((o) => o.kind === "list" && o.effect === "hidden"))).toBe(true);
    expect(changes.some((c) => c.ops.some((o) => o.kind === "text"))).toBe(true);
    expect(changes.some((c) => c.ops.some((o) => o.kind === "map" && o.field !== null))).toBe(true);
    expect(changes.some((c) => c.ops.length > 1 && new Set(c.ops.map((o) => o.root)).size > 1)).toBe(true);
    const { t, sent, deliver } = wire();
    const w = new KeyedWatch(t, welcome());
    const seen: OpsChangeFrame[] = [];
    w.onChange((c) => void seen.push(c));
    for (const f of FRAMES.frames) deliver(f);
    expect(w.stateJson()).toBe(FRAMES.final_json);
    expect(w.rev).toBe(FRAMES.final_rev);
    expect(seen.length).toBe(changes.length);
    expect(watches(sent)).toBe(1);
  });

  it("keyedStateJson prints as the kit's writer does (escapes, order by UTF-8 bytes)", () => {
    const state = {
      schema: { name: "x", roots: [{ name: "m", kind: "map" }, { name: "t", kind: "text" }] },
      roots: new Map<string, import("../src/keyed").KeyedRootState>([
        ["m", { kind: "map", entries: new Map([["\u{1F600}", "a"], ["�", null], ["b\u001fc", " \u0001\b"]]) }],
        ["t", { kind: "text", text: "q\"\\\n\r\t\u{e9}" }],
      ]),
    };
    expect(keyedStateJson(state)).toBe(
      '{"roots":[{"name":"m","kind":"map","entries":[["b\\u001fc","\\u2028\\u0001\\u0008"],["�",null],["\u{1F600}","a"]]},{"name":"t","kind":"text","text":"q\\"\\\\\\n\\r\\t\u{e9}"}]}',
    );
  });
});
