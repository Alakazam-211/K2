// The hunk channel's frames between a client and the host that runs the kit's session.
// `WIRE.md` is the spec; these types follow it. Offsets are UTF-8 bytes; hunks are canonical
// (sorted, non-overlapping, touching hunks joined) in the coordinates the frame names.
// Field names follow the kit's Rust types (`Hunk`, `ChangeEvent`, `ErrorReport`).

import type { Hunk } from "./hunk";

/** Who made a change or appears in the roster: the host's participant id or opaque handle. */
export type Handle = string;

/** The kit's change kinds (`ChangeKind::as_str`), as `ChangeEvent.kind` sends them. */
export type ChangeKind = "edit" | "undo" | "redo" | "revert" | "restore" | "external" | "import";

/** UTF-8 byte offsets in a root's text, half-open: `start..end`. */
export interface ByteSpan {
  start: number;
  end: number;
}

/** Lines `first..=last` (1-based). */
export interface LinePair {
  first: number;
  last: number;
}

/** Where an outside save's merge kept both sides (the kit's `KeptPair`): each side's lines in
 * the root's text after the change. The merged text holds only the two sides' own lines there
 * (no marker or separator), in the merge's order: the live side first at one point, but each
 * side's text where its own change starts, so the file's lines can come first and one side's
 * range can enclose the other's. Use the ranges; never assume an order. */
export interface KeptPair {
  /** The live side's lines (what the doc's participants had). */
  live: LinePair;
  /** The outside save's lines (the file's). */
  file: LinePair;
}

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
  /** What kind of change it is (the kit's `ChangeKind`, kit v0.10.1; absent from an older
   * host). An undo and a redo say so here. */
  kind?: ChangeKind;
  /** The host-given turn of the agent step that made it:
   * opaque, at most 64 bytes, compared only for equality, to group one agent run's changes.
   * `null` for a person's change, an outside save, and any call the host gave none; absent
   * from a host before wire 3. An App viewer always gets `null`. */
  turn?: string | null;
  /** The id of the pack verb that made it, as the pack names it
   * (`shape move`, `replace-section`), at most 64 bytes. `null` for a raw write, a kit tool
   * (`sed -i`, `append`), an outside save, an undo, a redo, a revert, a restore and the import;
   * absent from a host before it (wire 3, kit after v0.14.0). `via` stays the client (`cli`),
   * never the verb. */
  verb?: string | null;
  /** On an outside save's change (`kind: "external"`) whose merge kept both sides: where, one
   * pair per region, in order. Absent from every other change, from a clean save, and from a
   * host before it (kit after v0.20.0). */
  kept_pairs?: KeptPair[];
}

/** What to run next, as data (the kit's `NextStep`): a kit verb by its registry
 * name, its arguments one argv token each (`{key}` stands for the refusal's `details.key`),
 * and why, in prose. No host syntax: the host (or its UI) renders it in its own. */
export interface NextStep {
  verb: string;
  args: string[];
  why: string;
}

/** A refusal (the kit's `ErrorReport` JSON: `{id, exit, message, next, next_step, rule,
 * details}`). `next_step` is absent when there is nothing to run, and from an older host
 * (before kit v0.10.1). */
