// Planted faults for the library's own rules, each a spy on `LiveText`'s internals that a test
// turns on around one run: the Re-apply placement (M1's region rule), Re-apply's presence check,
// own-change recognition by hunks alone, and the kind rule for a lost change's landing. A test
// passes with the real code and must fail under its fault ("plant caught"). Test-side only; the
// DOM-free host driver (`live-host.ts`) and the library tests use them.

import { vi } from "vitest";
import { gridRead } from "../src/grid";
import { coalesce, type Hunk } from "../src/hunk";
import { KeyedWatch } from "../src/keyed";
import { KeyedView } from "../src/view";
import { LiveText } from "../src/live";
import { PresenceChannel } from "../src/presence";
import { Carried, rebaseOurs } from "../src/rebase";
import type { ChangeEvent, ClaimActor, RosterEvent } from "../src/wire";
import type { FakeServer } from "./fake-server";

/**
 * Run `f` with Re-apply's check planted out: `presence` always says `absent` and a late landing
 * isn't recognised, so a change that landed after all is put in again (the fault the client
 * guard prevents).
 */
export async function withoutPresenceCheck<T>(f: () => Promise<T>): Promise<T> {
  const spy = vi.spyOn(LiveText.prototype, "presence").mockReturnValue("absent");
  // and a late landing goes unrecognised
  const landed = vi.spyOn(LiveText.prototype as unknown as { checkLanded(): void }, "checkLanded").mockImplementation(() => {});
  try {
    return await f();
  } finally {
    spy.mockRestore();
    landed.mockRestore();
  }
}

/**
 * Where the server's rule puts rejection `id`, from the fake server's own log: its change as sent
 * (the replica's record of the `edit` frame) rebased with `rebaseOurs` through the log since its
 * base, then through the replica's pending changes. `null`: the rule refuses it (a conflict, or
 * no sent form). Independent of the replica's own history.
 */
export function serverPlacement(live: LiveText, server: FakeServer, id: number): Hunk[] | null {
  type I = { sentOf: Map<number, { hunks: Hunk[]; base: number }>; inflight: { hunks: Hunk[]; doomed?: unknown } | null; queue: Array<{ hunks: Hunk[] } | { held: string }> };
  const x = live as unknown as I;
  const sent = x.sentOf.get(id);
  if (!sent) return null;
  const log = server.log.filter((a) => a.version > sent.base && a.version <= live.version).map((a) => a.hunks);
  const r = rebaseOurs(sent.hunks, log);
  if ("conflict" in r) return null;
  let c = Carried.of(r.ok);
  // the replica's pending edits (an undo or redo held among them is no edit)
  const queued = x.queue.filter((e): e is { hunks: Hunk[] } => !("held" in e));
  for (const e of [...(x.inflight && !x.inflight.doomed ? [x.inflight] : []), ...queued]) {
    const t = c.through(e.hunks, 0);
    if ("conflict" in t) return null;
    c = t.ok;
  }
  return c.hunks();
}

/**
 * Run `f` with Re-apply placing changes as M1 did (the planted fault for the rule): all of a
 * rejection's typed text at its region's start, deletions never redone.
 */
export async function withRegionPlacement<T>(f: () => Promise<T>): Promise<T> {
  const spy = vi.spyOn(LiveText.prototype, "placeRejection").mockImplementation(function (this: LiveText, id: number) {
    const r = this.rejected.find((x) => x.id === id);
    if (!r) return null;
    return { hunks: r.text ? [{ start: r.region.start, end: r.region.start, insert: r.text }] : [] };
  });
  try {
    return await f();
  } finally {
    spy.mockRestore();
  }
}

/**
 * Run `f` with LiveText recognising its own change in a `since` reply by its hunks alone, as it
 * did before client IDs (the planted fault seed 51 caught: someone else's identical change
 * taken for its own).
 */
export async function withHunkOnlyRecognition<T>(f: () => Promise<T>): Promise<T> {
  type E = { op?: number; hunks: Hunk[] };
  type L = { stash: { op: number; version: number; hunks: Hunk[] } | null };
  const spy = vi.spyOn(LiveText.prototype as unknown as { isOwn(ev: ChangeEvent, e: E): boolean }, "isOwn");
  spy.mockImplementation(function (this: unknown, ev: ChangeEvent, e: E) {
    const self = this as L;
    if (self.stash && self.stash.op === e.op) return self.stash.hunks.length > 0 && ev.version === self.stash.version;
    const hs = coalesce(ev.hunks);
    return hs.length === e.hunks.length && hs.every((h, i) => h.start === e.hunks[i].start && h.end === e.hunks[i].end && h.insert === e.hunks[i].insert);
  });
  try {
    return await f();
  } finally {
    spy.mockRestore();
  }
}

/** How LiveText reads a change's `kind` for a lost change's landing (its private `kindOf`). */
export type KindClass = "edit" | "undo" | "other" | "none";
type KindFault = (real: KindClass) => KindClass;

