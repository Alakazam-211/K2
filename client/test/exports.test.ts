// What a host builds against stays put. A host that draws its own editor imports the library's
// names from the package root, a few modules and test helpers by path, and reads some test
// files by exact path from its own build (golden files a host's tests compare its frames with).
// Removing or renaming any of them breaks that host, so each is listed here:
//
// - names: compile-time (`npx tsc --noEmit` fails when one leaves `src/index.ts` or a helper
//   module), and at run time for the values;
// - files a host build reads by path: each must exist under that exact name (case included).
//   A planted rename must be caught.

import { cpSync, mkdtempSync, readdirSync, renameSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import * as lib from "../src/index";
import type * as Lib from "../src/index";
import * as core from "../src/core";
import { setDefaultLiveTimers } from "../src/live";
import * as helpers from "./index";
import type * as Helpers from "./index";
import { FakeLiveHost, microtasks, netTimers, settle, type FakeLiveTransport } from "./fake-live";
import { FakeServer, Net, rng } from "./fake-server";
import { randomEdit, randomText } from "./random-edits";
import { runrec } from "./runrec";
import "./jsdom-shims";

/** Values a host imports from the package root. */
const VALUES = [
  "Bias",
  "CLAIMS_FEATURE",
  "CLAIM_KEYS_MAX",
  "CLAIM_NOTE_MAX",
  "Carried",
  "ClaimRejected",
  "ELEM_SELECTION_MAX",
  "LiveText",
  "PRESENCE_FEATURE",
  "PresenceChannel",
  "apply",
  "byteToIndex",
  "canRetry",
  "coalesce",
  "indexToByte",
  "rebaseOurs",
  "setDefaultLiveTimers",
  "transform",
  "utf8Len",
] as const satisfies readonly (keyof typeof lib)[];

/** Types a host imports from the package root: each must resolve (a compile error if not). */
export type HostTypes = [
  Lib.ChangeEvent,
  Lib.ClaimOptions,
  Lib.ClaimRejectedReason,
  Lib.ClientFrame,
  Lib.CloseEvent,
  Lib.CloseReason,
  Lib.ErrorReport,
  Lib.Handle,
  Lib.Hunk,
  Lib.LiveText,
  Lib.LiveTransport,
  Lib.PresenceChannel,
  Lib.Rejection,
  Lib.RemoteCaret,
  Lib.Rights,
  Lib.RosterEntry,
  Lib.RosterEvent,
  Lib.ServerFrame,
  Lib.Transport,
  Lib.Welcome,
  // presence and marks as data (PRESENCE.md), what a host's editor draws from
  Lib.Participant,
  Lib.Mark,
  Lib.InsertHereResult,
  Lib.LiveOptions,
  Lib.LiveTimers,
];

/** Test helpers a host imports (from `k2-mpc-client/test`, or each module by path). */
const HELPERS = [
  "FakeLiveHost",
  "FakeLiveTransport",
  "microtasks",
  "netTimers",
  "settle",
  "FakeServer",
  "Net",
  "rng",
  "randomEdit",
  "randomText",
  "runrec",
] as const satisfies readonly (keyof typeof helpers)[];
export type HelperTypes = [Helpers.FakeLiveTransport, Helpers.Faults, Helpers.RandomEditKind, FakeLiveTransport];

/** Files a host build reads by exact path, relative to `client/`. */
const BY_PATH = [
  "src/index.ts",
  "src/core.ts",
  "src/live.ts",
  "src/wire.ts",
  "test/index.ts",
  "test/jsdom-shims.ts",
  "test/fake-live.ts",
  "test/fake-server.ts",
  "test/random-edits.ts",
  "test/runrec.ts",
  "test/golden/change_events.json",
  "test/golden/change_events_app.json",
  "test/golden/errors.json",
  "test/golden/roster_events.json",
  "test/golden/roster_events_app.json",
  "test/golden/roster_events_closed.json",
  "test/golden/roster_events_closed_app.json",
  "test/golden/roster_events_devices.json",
  "test/golden/roster_events_overwritten.json",
  "test/golden/roster_events_overwritten_app.json",
  "test/golden/roster_events_rollup.json",
];

const CLIENT = join(dirname(fileURLToPath(import.meta.url)), "..");

/** The paths in `paths` missing from `root`, compared by exact name, directory by directory
 * (so a rename that only changes case is caught on a case-insensitive disk too). */
function missingIn(root: string, paths: readonly string[]): string[] {
  const listed = new Map<string, Set<string>>();
  const names = (dir: string) => {
    let s = listed.get(dir);
    if (!s) {
      try {
        s = new Set(readdirSync(join(root, dir)));
      } catch {
        s = new Set();
      }
      listed.set(dir, s);
    }
    return s;
  };
  return paths.filter((p) => {
    const parts = p.split("/");
    for (let i = 0; i < parts.length; i++) if (!names(parts.slice(0, i).join("/")).has(parts[i])) return true;
    return false;
  });
}

/** The names in `want` that `mod` doesn't export as a defined value. */
const missingValues = (mod: object, want: readonly string[]) => want.filter((n) => (mod as unknown as Record<string, unknown>)[n] === undefined);

describe("what a host builds against", () => {
  it("every value a host imports is exported from the package root and from core", () => {
    expect(missingValues(lib, VALUES)).toEqual([]);
    expect(missingValues(core, VALUES)).toEqual([]);
    expect(lib.setDefaultLiveTimers).toBe(setDefaultLiveTimers);
  });

  it("the test helpers: from the test entry point, and the same objects at their own paths", () => {
    expect(missingValues(helpers, HELPERS)).toEqual([]);
    const byPath: Record<string, unknown> = { FakeLiveHost, microtasks, netTimers, settle, FakeServer, Net, rng, randomEdit, randomText, runrec };
    for (const [n, v] of Object.entries(byPath)) expect((helpers as unknown as Record<string, unknown>)[n], n).toBe(v);
  });

  it("every file a host build reads by path is there, under that exact name", () => {
    expect(missingIn(CLIENT, BY_PATH)).toEqual([]);
  });

  it("planted (a golden renamed, a helper's case changed, an export gone): caught", () => {
    const tmp = mkdtempSync(join(tmpdir(), "mpc-paths-"));
    try {
      cpSync(join(CLIENT, "test"), join(tmp, "test"), { recursive: true });
      cpSync(join(CLIENT, "src"), join(tmp, "src"), { recursive: true });
      expect(missingIn(tmp, BY_PATH), "the copy differs before the plant").toEqual([]);
      renameSync(join(tmp, "test/golden/roster_events_devices.json"), join(tmp, "test/golden/roster_events_device.json"));
      renameSync(join(tmp, "test/fake-live.ts"), join(tmp, "test/Fake-live.ts"));
      expect(missingIn(tmp, BY_PATH), "plant missed").toEqual(["test/fake-live.ts", "test/golden/roster_events_devices.json"]);
    } finally {
      rmSync(tmp, { recursive: true, force: true });
    }
    const without = Object.fromEntries(Object.entries(lib).filter(([k]) => k !== "canRetry"));
    expect(missingValues(without, VALUES), "plant missed").toEqual(["canRetry"]);
  });
});