export interface ErrorReport {
  /** Stable id, e.g. `MPC-E111`. */
  id: string;
  message: string;
  exit?: number;
  /** What to do next, in prose for people. */
  next?: string;
  /** `next` as data ([NextStep]); absent when there is nothing to run. */
  next_step?: NextStep;
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

/** Elements selected by identity: keys of one list root. Keys, not positions, so a selection
 * follows moves with no mapping and carries no version. */
export interface ElemSelection {
  root: string;
  /** At most 256, no repeats (more is refused, `MPC-E310`). */
  keys: string[];
}

/** The version of `WIRE.md` this client speaks ("Wire versions"). A `welcome` without `wire` is
 * from a wire-2 host. */
export const WIRE_VERSION = 3;

/** The container a connection has in view (wire 3; the kit's `ElemFocus`): an element of a list
 * root, by key (a slide). */
export interface ElemFocus {
  root: string;
  key: string;
}

/** A caret or selection inside one element's text field (wire 3; the kit's `ElemRange`): `field`
 * of element `key` of `root`. `anchor` and `head` are **UTF-8 bytes** of the field's text, as on
 * every offset on the wire (`rangeToUtf16` converts). The kit drops it when the field changes. */
export interface ElemRange {
  root: string;
  key: string;
  field: string;
  anchor: number;
  head: number;
}

/** A place in the doc's own coordinates; finite numbers. The kit doesn't interpret them. */
export interface Point {
  x: number;
  y: number;
}

/** An area in the doc's own coordinates: finite numbers, `w` and `h` not negative. */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** A live claim on elements of a list root, as a roster entry names it (`elem_claims`). */
export interface ElemClaim {
  /** `claim-N`. */
  claim: string;
  root: string;
  /** In key order, at most 256. */
  keys: string[];
  note: string | null;
  hold: boolean;
  expires_ms: number;
  /** When it was taken, on the host's clock (wire 3; absent from an older host). */
  started_ms?: number;
}

/**
 * One corner of a cell range in a live grid (WIRE.md, "The live grid"), by address: `row` is the
 * last keyed row of the container at or before the cell (`null` when there is none), `row_off`
 * how many rows past it (with `row: null`, the row number itself, so at least 1); `col` and
 * `col_off` the same for columns. In normal form, always: no keyed row (column) of the container
 * lies strictly between `row` and the cell. A corner is never moved; the kit drops it when its
 * row or column stops being live or its form breaks.
 */
export interface Corner {
  row: string | null;
  row_off: number;
  col: string | null;
  col_off: number;
}

/** A block of cells from `start` to `end`; either may be above or left of the other. */
export interface CellRange {
  start: Corner;
  end: Corner;
}

/** A connection's cells in one container (a sheet): its active cell (inside a range when there
 * are ranges) and at most 16 ranges (`CELLS_RANGES_MAX`). */
export interface Cells {
  container: string;
  active: Corner | null;
  ranges: CellRange[];
}

/** A live claim on a block of cells, as a roster entry names it (`cell_claims`). */
export interface CellClaim {
  /** `claim-N`. */
  claim: string;
  container: string;
  /** One range. */
  ranges: CellRange[];
  note: string | null;
  hold: boolean;
  expires_ms: number;
  /** When it was taken, on the host's clock. */
  started_ms: number;
}

/** Who holds a claim, as its events carry it (wire 3; the kit's `ClaimActor`, with `id` the
 * event's `who`), so a view needs no roster lookup, even after the claimant left. */
export interface ClaimActor {
  id: Handle;
  name: string;
  /** `ParticipantKind` in lower case (`person`, `agent`, ...). */
  kind: string;
  colour: string;
}

/**
 * One roster change, mirroring the kit's `RosterEvent`: a snapshot first, then deltas.
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
  /** A connection's selection, pointer or viewport (`null`: gone), coalesced to the last value
   * per flush. `client`: the connection (a `Device.client`), as for `cursor`. Outside the doc's
   * numbering: none of these takes or moves a version. */
  | { kind: "selection"; who: Handle; selection: ElemSelection | null; client?: number }
  | { kind: "pointer"; who: Handle; pointer: Point | null; client?: number }
  | { kind: "viewport"; who: Handle; viewport: Rect | null; client?: number }
  /** A connection's focus (the container in view) or text range (wire 3), as `selection`. A
   * range goes (`null`) when its field changed or its element went: never moved. */
  | { kind: "focus"; who: Handle; focus: ElemFocus | null; client?: number }
  | { kind: "range"; who: Handle; range: ElemRange | null; client?: number }
  /** A connection's cells in a live grid (feature `cells`), as `selection`. `null`: gone (cleared,
   * its focus moved to another container, or nothing of it is left after a row or column went). */
  | { kind: "cells"; who: Handle; cells: Cells | null; client?: number }
  /** A connection's view address (the view it is looking at) or time position (its playhead), as
   * `selection` (kit after v0.18.0; an older host sends none). */
  | { kind: "view_address"; who: Handle; view_address: string | null; client?: number }
  | { kind: "time_position"; who: Handle; time_position: TimePositionJson | null; client?: number }
  /** `actor` and `started_ms`: wire 3, absent from an older host. */
  /** `bytes` (kit after v0.20.0): the claim's span when it was taken, in UTF-8 bytes of its
   * root's text at `version` (the doc text as hunks count it: `\n` line breaks, no BOM, a CRLF
   * file's `\r` not in it). Absent from an older host and for a grid's line claim. */
  | { kind: "claim_started"; who: Handle; claim: string; lines: LineSpan; bytes?: ByteSpan; version: number; actor?: ClaimActor; started_ms?: number; [k: string]: unknown }
  /** A claim on elements of a list root: it has no lines, so it has its own kind. It ends with
   * `claim_ended` (`how: "deleted"` when none of its elements is live after a live change,
   * `"gone_outside_save"` when an outside save removed the last of them). */
  | {
      kind: "claim_started_elems";
      who: Handle;
      claim: string;
      root: string;
      keys: string[];
      note: string | null;
      hold: boolean;
      expires_ms: number;
      version: number;
      /** Who holds it (wire 3; absent from an older host). */
      actor?: ClaimActor;
      /** When it was taken, on the host's clock (wire 3). */
      started_ms?: number;
    }
  /** A claim on a block of cells of a live grid (feature `cell_claims`): one range of one
   * container. It ends with `claim_ended`. Always with its actor and start time. */
  | {
      kind: "claim_started_cells";
      who: Handle;
      claim: string;
      container: string;
      /** One range. */
      ranges: CellRange[];
      note: string | null;
      hold: boolean;
      expires_ms: number;
      version: number;
      actor: ClaimActor;
      started_ms: number;
    }
  /** `actor`, `at_ms` (when it ended, the host's clock) and `at_rev` (the change that ended it:
   * `edited`, `deleted`, `gone_outside_save`; else `null`): wire 3, absent from an older host.
   * `lines`: a claim on lines, its lines; a claim on cells, its range's first and last row
   * numbers at `version`, in the rows root. `how` is a `ClaimEndReason` from this kit; read it
   * with `claimEndReason`, which takes a reason this client doesn't know as `deleted`. `keys`: a
   * claim on elements, its elements by address as they were when it was taken (so a view can
   * name what went); absent otherwise (and from an older kit). */
  | { kind: "claim_ended"; who: Handle; claim: string; how: string; version: number; actor?: ClaimActor; at_ms?: number; at_rev?: number | null; lines?: LineSpan; keys?: string[]; [k: string]: unknown }
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
  /** This connection's selection, pointer and viewport; absent or `null` when it has none. */
  selection?: ElemSelection | null;
  pointer?: Point | null;
  viewport?: Rect | null;
  /** This connection's focus and text range (wire 3); absent or `null` when it has none. */
  focus?: ElemFocus | null;
  range?: ElemRange | null;
  /** This connection's cells in a live grid; absent or `null` when it has none. */
  cells?: Cells | null;
  /** This connection's view address and time position; absent when it has none. */
  view_address?: string | null;
  time_position?: TimePositionJson | null;
}

