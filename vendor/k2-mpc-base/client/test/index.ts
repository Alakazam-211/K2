// The client's test helpers, as a stable entry point (`k2-mpc-client/test`): a fake host and
// server for the live wire, timers on its fake clock, random edits, and the evidence run record.
// For a host's own editor tests (its editor over `LiveText`, against the kit's fake server).
// Test-side only: nothing here ships in a product build.
//
// Each module also stays importable at its own path (`test/fake-live.ts`, `test/fake-server.ts`,
// `test/random-edits.ts`, `test/runrec.ts`), and `test/jsdom-shims.ts` (a side-effect import,
// `k2-mpc-client/test/jsdom-shims`: the Range rectangles jsdom lacks, for an editor mounted in
// jsdom) is not re-exported here, so importing this entry point changes no globals.
// `exports.test.ts` pins these names and paths.

export { FakeLiveHost, FakeLiveTransport, microtasks, netTimers, settle } from "./fake-live";
export { FakeLive, FakeServer, Net, rng } from "./fake-server";
export type { Faults } from "./fake-server";
export { boundaries, randomEdit, randomText } from "./random-edits";
export type { Kind as RandomEditKind } from "./random-edits";
export { runrec } from "./runrec";
