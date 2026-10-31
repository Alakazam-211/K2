// The goldens' JSON against `wire.ts`'s types, at compile time. `golden.test.ts` reads each file
// with `JSON.parse` (an untyped value) and checks the values against the spec; this file has tsc
// check the same files against the types, so a type in `wire.ts` that disagrees with what the
// kit encodes (a field typed `null` where the kit sends a number, say) fails `tsc --noEmit`,
// naming the file. Nothing here runs: vitest doesn't pick it up, and it emits nothing.
//
// tsc infers each JSON file's shape with its strings widened to `string`, so each expected type
// is widened the same way (`W`): string literal types become `string`; `null`, numbers,
// booleans and the shape stay. A union (`RosterEvent`) is widened member by member, so an event
// must still match one member's fields.

import type { ChangeEvent, ErrorReport, RosterEvent } from "../src/wire";

import changeEvents from "./golden/change_events.json";
import changeEventsApp from "./golden/change_events_app.json";
import errors from "./golden/errors.json";
import rosterEvents from "./golden/roster_events.json";
import rosterEventsApp from "./golden/roster_events_app.json";
import rosterEventsClosed from "./golden/roster_events_closed.json";
import rosterEventsClosedApp from "./golden/roster_events_closed_app.json";
import rosterEventsOverwritten from "./golden/roster_events_overwritten.json";
import rosterEventsOverwrittenApp from "./golden/roster_events_overwritten_app.json";
import rosterEventsOverwrittenGrid from "./golden/roster_events_overwritten_grid.json";
import rosterEventsRollup from "./golden/roster_events_rollup.json";

/** `T` as tsc infers it from JSON: string literal types widened to `string`. */
type W<T> = T extends string
  ? string
  : T extends readonly (infer U)[]
    ? W<U>[]
    : T extends object
      ? { [K in keyof T]: W<T[K]> }
      : T;

type ChangeGolden = { root: string; version: number; text: string; events: ChangeEvent[]; final_version: number; final_text: string };
type RosterGolden = { self: string; events: RosterEvent[]; final: RosterEvent };
type RosterStream = { self: string; events: RosterEvent[] };
type RollupGolden = { about: string; self: string; saves: number; pushed: number; tail: RosterEvent[]; final: RosterEvent };

// one line a file, so tsc's error names the file whose values contradict the type
changeEvents satisfies W<ChangeGolden>;
changeEventsApp satisfies W<ChangeGolden>;
errors satisfies W<Record<string, ErrorReport>>;
rosterEvents satisfies W<RosterGolden>;
rosterEventsApp satisfies W<RosterGolden>;
rosterEventsClosed satisfies W<RosterStream>;
rosterEventsClosedApp satisfies W<RosterStream>;
rosterEventsOverwritten satisfies W<RosterGolden>;
rosterEventsOverwrittenApp satisfies W<RosterGolden>;
rosterEventsOverwrittenGrid satisfies W<RosterGolden & { about: string }>;
rosterEventsRollup satisfies W<RollupGolden>;