/** A playhead (WIRE.md, Presence frames, `time_position`): `at` a decimal integer string in the
 * pack's unit (the view schema's `time_unit`), never a float; `state`; `at_ms` the host clock when
 * the kit took it (so a follower extrapolates a playing one at `rate`); `in` what it is in, by
 * address; `rate` a decimal ratio (`1/2`), 1 when absent. */
export interface TimePositionJson {
  at: string;
  state: "playing" | "paused";
  at_ms: number;
  in?: string;
  rate?: string;
}

/** A time position as a client sends it: the host stamps `at_ms`. */
export type TimePositionSet = Omit<TimePositionJson, "at_ms">;

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
  /** The most active connection's selection, pointer and viewport that has one; absent from an
   * older host. */
  selection?: ElemSelection | null;
  pointer?: Point | null;
  viewport?: Rect | null;
  /** The most active connection's focus and text range that has one (wire 3); absent from an
   * older host. */
  focus?: ElemFocus | null;
  range?: ElemRange | null;
  /** The most active connection's cells in a live grid that has some; absent or `null` when
   * none has. */
  cells?: Cells | null;
  /** The most active connection's view address and time position that has one; absent when
   * none has (and from an older host). */
  view_address?: string | null;
  time_position?: TimePositionJson | null;
  /** The participant's live claims on elements, so a late subscriber sees who holds what.
   * Present only when they hold some. */
  elem_claims?: ElemClaim[];
  /** The participant's live claims on cells, likewise. Present only when they hold some. */
  cell_claims?: CellClaim[];
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
  /** This connection's client ID (`Joined.client`): new on every connection, or the client's
   * own writer again when `kept`. */
  client: number;
  /** This connection's writer lease (`Joined.lease`, kit v0.14.0; a decimal string, as the
   * token is 64 bits): the client names it on its next reconnect (`ConnectOptions.lease`).
   * Absent from a host without writer leases. */
  lease?: string;
  /** The reconnect took the client's own writer back (`Joined.kept`): `client` is its previous
   * one, and nothing of its previous connection lands after this welcome. */
  kept?: boolean;
  /** A reconnect that didn't keep its writer: the version its previous lease ended at, when the
   * host knows it (`Joined.prev_ended`). Changes under that writer after it are another
   * connection's. */
  prevEnded?: number;
  /** This participant as the roster names them (pid, or handle for an App viewer). */
  self: Handle;
  rights: Rights;
  /** The frame families this host takes, by name (`"presence"`: `select`, `pointer`, `viewport`;
   * `"claims"`: `claim`, `release`). A client sends a family's frames only when its name is listed
   * and ignores names it doesn't know. Absent from an older host, which takes none (WIRE.md,
   * Feature names). */
  features?: string[];
  /** The wire version the host speaks (`WIRE_VERSION`; WIRE.md, Wire versions). Absent: 2. */
  wire?: number;
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
  | { type: "view"; root?: string; first: number; last: number }
  /** This connection's selection, pointer or viewport (`null`: clear). `req` is the presence
   * channel's own request number, never an `op` (WIRE.md, Presence frames). Answered only when
   * refused (`presence_refused`). */
  | { type: "select"; req: number; selection: ElemSelection | null }
  | { type: "pointer"; req: number; point: Point | null }
  | { type: "viewport"; req: number; rect: Rect | null }
  /** This connection's focus or text range (wire 3; `null`: clear), sent only when the host
   * lists `"focus"` or `"range"`. Answered only when refused. */
  | { type: "focus"; req: number; focus: ElemFocus | null }
  | { type: "range"; req: number; range: ElemRange | null }
  /** This connection's view address and time position (`null`: clear), sent only when the host
   * lists `"view_address"` or `"time_position"`. Answered only when refused. */
  | { type: "view_address"; req: number; address: string | null }
  | { type: "time_position"; req: number; time_position: TimePositionSet | null }
  /** This connection's cells in a live grid (`null`: clear), sent only when the host lists
   * `"cells"`. It also sets the connection's focus to their container. Answered only when
   * refused. */
  | { type: "cells"; req: number; cells: Cells | null }
  /** Claim elements of a list root (`Session::claim_elems`). `req` is the channel's own request
   * number, never an `op` (WIRE.md, Claim frames). Answered by `claimed` or `presence_refused`. */
  | { type: "claim"; req: number; root: string; keys: string[]; ttl_ms?: number; note?: string; hold?: boolean }
  /** Claim one block of cells of a live grid: `cells` in place of `root` and `keys`, sent only
   * when the host lists `"cell_claims"`. Answered as the claim on elements. */
  | { type: "claim"; req: number; cells: Pick<Cells, "container" | "ranges">; ttl_ms?: number; note?: string; hold?: boolean }
  /** Release this participant's claim, or all of them here with no `claim`
   * (`Session::release_claim`). Answered by `released` or `presence_refused`. */
  | { type: "release"; req: number; claim?: string }
  /** The client holds nothing unsent and is done with its writer (kit v0.14.0, a host with
   * writer leases; `LiveText.bye`): the host releases the connection (`Session::release`) and
   * closes it. No reply. */
  | { type: "bye" }
  /** Start the keyed watch, or start it again after a gap (`KeyedWatch`): the host answers
   * with a fresh snapshot (`keyed_snapshot`, then its pages). Sent only when the host lists
   * `"keyed_watch"`. */
  | { type: "watch" }
  /** The last numbered keyed frame this client applied (`seq`), never a presence frame's. Sent
   * only when the host lists `"keyed_watch"`. No reply. */
  | { type: "report"; seq: number };

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
  /** The reply to `claim`: the claim's id, `claim-N`. */
  | { type: "claimed"; req: number; claim: string }
  /** The reply to `release`: the claims released (none when none was live and yours). */
  | { type: "released"; req: number; claims: string[] }
  /** A presence or claim frame refused, by its `req`; nothing changed. Only the presence channel
   * reads it, and it never reads `refused` (WIRE.md, Presence frames). */
  | { type: "presence_refused"; req: number; error: ErrorReport }
  /** The whole roster (for hosts that fold the deltas themselves). */
  | { type: "roster"; entries: RosterEntry[] }
  /** One roster change as the kit pushes it (`subscribe_roster`): the client folds them. */
  | { type: "roster_event"; event: RosterEvent }
  /** A problem with a frame that has no `op` to answer (a frame over the cap, unreadable JSON). */
  | { type: "error"; error: ErrorReport }
  /** The keyed watch's numbered frames (WIRE.md, The keyed watch; `KeyedWatch`). */
  | KeyedSnapshotFrame
  | KeyedPageFrame
  | OpsChangeFrame
  /** The host can't serve the keyed watch (`MPC-E401`: an entry over the frame cap). Unnumbered. */
  | { type: "watch_refused"; error: ErrorReport }
  /** A verb on the watched doc was refused and applied nothing (kit after v0.18.0). Unnumbered. */
  | VerbRefusedFrame
  /** What came of an outside save of the doc's file (kit after v0.18.0). Unnumbered. */
  | OutsideSaveFrame
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
  BAD_CLAIM: "MPC-E304",
  SELECTION_TOO_LARGE: "MPC-E310",
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
   * with `canRetry(reason)`; the host's retry loop goes by `canRetry`, never this flag
   * (HOST.md). */
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
  /**
   * The writer lease of this client's previous connection (the last welcome's `lease`, kit
   * v0.14.0), sent with `prev` (the `lease` query parameter). A host with writer leases gives
   * the client that writer back when the lease is still its (`welcome.kept`), closing the old
   * connection first; it never closes a connection on `prev` alone.
   */
  lease?: string;
  signal: AbortSignal;
}

