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
 * (absent: the doc's version when the frame arrives). `anchor_lc` / `head_lc` are the kit's
 * `CursorAt` as `"line:col"` (1-based; the column counts characters), for display only. */
export interface CaretAt {
  root: string;
  anchor: number;
  head: number;
  version?: number;
  anchor_lc?: string;
  head_lc?: string;
}

/** A participant as `joined` introduces them (from the kit's `Participant`). */
export interface ParticipantInfo {
  name: string;
  colour: string;
  /** `ParticipantKind` in lower case: `person`, `agent`, `app`, `external`, `restore`, `import`. */
  kind: string;
  via?: string;
}

/** A participant's rights on the live wire: `edit`, `read`, or `none` (kit `Rights`: `edit`
 * when `edit`, else `read` when `read`, else `none`; `history` and `owner` aren't sent). */
export type Rights = "edit" | "read" | "none";

/** Lines `first..=last` (1-based) of a root. */
export interface LineSpan {
  root?: string;
  first: number;
  last: number;
}

/**
 * One roster change, mirroring the kit's `RosterEvent` (K5): a snapshot first, then deltas.
 * `who` is the participant's id or opaque handle (`Who::Pid` / `Who::Handle`) as a string.
 * Notes of kind `note` are passed through untyped: their payload belongs to the kit's notes,
 * which grow by release. `overwritten` (and a snapshot's waiting copies of it) is typed in full.
 */
export type RosterEvent =
  | {
      kind: "snapshot";
      version: number;
      members: RosterEntry[];
      watching?: number;
      /** This subscriber's own waiting overwritten notes; dedupe on (change, row). Present only
       * when there are some: a missing field is none. */
      overwritten?: Extract<RosterEvent, { kind: "overwritten" }>[];
      [k: string]: unknown;
    }
  | { kind: "joined"; who: Handle; participant: ParticipantInfo; rights?: string }
  | { kind: "left"; who: Handle; reason?: string }
  | { kind: "rights"; who: Handle; rights: string }
  | { kind: "role"; who: Handle; role: string }
  | { kind: "away"; who: Handle; away: boolean }
  /** `version`: the doc version the caret's offsets are in (also put inside `cursor`).
   * `client` (kit v0.10.1): the connection whose caret this is (a `Device.client`); absent from
   * an older host, and on the participant's own caret (`RosterEntry.cursor`), which a host
   * sends without it when the last per-connection event wasn't that caret. */
  | { kind: "cursor"; who: Handle; cursor: CaretAt | null; version?: number; client?: number }
  | { kind: "claim_started"; who: Handle; claim: string; lines: LineSpan; version: number; [k: string]: unknown }
  | { kind: "claim_ended"; who: Handle; claim: string; how: string; version: number; [k: string]: unknown }
  | { kind: "note"; to: Handle; from: Handle; [k: string]: unknown }
  | {
      kind: "overwritten";
      to: Handle;
      change: number;
      /** The version holding the live value (a grid) or the text (a text doc, one this reader can open). Only link from this one. */
      version: number | null;
      /** A text doc: the same version, for telling someone with history access. Never a link for this reader. null on a grid. */
      restore_from: number | null;
      /** "an outside save" (a text doc); a grid also "a write" | "a revert" | "a restore" | "rows drop-deleted" */
      source: string;
      /**
       * A text doc: "removed_text" | "replaced_text" | "rollup".
       * A grid: "cell" | "name" | "quoting" | "edited_row" | "deleted_row" | "moved_row" | "dropped_row" | "dropped_col" | "rollup" | the column forms.
       */
      rule: string;
      /** A grid: the row's key; null on a text doc. */
      row: string | null;
      line: number | null;
      cells: { region: string; col: number | null; yours: string; file: string; cut: boolean }[];
      more: number;
      /** A text doc: characters of the reader's own removed text not carried; 0 on a grid. */
      not_shown: number;
      withheld: boolean;
      /** A text doc: the reader has no history right (version is null); false on a grid. */
      no_history: boolean;
      message: string;
    }
  | { kind: "watching"; count: number }
  /** The host closed this doc's live session (`reason`, its code): the last event; the subscription ends. */
  | { kind: "closed"; reason: string };

/** One connection of a participant (the kit's `Device`, v0.10.1): the same person in two tabs
 * or on two devices is one `RosterEntry` with two of these. */
export interface Device {
  /** The connection's client ID (its `welcome.client`). */
  client: number;
  /** This connection's caret, if it has one. */
  cursor: CaretAt | null;
  /** The host's label for the connection ("another tab"), when it gave one. Never doc text. */
  label?: string;
}

/** One participant as the host shows them (from the kit's `RosterMember`). */
export interface RosterEntry {
  /** `RosterMember.who` as a string. */
  handle: Handle;
  name: string;
  /** Display colour, e.g. `#7c3aed`. */
  colour: string;
  /** `person`, `agent`, `app`... (`ParticipantKind` in lower case). */
  kind: string;
  /** `editor` or `viewer`. */
  role: string;
  away: boolean;
  /** The participant's most active caret, if any. */
  cursor?: CaretAt | null;
  /** Each of the participant's connections with its own caret (kit v0.10.1, text docs). Absent
   * from an older host: then `cursor` is the participant's only caret. */
  devices?: Device[];
  /** `RosterMember.rights`, when the host sends it. */
  rights?: Rights;
  /** This connection itself. */
  self?: boolean;
}

/** Why the host closed the connection (`closed`, or the transport's own close). */
export type CloseReason =
  /** The connection left (the tab closed, `close()`). */
  | "left"
  /** The kit's idle expiry (`LeaveReason::Idle`). */
  | "idle"
  /** The host removed the participant (`LeaveReason::Kicked`). */
  | "kicked"
  /** The participant can't read the doc any more (`LeaveReason::RightsLost`). */
  | "rights_lost"
  /** The doc was deleted. */
  | "deleted"
  /** The doc moved (renamed); reopen it at its new key. */
  | "moved"
  /** The doc was handed off to another host. */
  | "released"
  /** The host's queue for this connection overflowed: reconnect and resume. */
  | "cut_off"
  /** Live editing is off for this doc (its pack declined it, or it doesn't fit one frame). */
  | "declined"
  /** No such doc. */
  | "not_found"
  /** The socket's authentication failed or expired. */
  | "auth"
  /** The network dropped (the transport's own reason; never sent in a frame). */
  | "network"
  /** The host closed the doc's live session (`SessionManager::close`). */
  | "closed";

/** The first frame the host sends on a connection: after its own authentication, the kit's
 * `join`, `subscribe`, `subscribe_roster`, then `read`. */
export interface Welcome {
  /** The text root this connection edits (`source` for Markdown). */
  root: string;
  /** The version `text` is at, or the doc's version now (a resume). */
  version: number;
  /** The root's text: on a first connect, or when resuming was impossible. Absent on a resume. */
  text?: string;
  /** This connection's client ID (`Joined.client`): new on every connection. */
  client: number;
  /** This participant as the roster names them (pid, or handle for an App viewer). */
  self: Handle;
  rights: Rights;
}

/** Frames the client sends. */
export type ClientFrame =
  /** Optional first frame, when the host doesn't take these in its connect request. */
  | { type: "hello"; root: string; resume?: number }
  /** A text change made at `base` (`Session::edit_at_base`); at most one in flight. */
  | { type: "edit"; op: number; root: string; base: number; hunks: Hunk[]; group?: string }
  /** The changes after `version` (`Session::events_since`), to fill a gap. */
  | { type: "since"; op: number; version: number }
  /** The root's whole text now (`Session::read`), after `onReload`. */
  | { type: "snapshot"; op: number }
  /** This connection's caret, in bytes of `root`'s text at `version`. No reply. */
  | { type: "cursor"; root: string; version: number; anchor: number; head: number }
  /** Undo or redo this participant's last change (the server applies and broadcasts it). */
  | { type: "undo"; op: number }
  | { type: "redo"; op: number }
  /** Whether this connection's view is in front (`Session::set_visible`). No reply. */
  | { type: "visible"; visible: boolean }
  /** The lines this connection shows (`Session::set_view`), 1-based inclusive. No reply. */
  | { type: "view"; root?: string; first: number; last: number };

/** Frames the host sends. */
export type ServerFrame =
  /** The first frame on a connection. */
  | ({ type: "welcome" } & Welcome)
  /** The reply to `edit`: applied as `hunks` (rebased; on the text at `version - 1`), now at
   * `version`. Nothing applied (already in the text): `hunks` empty, `version` unchanged. */
  | { type: "ack"; op: number; version: number; hunks: Hunk[] }
  /** The reply to `edit`, `since`, `snapshot`, `undo` or `redo` when refused; nothing applied. */
  | { type: "refused"; op: number; error: ErrorReport }
  /** A change by anyone else, or by this participant through another path (undo, an
   * agent verb), in server order. */
  | { type: "change"; event: ChangeEvent }
  /** The reply to `since`: every change after the version asked for, in order. */
  | { type: "events"; op: number; events: ChangeEvent[] }
  /** The reply to `snapshot`: the root's text at `version`. */
  | { type: "snapshot"; op: number; text: string; version: number }
  /** The reply to `undo` or `redo`, after the `change` frames it made: `steps` undone (0:
   * nothing to undo or redo), the doc now at `version`. `notice`: why there was nothing, when
   * the kit says (`MPC-E206` history lost, `MPC-E207` cleared by a purge). */
  | { type: "undone"; op: number; steps: number; version: number; notice?: ErrorReport }
  /** The whole roster (for hosts that fold the deltas themselves). */
  | { type: "roster"; entries: RosterEntry[] }
  /** One roster change as the kit pushes it (`subscribe_roster`): the client folds them. */
  | { type: "roster_event"; event: RosterEvent }
  /** A problem with a frame that has no `op` to answer (a frame over the cap, unreadable JSON). */
  | { type: "error"; error: ErrorReport }
  /** The last frame on a connection; the host then closes the socket. */
  | {
      type: "closed";
      reason: CloseReason;
      /** A host-defined refinement of `reason` (e.g. `core_off`); never changes whether to retry. */
      code?: string;
      /** With `moved`: the doc's new path, when the host can say it. */
      path?: string;
      error?: ErrorReport;
    };

/** Error ids the client acts on. */
export const ERR = {
  STALE: "MPC-E110",
  WRITE_CONFLICT: "MPC-E111",
  BASE_EXPIRED: "MPC-E112",
  BAD_EDIT: "MPC-E113",
  BAD_POSITION: "MPC-E114",
  BAD_HUNK: "MPC-E151",
  HISTORY_LOST: "MPC-E206",
  CLEARED_BY_PURGE: "MPC-E207",
  TOO_LARGE: "MPC-E401",
  RATE: "MPC-E901",
  /** The client's own: an edit whose reply was lost and that a `since` reply doesn't list. */
  NOT_APPLIED: "MPC-E000",
} as const;

/** The largest frame either side sends or accepts: 4 MiB of JSON (UTF-8 bytes). */
export const FRAME_MAX = 4 * 1024 * 1024;
/** Room the client leaves under `FRAME_MAX` for an edit, so its `ack` (the same hunks, moved,
 * and a few fields) fits too. */
export const FRAME_MARGIN = 64 * 1024;

/** How the host carries frames: all `LiveText` needs. One stable object for the replica's
 * life; a host that reconnects swaps the socket inside it. */
export interface Transport {
  /** Send a frame. While no socket is open the frame is dropped, never buffered for the next
   * socket: the wire has no idempotency key, and a replayed `edit` could apply twice. */
  send(frame: ClientFrame): void;
  /** Frames from the host, in the order it sent them. Returns an unsubscribe. */
  onFrame(cb: (f: ServerFrame) => void): () => void;
}

/** How a connection ended (`LiveTransport.onClose`). */
export interface CloseEvent {
  reason: CloseReason;
  /** Whether reconnecting can help (`network`, `cut_off`: yes; the others: no). It must agree
   * with `canRetry(reason)`; the live editor goes by `canRetry` and reports a disagreement. */
  retry: boolean;
  /** The `closed` frame's `code`, when there was one. */
  code?: string;
  /** The `closed` frame's `path` (with `moved`), when there was one. */
  path?: string;
  error?: ErrorReport;
}

/** One connection attempt's options (`LiveTransport.connect`). */
export interface ConnectOptions {
  /** The version the client has: the host sends no `text` when it can resume from it. */
  resume?: number;
  /**
   * The client ID of this client's previous connection (the last welcome's `client`), sent on
   * every reconnect (the `prev` query parameter of the connect request). The host leaves that
   * connection before it welcomes this one, when it is the same login's, on the same doc and
   * root, on this host; otherwise it ignores `prev` silently (WIRE.md, Reconnecting).
   */
  prev?: number;
  signal: AbortSignal;
}

/**
 * The transport a host implements for a live editor component: `Transport`, plus one
 * connection attempt and its end. The component owns the retry loop (back off, then
 * `connect({resume: live.version})`, then `live.reconnected(welcome)`).
 */
export interface LiveTransport extends Transport {
  /** One attempt: open the socket, authenticate (the host's; the component never sees a
   * token), join, and resolve with the `welcome` frame. `resume`: the version the client has;
   * the host then sends no `text`. Rejects (with a `CloseEvent`) when the attempt fails. */
  connect(opts: ConnectOptions): Promise<Welcome>;
  /** The connection ended: a `closed` frame, or the socket dropped (`network`). */
  onClose(cb: (e: CloseEvent) => void): () => void;
  /** The tab closed: close the socket (the host calls the kit's `leave`). */
  close(): void;
}

/** Whether a close reason allows a reconnect. */
export function canRetry(reason: CloseReason): boolean {
  return reason === "network" || reason === "cut_off";
}
