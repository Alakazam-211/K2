// The roster events the editor reads by name: the kit's removed-text note (the `overwritten`
// roster event of the kit's outside-removal notes, v0.9.7.2 / v0.9.8.1; `wire.ts` types it in
// full too, and test/golden.test.ts holds both to the kit's JSON) and claims.

import type { Handle } from "../wire";

/**
 * The kit's note that an outside save may have removed or replaced text this participant
 * wrote (the kit's roster event of kind `overwritten`, `NoteKind::Overwritten`). It goes only
 * to its author; the editor also drops any whose `to` isn't this connection's participant.
 */
export interface RemovedTextNote {
  kind: "overwritten";
  to: Handle;
  /** The outside save's change number. */
  change: number;
  /** The version holding the text that this reader can open; only link from this one. */
  version: number | null;
  /** The same version, to tell someone with history access; never a link for this reader. */
  restore_from: number | null;
  source: string;
  /** `removed_text`, `replaced_text` or `rollup`. */
  rule: "removed_text" | "replaced_text" | "rollup" | (string & {});
  row: string | null;
  line: number | null;
  cells: Array<{ region: string; col: number | null; yours: string; file: string; cut: boolean }>;
  more: number;
  not_shown: number;
  withheld: boolean;
  no_history: boolean;
  /** The note's one line, ready to show. It holds the person's text: don't log it. */
  message: string;
}

/** A claim starting or ending on lines of the doc (roster events `claim_started` / `claim_ended`). */
export type ClaimEvent =
  | { kind: "claim_started"; who: Handle; claim: string; lines: { first: number; last: number }; version: number }
  | { kind: "claim_ended"; who: Handle; claim: string; how: string; version: number };