/**
 * The transport a host implements for its live editor: `Transport`, plus one connection attempt
 * and its end. One object for the editor's whole life, across reconnects: frame and close
 * listeners outlive connections; `connect` while open replaces the connection; `send` while
 * no connection is open drops the frame (never buffers it). The host's retry loop backs off,
 * then `connect({resume: live.version, prev})`, then `live.reconnected(welcome)` (HOST.md).
 */
export interface LiveTransport extends Transport {
  /** One attempt: open the socket, authenticate (the host's; the library never sees a
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

// ---------- the keyed watch (WIRE.md, "The keyed watch") ----------

/** A value in a keyed doc: a string register, or none. */
export type KeyedValue = string | null;

/** A keyed doc's root kinds as the watch names them. */
export type KeyedRootKind = "text" | "list" | "map";

/** The doc's model, in the snapshot's head: its roots in order, and `composite` (kit after
 * v0.15.1, additive): the map roots whose keys are a list's elements' fields
 * (`<element>\x1f<field>`), as `{map, list}`. Absent from an older kit. */
export interface KeyedSchema {
  name: string;
  roots: { name: string; kind: string }[];
  composite?: { map: string; list: string }[];
}

/** Where a view field's value comes from (the view schema's `Src`): `null` (not carried), a
 * field's name, the element's own list value, the pack's derived value, or a field whose raw
 * values stand for words. */
export type ViewSrc = null | string | { list_value: true } | { derived: string } | { field: string; values: Record<string, string> };

/** What a field holds, for the change frames' op kinds (the view schema's `field_class`):
 * `cell` is a cell's value in a live grid. */
export type FieldClass = "geometry" | "text" | "style" | "visibility" | "meta" | "cell";

/** How one axis of a live grid is numbered: each row's (column's) gap is its `gap_field` value
 * in its composite map, and the first one's number is its gap plus `first`. */
export interface GridNumbering {
  gap_field: string;
  first: number;
}

/**
 * Where a live grid lives in the keyed doc (the view schema's `grid`; WIRE.md, "The live grid"):
 * the rows (always the schema's `units`), the columns, their composite maps (`cells` for the rows,
 * keyed `<row address>\x1f<field>`; `col_fields` for the columns), which field names a column's
 * container, how numbers are worked out, the highest row and column numbers, how a cell's value
 * is encoded, the row and column fields a view reads, and how a peer's viewport is measured
 * (`"cells"`: in cells of its focus container).
 */
export interface GridSlot {
  rows: string;
  cols: string;
  cells: string;
  col_fields: string;
  col_container_field: string;
  /** A field of the rows' map starting with this is a row field; any other is a cell. */
  row_fields_start: string;
  /** A cell field's column address: `{container}` the container's address, `{field}` the field. */
  col_of_cell_field: string;
  numbering: { rows: GridNumbering; cols: GridNumbering };
  /** How a column is labelled: `letters` (1 = A, 27 = AA) or `numbers`. */
  col_label: "letters" | "numbers";
  max: { rows: number; cols: number };
  cell: { encoding: string; type: string; value: string; formula: string; formula_kind: string; style: string; types: Record<string, string> };
  row_fields: { hidden: string; height: string; height_unit: string };
  col_field_names: { hidden: string; width: string; width_unit: string };
  viewport: string;
  style_hint: { field: string; default: string; table: string; parts: string[] };
  /** The pack's derived values that are kept by cell field (`a1`, a formula's text as people
   * read it): a snapshot's rows pages carry them, and a change's `derived.units` updates them.
   * Absent: none. */
  cell_derived?: string[];
}

/** The pack's view schema, as the snapshot's view carries it. */
export interface ViewSchemaJson {
  containers: string;
  units: string;
  unit_container_field: string;
  container_id_field: string | null;
  native_unit: string;
  page: { root: string; w: string; h: string } | null;
  /** `parent`: where a container's parent container comes from (WIRE.md, "Containers in
   * containers"); absent when the pack declares none (and from an older kit). A client never
   * reads it from the keyed state: the kit sends each container's `parent` and `parent_after`. */
  container_fields: { title: ViewSrc; hidden: ViewSrc; layout: ViewSrc; parent?: ViewSrc };
  unit_fields: { kind: ViewSrc; name: ViewSrc; alt: ViewSrc; text: ViewSrc; hidden: ViewSrc; editable_text: ViewSrc; placeholder: ViewSrc };
  geometry: { field: string; parts: string[]; rot_unit: string } | null;
  field_class: Record<string, FieldClass>;
  /** Ordered `[prefix, class]` rules: a field with no `field_class` entry takes the class of the
   * first prefix it starts with. Absent from an older kit (none). */
  field_class_prefix?: [string, FieldClass][];
  derived: { containers: string[]; units: string[] };
  /** The live grid, for a pack whose units are a grid's rows; `null` (or absent, from an older
   * kit) otherwise. */
  grid?: GridSlot | null;
  /** The names of the pack's derived values per unit (small texts only the pack works out: a
   * transform, a label as people read it): each unit carries its own in `derived`, and a
   * change's `derived.units.<unit>.<name>` updates them. Absent: none (and from an older kit). */
  unit_derived?: string[];
  /** The names of the pack's derived values per container (a timeline track's label and kind):
   * each container carries its own in `derived`, and a change's
   * `derived.containers.<container>.<name>` updates them. Absent: none (and from an older kit). */
  container_derived?: string[];
  /** The 3D slot (WIRE.md, "Placements in 3D"): the fields a placement is read from and its
   * numbers' precision. Absent: no 3D (and from an older kit). */
  space?: SpaceSlotJson;
  /** Which unit kinds nest inside which (WIRE.md, "Nesting"), in the pack's order: a unit of
   * `kind` sits inside a unit of `parent` (its own `parent`, by address, says which one).
   * Absent: none declared (and from an older kit). */
  nesting?: NestingJson[];
}

/** One nesting relation of the view schema: units of `kind` sit inside a unit of `parent`. */
export interface NestingJson {
  kind: string;
  parent: string;
}

/** The view schema's 3D slot. */
export interface SpaceSlotJson {
  unit_fields: string[];
  container_fields: string[];
  precision: "f32" | "f64";
}

/** A rotation, radians: Euler angles about X, Y, Z (in that slot order) turned in the order
 * word's order about fixed axes; a quaternion `w, x, y, z`; an axis and an angle. */
export type RotationJson = { euler: string; angles: number[] } | { quat: number[] } | { axis: number[]; angle: number };

/** One step of a placement. */
export type StepJson =
  | { trs: { loc: number[]; rot: RotationJson; scale: number[] } }
  | { axes: { origin: number[]; z: number[] | null; x: number[] | null } }
  | { matrix: number[] };

/** Where a unit or container sits: relative to another element's address, or (`null`) the doc's
 * space; the first step outermost; `approx` a reason word when the file can't say exactly. */
export interface PlacementJson {
  relative_to: string | null;
  steps: StepJson[];
  approx?: string | null;
}

/** The doc's space. */
export interface SpaceJson {
  length_unit: string;
  metres_per_unit: number | null;
  up: string;
  handed: string;
}

/** One container of the view index (a slide), by address; `position` from 1. */
export interface ViewContainer {
  key: string;
  position: number;
  title: string | null;
  hidden: boolean;
  layout: string | null;
  render_hash: string | null;
  fonts: number[];
  has_fields: string[];
  unit_count: number;
  /** The container it sits inside, by address (a collection inside a collection), for an
   * outliner's tree (WIRE.md, "Containers in containers"): always another container of the same
   * view, never in a cycle, at most `CONTAINER_DEPTH_MAX` levels deep. Absent: at the top level
   * (and every container from an older kit). `position` stays the flat order. */
  parent?: string;
  /** Its placement (a 3D slot with container fields); absent or `null`: not placed. */
  placement?: PlacementJson | null;
  /** The pack's derived values (the view schema's `container_derived`), `{name: text}`, each
   * text at most `VIEW_TEXT_MAX` bytes; absent when the container has none. */
  derived?: Record<string, string>;
}

/** One unit of the view index (a shape), by address, in native units. */
export interface ViewUnit {
  key: string;
  container: string;
  parent: string | null;
  live: boolean;
  kind: string;
  name: string;
  name_is_generic: boolean | null;
  alt: string | null;
  z: number;
  bbox: { x: number; y: number; w: number; h: number } | null;
  bbox_inherited: boolean;
  rot: number;
  flip: { h: boolean; v: boolean };
  approx_box: boolean;
  text: { summary: string; truncated: boolean } | null;
  fields: string[];
  hidden: boolean;
  placeholder: { type: string; idx: string | null } | null;
  editable_text: "plain" | "formatted" | "never" | null;
  /** The pack's derived values (the view schema's `unit_derived`), `{name: text}`, each text at
   * most `VIEW_TEXT_MAX` bytes; absent when the unit has none. */
  derived?: Record<string, string>;
  /** Its placement (a 3D slot); absent or `null`: not placed by the slot, not drawn from it. */
  placement?: PlacementJson | null;
}

/** The view index's snapshot, as a snapshot's head carries it: everything but the units. */
export interface ViewSnapshotJson {
  schema: ViewSchemaJson;
  tier: string;
  rev: number | null;
  version: number | null;
  page: { w: number; h: number; unit: string } | null;
  fonts: unknown[];
  layouts: unknown[];
  containers: ViewContainer[];
  /** The doc's space (a 3D slot). */
  space?: SpaceJson;
}

/** What a change did to one unit of the view (the `UnitDelta`):
 * its address and its container's (before a `delete`, after anything else), the kinds of
 * change, the geometry field's value before and after when the change moved it (`null`
 * otherwise, and for an empty one), the fields that changed (names only), whether it is hidden
 * now (with `hide_show`), and its placeholder type. `cell`: a field of class `cell` changed (a
 * row's cell value in a live grid), listed last. */
export interface UnitDelta {
  key: string;
  container: string | null;
  ops: ("add" | "delete" | "restore" | "reorder" | "move_resize" | "rotate" | "flip" | "place" | "text" | "style" | "hide_show" | "props" | "cell")[];
  geom_before: string | null;
  geom_after: string | null;
  fields: string[];
  hidden_after: boolean | null;
  placeholder_type: string | null;
  /** A 3D slot: its placement after the change, on `add` and `restore` always and with `place`
   * when it moved (`null`: none now). Absent: not carried. */
  placement_after?: PlacementJson | null;
}

/** What a change did to one container (`ContainerDelta`): a list op with its positions (from
 * 1) before and after, or its fields (`hide_show` with `hidden_after`, `props`), by name. */
export interface ContainerDelta {
  key: string;
  op: "add" | "delete" | "restore" | "reorder" | "hide_show" | "props";
  position_before: number | null;
  position_after: number | null;
  hidden_after: boolean | null;
  fields: string[];
  /** A 3D slot with container fields: its placement after the change (`null`: none now). */
  placement_after?: PlacementJson | null;
  /** Containers that nest: its parent's address after the change (`null`: at the top level
   * now), on `add` and `restore` always and otherwise when it changed. A container whose parent
   * changed with no other change of its own gets a `props` delta naming no field, with this.
   * Absent: unchanged (and from an older kit). */
  parent_after?: string | null;
}

/** The head of a snapshot: the doc's keyed state at version `upto` follows in `pages` pages.
 * `view`: the view index's snapshot at `upto` (its view schema, page, fonts, layouts and
 * containers), opaque here, or `null` when the doc's pack
 * declares no view schema; its units follow in `units` pages. */
export interface KeyedSnapshotFrame {
  type: "keyed_snapshot";
  seq: number;
  upto: number;
  schema: KeyedSchema;
  view?: ViewSnapshotJson | null;
  pages: number;
}

/** One page of a container's units of the view index (`container` is its address): units
 * `from` on, back to front; `next` where the next page starts (`null`: the last); `total` the
 * container's units. Each unit is opaque here (the contract's `Unit`). */
export interface KeyedUnitsPage {
  container: string;
  from: number;
  units: ViewUnit[];
  next: number | null;
  total: number;
}

/** One page of a snapshot (`page` from 1): one root's run, exactly one of `live` (elements by
 * address, in order, appended), `hidden` (deleted elements), `entries` (map entries: key or
 * element address, field, value), `text` (appended) and `units` (the view index's units of one
 * container; `root` is the units root). */
export interface KeyedPageFrame {
  type: "keyed_page";
  seq: number;
  upto: number;
  page: number;
  pages: number;
  root: string;
  live?: [string, KeyedValue][];
  hidden?: [string, KeyedValue][];
  entries?: [string, string | null, KeyedValue][];
  text?: string;
  units?: KeyedUnitsPage;
  /** On a page of the rows' map in a live grid: the pack's derived values kept by cell field
   * (the grid's `cell_derived`) for this page's entries, `{name: {row address: {field: text}}}`;
   * absent when there are none. */
  derived?: Record<string, Record<string, Record<string, string>>>;
}

/** Who made a change: `kind` is `person`, `agent`, `app` or `file` (an outside save; no
 * `colour`, the host's to choose). */
export interface WatchActor {
  id: Handle;
  name: string;
  kind: string;
  colour: string | null;
}

/** One op of a change. `before` / `after` are absent when there is none (before an insert, after
 * a map delete). A list op's indexes are positions in the live order just before and just after
 * it (`null`: not shown); `effect: "hidden"` wrote to a deleted element. A map op on a composite
 * root has the element in `key` and the rest in `field` (`<key>\x1f<field>`); else `field` is
 * `null`. */
export type WatchOp =
  | { root: string; kind: "text"; hunks: Hunk[] }
  | {
      root: string;
      kind: "list";
      op: "insert" | "move" | "delete" | "restore" | "set" | "drop";
      key: string;
      index_before: number | null;
      index_after: number | null;
      before?: KeyedValue;
      after?: KeyedValue;
      effect: "visible" | "hidden";
    }
  | { root: string; kind: "map"; op: "set" | "delete"; key: string; field: string | null; before?: KeyedValue; after?: KeyedValue };

/** A refused verb on a keyed watch (WIRE.md, "Refused verbs"): unnumbered, it changes nothing.
 * Sent only to viewers who may read the doc; an App viewer gets handles and no `turn`. */
export interface VerbRefusedFrame {
  type: "verb_refused";
  at_ms: number;
  /** Who was refused. */
  actor: WatchActor;
  client: number;
  turn: string | null;
  /** The verb's name (`shape move`). */
  verb: string;
  /** The unit it addressed, by address; `null` when it was refused before it named one. */
  unit: string | null;
  /** The refusal's id (`MPC-E301` for a claim). */
  code: string;
  /** Who holds the claim or lease that refused it; `null` otherwise. */
  holder: WatchActor | null;
  /** That claim, `claim-N`; `null` otherwise. */
  claim: string | null;
}

/** What the kit made of an outside save of the watched doc's file (WIRE.md, "Outside saves"):
 * unnumbered, after the save's own change frame. `ingested`: taken in, the doc at `rev`.
 * `kept_both`: the save is the doc at `rev`; what was live before is the stored version `kept`
 * and the conflict `copy` (`null` when it couldn't be written; `doc` when it is a document).
 * `declined`: the file can't be a live doc (`code`, `message`). `missing`: the file is gone. */
export type OutsideSaveFrame =
  | { type: "outside_save"; at_ms: number; doc: string; outcome: "ingested"; rev: number }
  | {
      type: "outside_save";
      at_ms: number;
      doc: string;
      outcome: "kept_both";
      rev: number;
      kept: number;
      copy: { name: string; path: string; doc?: string } | null;
    }
  | { type: "outside_save"; at_ms: number; doc: string; outcome: "declined"; code: string; message: string }
  | { type: "outside_save"; at_ms: number; doc: string; outcome: "missing" };

/** One committed change (the doc's version `rev`), numbered `seq`. `change_kind` is the kit's
 * `ChangeKind` (`edit`, `undo`, `redo`, `revert`, `restore`, `external`, `import`). */
export interface OpsChangeFrame {
  type: "ops_change";
  seq: number;
  rev: number;
  at_ms: number;
  change_kind: string;
  actor: WatchActor;
  client: number;
  via: string;
  on_behalf_of: string | null;
  turn: string | null;
  verb: string | null;
  claim: string | null;
  ops: WatchOp[];
  /** What the change did to the view, for a doc whose pack has a view schema (kit after v0.15.1;
   * absent otherwise, and from an older kit): its units, its containers, and the render hint
   * (every container a delta names, then those whose render hash changed). */
  units?: UnitDelta[];
  containers_changed?: ContainerDelta[];
  containers?: string[];
  /** The pack's derived values (`null`: none): the kit's typed parts, and any other key a pack
   * adds. */
  derived: FrameDerived | null;
  /** The containers whose render hash changed (`null`: none). */
  render_hashes: Record<string, string> | null;
  /** A 3D slot: the doc's space after the change, when it changed. */
  view_space?: SpaceJson;
}

/** A change frame's `derived` (WIRE.md, "Derived values"): what only the pack works out, by
 * address, in the parts the kit gives builders for; a pack may add other keys. */
export interface FrameDerived {
  /** Per unit (a grid's row) by address. */
  units?: Record<string, UnitDerived>;
  /** Per container (a sheet) by address. */
  containers?: Record<string, ContainerDerived>;
  /** The cells a verb wrote. */
  written?: Written;
  /** The rows or columns a verb inserted or deleted. */
  bands?: Band[];
  [k: string]: unknown;
}

/** What the pack worked out for one unit: `a1`, by field, the field's value as people read it
 * in A1 terms (a formula's text), `null` when it has none. */
export interface UnitDerived {
  a1?: Record<string, string | null>;
  [k: string]: unknown;
}

/** What the pack worked out for one container: its merged blocks, and its style hints by the
 * cells' style id. */
export interface ContainerDerived {
  merges?: CellRange[];
  styles?: Record<string, StyleHint>;
  [k: string]: unknown;
}

/** Enough of a cell style for a view to hint at it: bold, the fill colour (`null`: none), and the
 * number format. */
export interface StyleHint {
  bold: boolean;
  fill: string | null;
  numfmt: string;
}

/** The cells a verb wrote, in one container. */
export interface Written {
  container: string;
  ranges: CellRange[];
}

/**
 * Rows or columns a verb inserted or deleted, by number: an insert's `from`..`to` are numbers
 * after the change (`numbers: "after"`), a delete's are numbers before it (`"before"`).
 */
export interface Band {
  container: string;
  axis: "rows" | "cols";
  op: "insert" | "delete";
  from: number;
  to: number;
  numbers: "before" | "after";
}