/** Planted faults for the kind rule: an undo or redo read as the lost edit. */
export const UNDO_AS_EDIT: KindFault = (k) => (k === "undo" ? "edit" : k);
/** Every kind ignored: the rule before kinds (an older kit's), whatever the host sends. */
export const KIND_IGNORED: KindFault = () => "none";
/** A revert, restore, outside save or import under the client's own ID read as an undo. */
export const OTHER_AS_UNDO: KindFault = (k) => (k === "other" ? "undo" : k);
/** A change without a kind, beside ones with, read as an undo (the mixed case not kept
 * conservative). */
export const NONE_AS_UNDO: KindFault = (k) => (k === "none" ? "undo" : k);

/** Run `f` (sync) with LiveText reading kinds through `fault`. */
export function kindsAs<T>(fault: KindFault, f: () => T): T {
  const P = LiveText.prototype as unknown as { kindOf(k: unknown): KindClass };
  const orig = P.kindOf;
  const spy = vi.spyOn(P, "kindOf").mockImplementation(function (this: unknown, k: unknown) {
    return fault(orig.call(this, k));
  });
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` (async) with LiveText reading kinds through `fault`. */
export async function withKindsAs<T>(fault: KindFault, f: () => Promise<T>): Promise<T> {
  const P = LiveText.prototype as unknown as { kindOf(k: unknown): KindClass };
  const orig = P.kindOf;
  const spy = vi.spyOn(P, "kindOf").mockImplementation(function (this: unknown, k: unknown) {
    return fault(orig.call(this, k));
  });
  try {
    return await f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the claims feature check planted out: a `PresenceChannel` sends `claim` and
 * `release` to a host that didn't list `"claims"`. */
export function withClaimsSentAnyway<T>(f: () => T): T {
  const spy = vi.spyOn(PresenceChannel.prototype, "claimsSupported", "get").mockReturnValue(true);
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the feature check planted out: a `PresenceChannel` sends its frames to a host
 * that didn't list `"presence"` (the fault `welcome.features` prevents). */
export function withPresenceSentAnyway<T>(f: () => T): T {
  const spy = vi.spyOn(PresenceChannel.prototype, "supported", "get").mockReturnValue(true);
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the lease cap planted out for a writer given back unkept (`SameWriterUncapped`):
 * a reconnect that gets its old writer again, not kept, caps nothing, so a later lease's change
 * under that writer can be read as this client's own. */
export function withSameWriterUncapped<T>(f: () => T): T {
  type C = { capsOldWriter(old: number, client: number, lease: string | undefined): boolean };
  const spy = vi
    .spyOn(LiveText.prototype as unknown as C, "capsOldWriter")
    .mockImplementation((old, client, lease) => lease !== undefined && old !== client);
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the keyed watch's gap check planted out (`GapNotDetected`): a `KeyedWatch` takes
 * a frame after a skipped `seq` as the next one, and asks for no snapshot. */
export function withGapUndetected<T>(f: () => T): T {
  const spy = vi.spyOn(KeyedWatch.prototype, "inOrder").mockReturnValue(true);
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the keyed watch's feature check planted out (`FeatureNotGated`): a `KeyedWatch`
 * sends `watch` and `report` to a host that didn't list `"keyed_watch"`. */
export function withWatchSentAnyway<T>(f: () => T): T {
  const spy = vi.spyOn(KeyedWatch.prototype, "supported", "get").mockReturnValue(true);
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the grid's numbering ignoring each row's and column's gap (`GridGapIgnored`):
 * every gap reads as 0, so the numbers after a blank row come out one short. */
export function withGridGapIgnored<T>(f: () => T): T {
  const spy = vi.spyOn(gridRead, "gap").mockReturnValue(0);
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with a claim on cells not taking its actor from its event
 * (`ClaimCellsActorMissing`): its `ClaimView` names only the claimant's id, with no name, kind or
 * colour, as if the event carried none and the roster had no row. */
export function withClaimCellsActorMissing<T>(f: () => T): T {
  type E = Extract<RosterEvent, { kind: "claim_started_cells" }>;
  const P = PresenceChannel.prototype as unknown as { cellsClaimActor(e: E): ClaimActor };
  const spy = vi.spyOn(P, "cellsClaimActor").mockImplementation((e: E) => ({ id: e.who, name: "", kind: "", colour: "" }));
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}

/** Run `f` with the view's container deltas planted out (`ClientViewDrift`): a `KeyedView`
 * skips every `containers_changed` entry, so its containers drift from the doc's. */
export function withContainerDeltaSkipped<T>(f: () => T): T {
  const spy = vi.spyOn(KeyedView.prototype, "containerDelta").mockImplementation(() => {});
  try {
    return f();
  } finally {
    spy.mockRestore();
  }
}
