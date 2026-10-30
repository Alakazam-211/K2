// The hunk channel's frames between a client and the host that runs the kit's session.
// `WIRE.md` is the spec; these types follow it. Offsets are UTF-8 bytes; hunks are canonical
// (sorted, non-overlapping, touching hunks joined) in the coordinates the frame names.
// Field names follow the kit's Rust types (`Hunk`, `ChangeEvent`, `ErrorReport`).

import type { Hunk } from "./hunk";

/** Who made a change or appears in the roster: the host's participant id or opaque handle. */
export type Handle = string;

/** One applied change to one text root (the kit's `ChangeEvent`). */
export interface ChangeEvent {
  /** The doc's version after the change. Consecutive; one change touching several roots is
   * one event per root, all with the same version. */
  version: number;
  pid: Handle;
  client: number;
  via: string;
  root: string;
  /** The change, in the coordinates of the root's text before it. */
  hunks: Hunk[];
  at_ms: number;
  on_behalf_of?: string | null;
}

/** A refusal (the kit's `ErrorReport` JSON: `{id, exit, message, next, rule, details}`). */
export interface ErrorReport {
  /** Stable id, e.g. `MPC-E111`. */
  id: string;
  message: string;
  exit?: number;
  next?: string;
  rule?: string;
  details?: Record<string, unknown>;
}

/** A caret or selection in a root, in UTF-8 bytes of that root's text at `version`
 * (absent: the doc's version when the frame arrives). */
export interface CaretAt {
  root: string;
  anchor: number;
  head: number;
  version?: number;
}

/** A participant as `joined` introduces them (from the kit's `Participant`). */
export interface ParticipantInfo {
  name: string;
  colour: string;
  kind: string;
  via?: string;
}

/**
 * One roster change, mirroring the kit's `RosterEvent` (K5): a snapshot first, then deltas.
 * `who` is the participant's id or opaque handle (`Who::Pid` / `Who::Handle`) as a string.
 */
export type RosterEvent =
  | {
      kind: "snapshot";
      version: number;
      members: RosterEntry[];
      watching?: number;
      /** This subscriber's own waiting overwritten notes; dedupe on (change, row). */
      overwritten?: Extract<RosterEvent, { kind: "overwritten" }>[];
    }
  | { kind: "joined"; who: Handle; participant: ParticipantInfo; rights?: string }
  | { kind: "left"; who: Handle; reason?: string }
  | { kind: "rights"; who: Handle; rights: string }
  | { kind: "role"; who: Handle; role: string }
  | { kind: "away"; who: Handle; away: boolean }
  | { kind: "cursor"; who: Handle; cursor: CaretAt | null }
  | { kind: "claim_started"; who: Handle; claim: string; lines: { first: number; last: number }; version: number; [k: string]: unknown }
  | { kind: "claim_ended"; who: Handle; claim: string; how: string; version: number; [k: string]: unknown }
  | { kind: "note"; to: Handle; from: Handle; [k: string]: unknown }
  | {
      kind: "overwritten";
      to: Handle;
      change: number;
      /** The version holding the text, which this reader can open. Only link from this one. */
      version: number | null;
      /** The same version, for telling someone with history access. Never a link for this reader. */
      restore_from: number | null;
      source: string;
      /** "removed_text" | "replaced_text" | "rollup" */
      rule: string;
      row: string | null;
      line: number | null;
      cells: { region: string; col: number | null; yours: string; file: string; cut: boolean }[];
      more: number;
      not_shown: number;
      withheld: boolean;
      no_history: boolean;
      message: string;
    }
  | { kind: "watching"; count: number }
  /** The host closed this doc's live session (`reason`, its code): the last event; the subscription ends. */
  | { kind: "closed"; reason: string };

/** One participant as the host shows them (from the kit's `RosterEntry`). */
export interface RosterEntry {
  handle: Handle;
  name: string;
  /** Display colour, e.g. `#7c3aed`. */
  colour: string;
  /** `person`, `agent`, `app`... as the host names them. */
  kind: string;
  role: string;
  away: boolean;
  /** The participant's most active caret, if any. */
  cursor?: CaretAt | null;
  /** This connection itself. */
  self?: boolean;
}

/** Frames the client sends. */
export type ClientFrame =
  /** A text change made at `base` (`Session::edit_at_base`); at most one in flight. */
  | { type: "edit"; op: number; root: string; base: number; hunks: Hunk[]; group?: string }
  /** The changes after `version` (`Session::events_since`), to fill a gap. */
  | { type: "since"; op: number; version: number }
  /** This connection's caret, in bytes of `root`'s text at `version`. */
  | { type: "cursor"; root: string; version: number; anchor: number; head: number }
  /** Undo or redo this participant's last change (the server applies and broadcasts it). */
  | { type: "undo"; op: number }
  | { type: "redo"; op: number };

/** Frames the host sends. */
export type ServerFrame =
  /** The reply to `edit`: applied as `hunks` (rebased; on the text at `version - 1`), now at
   * `version`. Nothing applied (already in the text): `hunks` empty, `version` unchanged. */
  | { type: "ack"; op: number; version: number; hunks: Hunk[] }
  /** The reply to `edit`, `since`, `undo` or `redo` when refused; nothing applied. */
  | { type: "refused"; op: number; error: ErrorReport }
  /** A change by anyone else, or by this participant through another path (undo, an
   * agent verb), in server order. */
  | { type: "change"; event: ChangeEvent }
  /** The reply to `since`: every change after the version asked for, in order. */
  | { type: "events"; op: number; events: ChangeEvent[] }
  /** The whole roster (for hosts that fold the deltas themselves). */
  | { type: "roster"; entries: RosterEntry[] }
  /** One roster change as the kit pushes it (`subscribe_roster`): the client folds them. */
  | { type: "roster_event"; event: RosterEvent };

/** Error ids the client acts on. */
export const ERR = {
  STALE: "MPC-E110",
  WRITE_CONFLICT: "MPC-E111",
  BASE_EXPIRED: "MPC-E112",
  BAD_EDIT: "MPC-E113",
  BAD_HUNK: "MPC-E151",
  RATE: "MPC-E901",
} as const;

/** How the host carries frames. The client never assumes anything else about it. */
export interface Transport {
  send(frame: ClientFrame): void;
  /** Frames from the host, in the order it sent them. Returns an unsubscribe. */
  onFrame(cb: (f: ServerFrame) => void): () => void;
}
