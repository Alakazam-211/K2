// Presence and marks as data (`LiveText.participants()`, `LiveText.marks()`): what a host needs
// to draw other people's carets and the person's rejected changes in its own editor UI. No
// DOM: the library holds the data and says when it changed; the host draws. See PRESENCE.md.
//
// Offsets here are UTF-16 code units of the replica's current text (what CodeMirror and a JS
// string index use), not the UTF-8 bytes the wire and `carets()` use.

import type { OpId, Presence } from "./live";
import { byteToIndex, indexToByte, utf8Len } from "./hunk";
import { ERR } from "./wire";
import type {
  CellRange,
  Cells,
  ClaimActor,
  ClientFrame,
  Corner,
  ElemFocus,
  ElemRange,
  ElemSelection,
  ErrorReport,
  Handle,
  Point,
  Rect,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  TimePositionJson,
  TimePositionSet,
  Transport,
  Welcome,
} from "./wire";
import { CELLS_FEATURE, CELLS_RANGES_MAX, CELL_CLAIMS_FEATURE } from "./grid";

/**
 * A participant's kind, stable for a host to switch on. The roster's `kind` (the kit's
 * `ParticipantKind` in lower case) maps: `person` to `person`, `agent` to `agent`, and every
 * other value (`app`, `external`, `restore`, `import`, and any added later) to `app`: a program
 * acting on the doc, not a person or an agent typing. `Participant.rawKind` keeps the roster's
 * own value.
 */
export type ParticipantKind = "person" | "agent" | "app";

/** A caret or selection in UTF-16 code units of the current text (`anchor` stays put while
 * `head` moves; equal for a bare caret). */
export interface Range16 {
  anchor: number;
  head: number;
}

/** One connection in the doc (`LiveText.participants()`). */
export interface Participant {
  /** The participant's handle (the roster's `handle`, a change event's `pid`). */
  handle: Handle;
  /** The connection (`Device.client`), from a host that sends per-connection carets; absent
   * from an older host (one entry a participant) and on a participant's caret-less entry. */
  client?: number;
  name: string;
  /** The roster's `colour`, exactly as the host sent it (one source of truth, no
   * conversion): a stable identity value for the host to map to its own palette, not a style
   * instruction. */
  colour: string;
  kind: ParticipantKind;
  /** The roster's own `kind` string (`person`, `agent`, `app`, `external`, ...). */
  rawKind: string;
  /** This connection itself: listed once, with `caret: null` and no selections (the host
   * draws its own caret), so the host can show "you" in a list. */
  isSelf: boolean;
  /** Another connection of this connection's own participant (the same person in another tab
   * or on another device). */
  sameUser: boolean;
  /** The host's label for the connection (`Device.label`), when it gave one. Never doc text. */
  label?: string;
  /** The connection's caret in the current text; `null` when it has none to show. */
  caret: Range16 | null;
  /**
   * Every selection range of the connection. One element (the caret) today, since the wire
   * carries one range a connection; empty with no caret. Kept an array so a host that draws
   * from it keeps working when the wire carries several.
   */
  selections: Range16[];
  /**
   * When this connection's caret last moved or it last changed the text, on the replica's
   * clock (`LiveOptions.timers.now()`, default `Date.now()`); when it was first seen until then.
   * A caret the host re-sends unmoved (after other people's changes) doesn't count. For a host
   * to fade idle carets.
   */
  lastActiveAt: number;
  /** The roster's `away`. */
  away: boolean;
}

/** Where a rejected change stands for Re-apply (`placeRejection`): `ok` (it would go in),
 * `in-text` (all of it is in the text already: Re-apply only dismisses it), `conflict`,
 * `cant-place`. */
export type MarkPlacement = "ok" | "in-text" | "conflict" | "cant-place";

/** One rejected change, as data (`LiveText.marks()`). */
export interface Mark {
  /** The rejection's id (`Rejection.id`): what `reapply`, `dismiss` and `insertHere` take. */
  id: number;
  /** Where the change was, in UTF-16 code units of the current text (`Rejection.region`; it
   * moves with every change until dismissed). Empty (`from === to`) for a deletion, or for a
   * change the replica can't place (a reload since). */
  range: { from: number; to: number };
  /** The text it would have inserted (what the person typed); may be empty (a deletion). */
  text: string;
  /**
   * Why it was refused: the server's error id and message when it sent one (e.g. `MPC-E111`, a
   * write conflict; `MPC-E000`, the reply was lost). Without one: `MPC-E111` when the client saw
   * the change meet competing text, `MPC-E000` when the replica started again from the server's
   * text (a reload) before the change was answered.
   */
  reason: { code: string; message: string };
  /** The server's full refusal, when it sent one (`details` included). */
  error: ErrorReport | null;
  /** Its reply was lost (`MPC-E000` with a record to check): it may have landed after all. */
  mayHaveApplied: boolean;
  /** For a change whose reply was lost, `LiveText.presence(id)`; else `absent` (the server
   * refused it, or the client saw it would). */
  presence: Presence;
  /** `LiveText.placeRejection(id)`, summarized. */
  placement: MarkPlacement;
  /** `reapply(id)` would put the change back now: `presence` is `absent` and `placement` is
   * `ok`. */
  canReapply: boolean;
  /** `insertHere(id)` would insert the typed text now (one hunk, some text, `absent`). */
  canInsertHere: boolean;
  /** The change also removed text: "Insert my text here" puts back only the typed text, never
   * the removal. */
  removed: boolean;
}

/** Why "Insert my text here" can't go ahead: `unavailable` (no such rejection, no typed text,
 * or its place isn't a character boundary), `multi-hunk` (only a one-hunk change has one place
 * to insert at), `already-present` (its reply was lost and it landed), `cant-tell` (its reply
 * was lost and the record can't tell whether it landed). */
export type InsertRefusal = "unavailable" | "multi-hunk" | "already-present" | "cant-tell";

/** Where "Insert my text here" would put the typed text (`insertHerePreview`): `at` in UTF-16
 * code units of the current text. */
export type InsertPreview = { at: number; text: string; removed: boolean } | { refused: InsertRefusal };

/** What `insertHere` did: inserted `text` at `at` (UTF-16) as local edit `op`, or why not. */
export type InsertHereResult = { op: OpId; at: number; text: string } | { refused: InsertRefusal };

/** One connection as a `PresenceChannel` shows it. */
export interface PresencePeer {
  handle: Handle;
  /** The connection (`Device.client`); absent on a participant-wide entry. */
  client?: number;
  name: string;
  /** The roster's `colour`, exactly as sent. */
  colour: string;
  kind: ParticipantKind;
  rawKind: string;
  isSelf: boolean;
  sameUser: boolean;
  selection: ElemSelection | null;
  pointer: Point | null;
  viewport: Rect | null;
  /** The container in view (wire 3), or `null`. */
  focus: ElemFocus | null;
  /** A caret or selection inside one element's text field (wire 3), in **UTF-8 bytes** of the
   * field's text as the host sent it (`rangeToUtf16` converts), or `null`. */
  range: ElemRange | null;
  /** Its cells in a live grid (feature `cells`), or `null`. */
  cells: Cells | null;
  /** The view it is looking at, by address, or `null`. */
  viewAddress: string | null;
  /** Its playhead, or `null`. */
  timePosition: TimePositionJson | null;
  away: boolean;
}

/**
 * A live claim on elements, or on a block of cells, as `PresenceChannel.claims()` folds the roster:
 * from a snapshot's `elem_claims` and `cell_claims`,
 * `claim_started_elems`, `claim_started_cells` and `claim_ended`. A claim on cells has `cells`,
 * with `root` `""` and no `keys`.
 */
export interface ClaimView {
  /** `claim-N`. */
  claim: string;
  /** Who holds it: the event's `actor` (wire 3), or from their roster row (a wire-2 host). */
  actor: ClaimActor;
  root: string;
  keys: string[];
  note: string | null;
  hold: boolean;
  /** When it was taken (host clock); `null` from a wire-2 host. */
  startedMs: number | null;
  expiresMs: number;
  /** A claim on cells: its container and its one range. Absent on a claim on elements. */
  cells?: Pick<Cells, "container" | "ranges">;
}

/** A presence frame the host refused, by the `req` the channel sent it with. */
export interface PresenceRefusal {
  /** The refused frame's type; `null` when the refusal names a request too old to remember. */
  frame: PresenceFrameType | null;
  error: ErrorReport;
}

/** The frames a `PresenceChannel` sends, each refusable. */
export type PresenceFrameType = "select" | "pointer" | "viewport" | "focus" | "range" | "cells" | "view_address" | "time_position" | "claim" | "release";

/** The name a host lists in `welcome.features` when it takes the presence frames. */
export const PRESENCE_FEATURE = "presence";

/** The name a host lists in `welcome.features` when it takes the `focus` frame (wire 3). */
export const FOCUS_FEATURE = "focus";

/** The name a host lists in `welcome.features` when it takes the `range` frame (wire 3). */
export const RANGE_FEATURE = "range";

/** The name a host lists in `welcome.features` when it takes the `view_address` frame. */
export const VIEW_ADDRESS_FEATURE = "view_address";

/** The name a host lists in `welcome.features` when it takes the `time_position` frame. */
export const TIME_POSITION_FEATURE = "time_position";

/** The longest view address or `in` the host takes, in UTF-8 bytes (`PRESENCE_ADDRESS_MAX`). */
export const PRESENCE_ADDRESS_MAX = 512;

/** A decimal integer as a time position writes one: an optional `-`, digits, no leading zero,
 * within 64 bits. */
function decimalInt(s: string): boolean {
  if (!/^-?(0|[1-9][0-9]*)$/.test(s) || s === "-0") return false;
  const n = BigInt(s);
  return n >= -(2n ** 63n) && n < 2n ** 63n;
}

/** What the host would refuse (`MPC-E114`) about a time position, or `null`. */
function timeProblem(t: TimePositionSet): string | null {
  if (!decimalInt(t.at)) return `the time ${JSON.stringify(t.at)} is not a decimal integer`;
  if (t.state !== "playing" && t.state !== "paused") return `the state ${JSON.stringify(t.state)} is not playing or paused`;
  if (t.rate !== undefined) {
    const [n, d, more] = t.rate.split("/");
    const good = more === undefined && decimalInt(n) && (d === undefined || (decimalInt(d) && BigInt(d) > 0n));
    if (!good) return `the rate ${JSON.stringify(t.rate)} is not a decimal ratio`;
  }
  if (t.in !== undefined && addressProblem(t.in)) return `the time position's in ${JSON.stringify(t.in)} is not an address`;
  return null;
}

/** What the host would refuse (`MPC-E114`) about an address, or `null`: empty, over
 * `PRESENCE_ADDRESS_MAX` bytes, or a malformed native ID (`@kind:id`) or key (`@k:`). */
function addressProblem(a: string): string | null {
  if (a.length === 0) return "an empty address";
  if (utf8Len(a) > PRESENCE_ADDRESS_MAX) return `over ${PRESENCE_ADDRESS_MAX} bytes`;
  if (a.startsWith("@k:") || a.startsWith("key:")) return /^(@k:|key:)[0-9a-z._-]{1,24}$/.test(a) ? null : "a malformed key";
  if (a.startsWith("@")) {
    const ok = a
      .slice(1)
      .split("/")
      .every((seg) => /^[a-z][a-z0-9_-]*:\S+$/.test(seg) && !seg.startsWith("k:"));
    return ok ? null : "a malformed native id";
  }
  return null;
}

/**
 * A range's offsets (UTF-8 bytes on the wire) as UTF-16 indexes into `text`, the field's text,
 * or `null` when either isn't a character boundary of it (the text isn't the one the range was
 * set on). Contract case A: `Café plan` with bytes 0..5 is 0..4.
 */
export function rangeToUtf16(range: Pick<ElemRange, "anchor" | "head">, text: string): Range16 | null {
  const anchor = byteToIndex(text, range.anchor);
  const head = byteToIndex(text, range.head);
  return anchor < 0 || head < 0 ? null : { anchor, head };
}

/** The name a host lists in `welcome.features` when it takes the claim frames. */
export const CLAIMS_FEATURE = "claims";

/** The most keys in one claim. */
export const CLAIM_KEYS_MAX = 256;

/** The longest note on a claim, in UTF-8 bytes. */
export const CLAIM_NOTE_MAX = 1024;

/** How a claim ended (the kit's `ClaimEnd`; WIRE.md, "Claims on elements"). */
export type ClaimEndReason = "released" | "edited" | "expired" | "left" | "reset" | "deleted" | "gone_outside_save";

const CLAIM_END_REASONS: readonly string[] = ["released", "edited", "expired", "left", "reset", "deleted", "gone_outside_save"];

/** A `claim_ended` event's `how` as a `ClaimEndReason`: a reason this client doesn't know (from
 * a newer kit) is read as `deleted`, the claim simply ended. */
export function claimEndReason(how: string): ClaimEndReason {
  return (CLAIM_END_REASONS.includes(how) ? how : "deleted") as ClaimEndReason;
}

/** One claim's end, as `PresenceChannel.onClaimEnded` hands it on: the claim, who held it, how it
 * ended, and, for a claim on elements, its elements by address as they were (`keys`; empty
 * otherwise), so a view can say what went ("Atlas's hold on Drums ended: renamed outside"). */
export interface ClaimEnded {
  claim: string;
  who: string;
  how: ClaimEndReason;
  keys: string[];
}

/** `PresenceChannel.claim`'s options. */
export interface ClaimOptions {
  /** How long it lasts, in ms: a positive whole number, clamped to the host's most. Absent: the
   * host's default. */
  ttlMs?: number;
  /** Said to the others ("moving these"). At most `CLAIM_NOTE_MAX` bytes. */
  note?: string;
  /** Survive this participant's own edits. Without it, the first edit touching the claim ends it,
   * so a client making several edits under one claim (a drag) sets this and releases when done. */
  hold?: boolean;
}

/**
 * Why `claim` or `release` didn't get its answer: `refused` (`error` says why, from the host or
 * the channel itself), `unsupported` (this connection's host doesn't list `"claims"`: nothing was
 * sent), `closed` (the channel closed) or `reconnected` (the connection changed before the host
 * answered). For the last two the request's fate is unknown: see `PRESENCE.md`.
 */
export type ClaimRejectedReason = "refused" | "unsupported" | "closed" | "reconnected";

/** What `claim` and `release` reject with. */
export class ClaimRejected extends Error {
  readonly reason: ClaimRejectedReason;
  /** The refusal, when `reason` is `refused`. */
  readonly error: ErrorReport | null;

  constructor(reason: ClaimRejectedReason, message: string, error: ErrorReport | null = null) {
    super(message);
    this.name = "ClaimRejected";
    this.reason = reason;
    this.error = error;
  }
}

/** A `claim` or `release` waiting for its answer. */
interface Asked {
  kind: "claim" | "release";
  resolve: (answer: string | string[]) => void;
  reject: (why: ClaimRejected) => void;
}

/** The most keys in one `ElemSelection`. */
export const ELEM_SELECTION_MAX = 256;

/** `PresenceChannel`'s options. */
export interface PresenceChannelOptions {
  /** How the pointer and the viewport are batched (default: `defaultScheduler`). */
  schedule?: FrameScheduler;
}

/**
 * Runs `deliver` later, once: the presence and marks events' batching (`LiveOptions.schedule`).
 * A browser host can leave the default; a host with no frames (an agent, a Node tool) can pass
 * `(deliver) => deliver()` for immediate delivery, or its own timer.
 */
export type FrameScheduler = (deliver: () => void) => void;

/** The default batch interval when there's no `requestAnimationFrame`, in ms. */
export const FRAME_MS = 16;

/** The roster's `kind` as a stable `ParticipantKind` (see there). */
export function participantKind(raw: string): ParticipantKind {
  const k = raw.toLowerCase();
  return k === "person" ? "person" : k === "agent" ? "agent" : "app";
}

/** The default scheduler: `requestAnimationFrame` when the global has one, else
 * `set(deliver, FRAME_MS)` (the replica's timers). */
export function defaultScheduler(set: (f: () => void, ms: number) => unknown): FrameScheduler {
  const g = globalThis as { requestAnimationFrame?: (cb: () => void) => unknown };
  if (typeof g.requestAnimationFrame === "function") return (deliver) => void g.requestAnimationFrame!(() => deliver());
  return (deliver) => void set(deliver, FRAME_MS);
}

/** How many sent requests `PresenceRefusal.frame` can still name. */
const REQS_KEPT = 1024;

/** One participant as the channel folds the roster. */
interface Folded {
  handle: Handle;
  name: string;
  colour: string;
  rawKind: string;
  away: boolean;
  /** The participant's own values (a grid's only ones, `client` absent on its events). */
  top: Values;
  /** Each connection's values, by client ID. */
  conns: Map<number, Values>;
  /** The host reports this participant per connection. */
  perConn: boolean;
}

interface Values {
  selection: ElemSelection | null;
  pointer: Point | null;
  viewport: Rect | null;
  focus: ElemFocus | null;
  range: ElemRange | null;
  cells: Cells | null;
  viewAddress: string | null;
  timePosition: TimePositionJson | null;
}

const noValues = (): Values => ({ selection: null, pointer: null, viewport: null, focus: null, range: null, cells: null, viewAddress: null, timePosition: null });
const hasValue = (v: Values) =>
  v.selection !== null || v.pointer !== null || v.viewport !== null || v.focus !== null || v.range !== null || v.cells !== null || v.viewAddress !== null || v.timePosition !== null;

const copyCorner = (c: Corner): Corner => ({ row: c.row, row_off: c.row_off, col: c.col, col_off: c.col_off });
const copyRange = (r: CellRange): CellRange => ({ start: copyCorner(r.start), end: copyCorner(r.end) });
const copyCells = (c: Cells): Cells => ({ container: c.container, active: c.active && copyCorner(c.active), ranges: c.ranges.map(copyRange) });

/** What the host would refuse (`MPC-E304`) about a claim, in its words. */
function claimProblem(keys: readonly string[], opts: ClaimOptions): string | null {
  if (keys.length === 0) return "a claim on elements names at least one";
  if (keys.length > CLAIM_KEYS_MAX) return `${keys.length} elements in one claim; at most ${CLAIM_KEYS_MAX}`;
  return claimOptionsProblem(opts);
}

/** What the host would refuse (`MPC-E304`) about a claim on cells, in its words. */
function cellsClaimProblem(cells: Pick<Cells, "container" | "ranges">, opts: ClaimOptions): string | null {
  if (cells.ranges.length !== 1) return `a claim on cells names one range, not ${cells.ranges.length}`;
  return claimOptionsProblem(opts);
}

/** What the host would refuse (`MPC-E304`) about a claim's options. */
function claimOptionsProblem(opts: ClaimOptions): string | null {
  if (opts.note !== undefined) {
    const n = utf8Len(opts.note);
    if (n > CLAIM_NOTE_MAX) return `a note of ${n} bytes; at most ${CLAIM_NOTE_MAX}`;
  }
  if (opts.ttlMs !== undefined && (!Number.isSafeInteger(opts.ttlMs) || opts.ttlMs <= 0)) return "a claim needs a TTL above 0 ms";
  return null;
}

function finite(...ns: number[]): boolean {
  return ns.every((n) => typeof n === "number" && Number.isFinite(n));
}

/**
 * Selection by element key, pointer and viewport over any `Transport`: data and events, no DOM.
 * It can share a transport with a `LiveText`: each reads only its own frames, and the two number
 * their requests separately (WIRE.md, Presence frames). Nothing it sends takes or moves the
 * doc's version.
 *
 * Frames go out only on a connection whose `welcome.features` lists `"presence"`. Otherwise the
 * setters only record the value, and `reconnected` sends it if a later connection lists it.
 */
export class PresenceChannel {
  private readonly t: Transport;
  private readonly unsub: () => void;
  private readonly schedule: FrameScheduler;
  private welcome: Welcome;
  private closed = false;
  private nextReq = 1;
  /** Sent requests, oldest first, bounded: successes get no reply to clear them. */
  private readonly reqs = new Map<number, PresenceFrameType>();
  /** The `claim`s and `release`s not answered yet, by request number. */
  private readonly asked = new Map<number, Asked>();
  /** The latest request of each kind, so a refusal of it forgets the value. */
  private readonly latest: Partial<Record<PresenceFrameType, number>> = {};
  private selection: ElemSelection | null = null;
  private pointer: Point | null = null;
  private viewport: Rect | null = null;
  private focus: ElemFocus | null = null;
  private range: ElemRange | null = null;
  private cells: Cells | null = null;
  private viewAddress: string | null = null;
  private timePosition: TimePositionSet | null = null;
  /** What is waiting for the next batch. */
  private pointerDue = false;
  private viewportDue = false;
  private batching = false;
  private folded: Folded[] = [];
  /** The live claims on elements, in the order they were taken. */
  private claimed: ClaimView[] = [];
  /** How many watch past the host's participant cap (the roster's `watching`). */
  private watchers = 0;
  private readonly watchingCbs = new Set<(count: number) => void>();
  private readonly claimCbs = new Set<(claims: readonly ClaimView[]) => void>();
  private readonly endedCbs = new Set<(e: ClaimEnded) => void>();
  private readonly changeCbs = new Set<(peers: readonly PresencePeer[]) => void>();
  private readonly refusedCbs = new Set<(r: PresenceRefusal) => void>();

  constructor(transport: Transport, welcome: Welcome, opts: PresenceChannelOptions = {}) {
    this.t = transport;
    this.welcome = welcome;
    this.schedule = opts.schedule ?? defaultScheduler((f, ms) => setTimeout(f, ms));
    this.unsub = transport.onFrame((f) => this.onFrame(f));
  }

  /** The connection's `welcome.features` lists `"presence"`. While false, the setters only
   * record the value. */
  get supported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(PRESENCE_FEATURE);
  }

  /** The connection's `welcome.features` lists `"claims"`. While false, `claim` and `release`
   * send nothing and reject with `unsupported`. */
  get claimsSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(CLAIMS_FEATURE);
  }

  /** The connection's `welcome.features` lists `"focus"` (wire 3). While false, `setFocus` only
   * records the value. */
  get focusSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(FOCUS_FEATURE);
  }

  /** The connection's `welcome.features` lists `"range"` (wire 3). While false, `setRange` only
   * records the value. */
  get rangeSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(RANGE_FEATURE);
  }

  /** The connection's `welcome.features` lists `"cells"`. While false, `setCells` only records
   * the value. */
  get cellsSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(CELLS_FEATURE);
  }

  /** The connection's `welcome.features` lists `"cell_claims"`. While false, `claimCells` sends
   * nothing and rejects with `unsupported`. */
  get cellClaimsSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(CELL_CLAIMS_FEATURE);
  }

  /** The connection's `welcome.features` lists `"view_address"`. While false, `setViewAddress`
   * only records the value. */
  get viewAddressSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(VIEW_ADDRESS_FEATURE);
  }

  /** The connection's `welcome.features` lists `"time_position"`. While false,
   * `setTimePosition` only records the value. */
  get timePositionSupported(): boolean {
    const f = this.welcome.features;
    return Array.isArray(f) && f.includes(TIME_POSITION_FEATURE);
  }

  /** Set the view this connection is looking at, by address (`Session::set_view_address`), or
   * clear it with `null`. Goes at once when the host lists `"view_address"`; else recorded and
   * sent on a later connection that does. An address the host would refuse (empty, over
   * `PRESENCE_ADDRESS_MAX` bytes, a malformed `@kind:id` or `@k:` key) is refused here
   * (`onRefused`, `MPC-E114`), unsent. */
  setViewAddress(address: string | null): void {
    const bad = address === null ? null : addressProblem(address);
    if (bad) return this.refuse("view_address", { id: ERR.BAD_POSITION, message: `bad position: the view address ${JSON.stringify(address)}: ${bad}` });
    if (address === this.viewAddress) return;
    this.viewAddress = address;
    this.sendViewAddress();
  }

  /** Set this connection's playhead (`Session::set_time_position`), or clear it with `null`:
   * `at` a decimal integer string in the pack's unit, `state`, and optionally `in` (an address)
   * and `rate` (a decimal ratio, `1/2`). The host stamps `at_ms`. Goes at once when the host
   * lists `"time_position"`; else recorded. A value the host would refuse is refused here
   * (`onRefused`, `MPC-E114`), unsent. */
  setTimePosition(time: TimePositionSet | null): void {
    const bad = time === null ? null : timeProblem(time);
    if (bad) return this.refuse("time_position", { id: ERR.BAD_POSITION, message: `bad position: ${bad}` });
    const next: TimePositionSet | null = time && {
      at: time.at,
      state: time.state,
      ...(time.in === undefined ? {} : { in: time.in }),
      ...(time.rate === undefined ? {} : { rate: time.rate }),
    };
    if (JSON.stringify(next) === JSON.stringify(this.timePosition)) return;
    this.timePosition = next;
    this.sendTimePosition();
  }

  /** The wire version of this connection's host (`welcome.wire`; absent: 2). */
  get wire(): number {
    return typeof this.welcome.wire === "number" ? this.welcome.wire : 2;
  }

  /** Set the container in view (wire 3; `Session::set_focus`), or clear it with `null`. Goes at
   * once when the host lists `"focus"`; else recorded and sent on a later connection that does.
   * The host refuses a key that isn't live (`MPC-E114`) and a watcher (`MPC-E305`). A focus on
   * another container (or none) clears this connection's cells here, as the host drops them. */
  setFocus(focus: ElemFocus | null): void {
    const next = focus && { root: focus.root, key: focus.key };
    if (this.cells && next?.key !== this.cells.container) this.cells = null;
    if (JSON.stringify(next) === JSON.stringify(this.focus)) return;
    this.focus = next;
    this.sendFocus();
  }

  /**
   * Set a caret or selection inside one element's text field (wire 3; `Session::set_range`), or
   * clear it with `null`. `anchor` and `head` are UTF-16 indexes into `text`, the field's text as
   * this client has it; they go out as UTF-8 bytes. The host also moves this connection's focus to
   * the element's container. The host drops a range when the field changes: send a new one then.
   * Offsets outside `text` are refused here (`onRefused`, `MPC-E114`), unsent.
   */
  setRange(range: (Omit<ElemRange, "anchor" | "head"> & Range16) | null, text = ""): void {
    let next: ElemRange | null = null;
    if (range) {
      const ok = (i: number) => Number.isSafeInteger(i) && i >= 0 && i <= text.length;
      if (!ok(range.anchor) || !ok(range.head)) {
        return this.refuse("range", { id: ERR.BAD_POSITION, message: "bad position: the range is not inside the field's text" });
      }
      next = { root: range.root, key: range.key, field: range.field, anchor: indexToByte(text, range.anchor), head: indexToByte(text, range.head) };
    }
    if (JSON.stringify(next) === JSON.stringify(this.range)) return;
    this.range = next;
    this.sendRange();
  }

  /**
   * Set this connection's cells in a live grid, or clear them with `null`: the active cell and at
   * most `CELLS_RANGES_MAX` ranges of one container, by corner (`cornerAt` makes one from numbers).
   * Goes at once when the host lists `"cells"`; else recorded and sent on a later connection that
   * does. The host also moves this connection's focus to the container, so the channel sets its
   * own focus there too (in `root`, the containers' list root; without it, the current focus's
   * root, and none is set when there is no focus), and sends no `focus` frame for it. More than
   * `CELLS_RANGES_MAX` ranges are refused here (`onRefused`, `MPC-E310`), unsent, and the cells
   * stay as they were. The host refuses a corner that isn't live, in normal form or in bounds, an
   * active cell outside the ranges, and another container's corner (`MPC-E114`), and a watcher
   * (`MPC-E305`).
   */
  setCells(cells: Cells | null, root?: string): void {
    if (cells && cells.ranges.length > CELLS_RANGES_MAX) {
      const message = `selection too large: ${cells.ranges.length} cell ranges (the most is ${CELLS_RANGES_MAX})`;
      return this.refuse("cells", { id: ERR.SELECTION_TOO_LARGE, message });
    }
    const next = cells && copyCells(cells);
    const r = root ?? this.focus?.root;
    if (next && r !== undefined) this.focus = { root: r, key: next.container };
    if (JSON.stringify(next) === JSON.stringify(this.cells)) return;
    this.cells = next;
    this.sendCells();
  }

  /**
   * Claim one block of cells of a live grid (`{container, ranges: [range]}`): resolves to the
   * claim's id, `claim-N`, as `claim` does. Sent only when the host lists `"cell_claims"`. Rejects
   * as `claim`, and here, unsent, with `MPC-E304` for other than one range (and the options
   * `claim` checks).
   */
  claimCells(cells: Pick<Cells, "container" | "ranges">, opts: ClaimOptions = {}): Promise<string> {
    const held = this.unavailable(true);
    if (held) return Promise.reject(held);
    const bad = cellsClaimProblem(cells, opts);
    if (bad) {
      const error: ErrorReport = { id: ERR.BAD_CLAIM, message: `bad claim: ${bad}` };
      this.refuse("claim", error);
      return Promise.reject(new ClaimRejected("refused", error.message, error));
    }
    return this.ask<string>("claim", (req) => ({
      type: "claim",
      req,
      cells: { container: cells.container, ranges: cells.ranges.map(copyRange) },
      ...(opts.ttlMs === undefined ? {} : { ttl_ms: opts.ttlMs }),
      ...(opts.note === undefined ? {} : { note: opts.note }),
      ...(opts.hold === undefined ? {} : { hold: opts.hold }),
    }));
  }

  /**
   * Claim elements of a list root (`Session::claim_elems`): resolves to the claim's id, `claim-N`.
   * Everyone sees it on the roster. Rejects with `ClaimRejected`: `refused` by the host (someone
   * else's claim, `MPC-E301`; a key that isn't live, `MPC-E114`), or by the channel, unsent, for
   * what the host would refuse anyway (`MPC-E304`: no keys, more than `CLAIM_KEYS_MAX`, a note over
   * `CLAIM_NOTE_MAX` bytes, a bad `ttlMs`); `onRefused` hears those too. `unsupported`, `closed` and
   * `reconnected` as on `ClaimRejectedReason`.
   */
  claim(root: string, keys: readonly string[], opts: ClaimOptions = {}): Promise<string> {
    const held = this.unavailable();
    if (held) return Promise.reject(held);
    const bad = claimProblem(keys, opts);
    if (bad) {
      const error: ErrorReport = { id: ERR.BAD_CLAIM, message: `bad claim: ${bad}` };
      this.refuse("claim", error);
      return Promise.reject(new ClaimRejected("refused", error.message, error));
    }
    return this.ask<string>("claim", (req) => ({
      type: "claim",
      req,
      root,
      keys: [...new Set(keys)],
      ...(opts.ttlMs === undefined ? {} : { ttl_ms: opts.ttlMs }),
      ...(opts.note === undefined ? {} : { note: opts.note }),
      ...(opts.hold === undefined ? {} : { hold: opts.hold }),
    }));
  }

  /**
   * Release a claim of this participant, or with no id all of theirs in this doc
   * (`Session::release_claim`): resolves to the ids released, none when none was live and theirs.
   * Rejects as `claim` does.
   */
  release(claim?: string): Promise<string[]> {
    const held = this.unavailable();
    if (held) return Promise.reject(held);
    return this.ask<string[]>("release", (req) => (claim === undefined ? { type: "release", req } : { type: "release", req, claim }));
  }

  /** Select elements, or clear with `null` (or no keys). At most 256 keys: more is refused here
   * (`onRefused`, `MPC-E310`) and the selection stays as it was. */
  setSelection(selection: ElemSelection | null): void {
    if (selection && selection.keys.length > ELEM_SELECTION_MAX) {
      const message = `selection too large: ${selection.keys.length} keys (the most is ${ELEM_SELECTION_MAX})`;
      return this.refuse("select", { id: ERR.SELECTION_TOO_LARGE, message });
    }
    const next = selection && selection.keys.length > 0 ? { root: selection.root, keys: [...new Set(selection.keys)] } : null;
    if (JSON.stringify(next) === JSON.stringify(this.selection)) return;
    this.selection = next;
    this.sendSelection();
  }

  /** Move the pointer, or clear with `null`. Batched: only the latest value goes out. A number
   * that isn't finite is refused here (`onRefused`, `MPC-E114`). */
  setPointer(point: Point | null): void {
    if (point && !finite(point.x, point.y)) {
      return this.refuse("pointer", { id: ERR.BAD_POSITION, message: "bad position: the pointer is not finite" });
    }
    const next = point && { x: point.x, y: point.y };
    if (JSON.stringify(next) === JSON.stringify(this.pointer)) return;
    this.pointer = next;
    this.pointerDue = true;
    this.batch();
  }

  /** Set the visible area, or clear with `null`. Batched as `setPointer`. Numbers that aren't
   * finite, or a negative size, are refused here (`onRefused`, `MPC-E114`). */
  setViewport(rect: Rect | null): void {
    if (rect && (!finite(rect.x, rect.y, rect.w, rect.h) || rect.w < 0 || rect.h < 0)) {
      return this.refuse("viewport", { id: ERR.BAD_POSITION, message: "bad position: the viewport is not finite, or its size is negative" });
    }
    const next = rect && { x: rect.x, y: rect.y, w: rect.w, h: rect.h };
    if (JSON.stringify(next) === JSON.stringify(this.viewport)) return;
    this.viewport = next;
    this.viewportDue = true;
    this.batch();
  }

  /**
   * In roster order: one entry for each connection that has a value, and one for a participant
   * with none (`client` absent). This connection is listed once, with `isSelf`, with the values
   * the host last reported for it.
   */
  peers(): readonly PresencePeer[] {
    const out: PresencePeer[] = [];
    for (const f of this.folded) {
      const peer = (client: number | undefined, v: Values): PresencePeer => {
        const mine = f.handle === this.welcome.self;
        const isSelf = mine && (client === undefined || client === this.welcome.client);
        return {
          handle: f.handle,
          ...(client === undefined ? {} : { client }),
          name: f.name,
          colour: f.colour,
          kind: participantKind(f.rawKind),
          rawKind: f.rawKind,
          isSelf,
          sameUser: mine && !isSelf,
          selection: v.selection,
          pointer: v.pointer,
          viewport: v.viewport,
          focus: v.focus,
          range: v.range,
          cells: v.cells,
          viewAddress: v.viewAddress,
          timePosition: v.timePosition,
          away: f.away,
        };
      };
      if (!f.perConn) {
        out.push(peer(undefined, f.top));
        continue;
      }
      const rows = [...f.conns].filter(([client, v]) => hasValue(v) || (f.handle === this.welcome.self && client === this.welcome.client));
      if (rows.length === 0) out.push(peer(undefined, noValues()));
      for (const [client, v] of rows) out.push(peer(client, v));
    }
    return out;
  }

  /** The live claims on elements, in the order they were taken (oldest first), each with its
   * actor and times: who holds what, for a view's claim marks and its activity line. */
  claims(): readonly ClaimView[] {
    return this.claimed.map((c) => ({
      ...c,
      actor: { ...c.actor },
      keys: [...c.keys],
      ...(c.cells ? { cells: { container: c.cells.container, ranges: c.cells.ranges.map(copyRange) } } : {}),
    }));
  }

  /** `claims()` changed (a claim started or ended, or a snapshot came). Returns an unsubscribe. */
  onClaims(cb: (claims: readonly ClaimView[]) => void): () => void {
    this.claimCbs.add(cb);
    return () => this.claimCbs.delete(cb);
  }

  /** A claim ended (any participant's): hands on each `claim_ended` as a `ClaimEnded`, as it
   * comes. Returns an unsubscribe. */
  onClaimEnded(cb: (e: ClaimEnded) => void): () => void {
    this.endedCbs.add(cb);
    return () => this.endedCbs.delete(cb);
  }

  /**
   * How many are watching past the host's participant cap: the
   * roster's `watching`, from its snapshot and its `watching {count}` events. Watchers have no
   * row in `peers()` and send no presence; a view shows them as this count ("+1 watching").
   */
  get watching(): number {
    return this.watchers;
  }

  /** `watching` changed. Returns an unsubscribe. */
  onWatching(cb: (count: number) => void): () => void {
    this.watchingCbs.add(cb);
    return () => this.watchingCbs.delete(cb);
  }

  /** `peers()` changed. Returns an unsubscribe. */
  onChange(cb: (peers: readonly PresencePeer[]) => void): () => void {
    this.changeCbs.add(cb);
    return () => this.changeCbs.delete(cb);
  }

  /** A presence or claim frame was refused, by the host or before sending. Returns an
   * unsubscribe. */
  onRefused(cb: (refusal: PresenceRefusal) => void): () => void {
    this.refusedCbs.add(cb);
    return () => this.refusedCbs.delete(cb);
  }

  /** A new connection's welcome: sends this connection's current values again when the host
   * takes them. Claims are not sent again: `claim` and `release` still waiting reject with
   * `reconnected`, and a claim doesn't outlive the connection that took it, so claim again. */
  reconnected(welcome: Welcome): void {
    this.welcome = welcome;
    this.rejectAsked(
      "reconnected",
      "the connection changed before the host answered, so this request's fate is unknown. A claim it may have taken has ended, or will end when the old connection closes: a claim doesn't outlive its connection, held or not. Read the roster, and claim again if you still need it.",
    );
    this.folded = [];
    this.emitChange();
    // the focus before the cells and the range: each moves the focus to its container on the host
    if (this.focus) this.sendFocus();
    if (this.cells) this.sendCells();
    if (this.range) this.sendRange();
    if (this.selection) this.sendSelection();
    if (this.pointer) this.pointerDue = true;
    if (this.viewport) this.viewportDue = true;
    if (this.viewAddress !== null) this.sendViewAddress();
    if (this.timePosition) this.sendTimePosition();
    this.flush();
  }

  /** Stop listening and sending. `claim` and `release` still waiting reject with `closed`. */
  close(): void {
    this.closed = true;
    this.unsub();
    this.rejectAsked("closed", "the channel closed before the host answered, so this request's fate is unknown.");
  }

  /** Why nothing can be asked now, if so (`cells`: a claim on cells). */
  private unavailable(cells = false): ClaimRejected | null {
    if (this.closed) return new ClaimRejected("closed", "the channel is closed");
    if (cells && !this.cellClaimsSupported) return new ClaimRejected("unsupported", "this connection's host doesn't take claims on cells (no \"cell_claims\" in welcome.features)");
    if (!cells && !this.claimsSupported) return new ClaimRejected("unsupported", "this connection's host doesn't take claim frames (no \"claims\" in welcome.features)");
    return null;
  }

  private ask<T extends string | string[]>(kind: "claim" | "release", frame: (req: number) => ClientFrame): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      const req = this.number(kind);
      this.asked.set(req, { kind, resolve: resolve as (answer: string | string[]) => void, reject });
      this.t.send(frame(req));
    });
  }

  private answer(req: number, kind: "claim" | "release", value: string | string[]): void {
    const a = this.asked.get(req);
    if (a?.kind !== kind) return; // not this channel's question (a reply from before a reconnect)
    this.asked.delete(req);
    this.reqs.delete(req);
    a.resolve(value);
  }

  private rejectAsked(reason: ClaimRejectedReason, message: string): void {
    const waiting = [...this.asked.values()];
    this.asked.clear();
    for (const a of waiting) a.reject(new ClaimRejected(reason, message));
  }

  private batch(): void {
    if (this.batching) return;
    this.batching = true;
    this.schedule(() => {
      this.batching = false;
      this.flush();
    });
  }

  private flush(): void {
    if (this.pointerDue) {
      this.pointerDue = false;
      this.send("pointer", (req) => ({ type: "pointer", req, point: this.pointer }));
    }
    if (this.viewportDue) {
      this.viewportDue = false;
      this.send("viewport", (req) => ({ type: "viewport", req, rect: this.viewport }));
    }
  }

  private sendSelection(): void {
    this.send("select", (req) => ({ type: "select", req, selection: this.selection }));
  }

  private sendFocus(): void {
    if (this.closed || !this.focusSupported) return;
    this.t.send({ type: "focus", req: this.number("focus"), focus: this.focus });
  }

  private sendRange(): void {
    if (this.closed || !this.rangeSupported) return;
    this.t.send({ type: "range", req: this.number("range"), range: this.range });
  }

  private sendViewAddress(): void {
    if (this.closed || !this.viewAddressSupported) return;
    this.t.send({ type: "view_address", req: this.number("view_address"), address: this.viewAddress });
  }

  private sendTimePosition(): void {
    if (this.closed || !this.timePositionSupported) return;
    this.t.send({ type: "time_position", req: this.number("time_position"), time_position: this.timePosition });
  }

  private sendCells(): void {
    if (this.closed || !this.cellsSupported) return;
    this.t.send({ type: "cells", req: this.number("cells"), cells: this.cells });
  }

  private send(kind: PresenceFrameType, frame: (req: number) => ClientFrame): void {
    if (this.closed || !this.supported) return;
    this.t.send(frame(this.number(kind)));
  }

  /** The next request number, remembered as `kind`. */
  private number(kind: PresenceFrameType): number {
    const req = this.nextReq++;
    this.reqs.set(req, kind);
    if (this.reqs.size > REQS_KEPT) this.reqs.delete(this.reqs.keys().next().value as number);
    this.latest[kind] = req;
    return req;
  }

  private refuse(frame: PresenceFrameType | null, error: ErrorReport): void {
    for (const cb of [...this.refusedCbs]) cb({ frame, error });
  }

  private onFrame(f: ServerFrame): void {
    if (this.closed) return;
    switch (f.type) {
      case "claimed":
        return this.answer(f.req, "claim", f.claim);
      case "released":
        return this.answer(f.req, "release", f.claims);
      case "presence_refused": {
        const asked = this.asked.get(f.req);
        const kind = asked?.kind ?? this.reqs.get(f.req) ?? null;
        this.reqs.delete(f.req);
        if (asked) {
          this.asked.delete(f.req);
          asked.reject(new ClaimRejected("refused", f.error.message, f.error));
        }
        // the value the host didn't take isn't sent again on a reconnect
        if (kind === "select" && this.latest.select === f.req) this.selection = null;
        if (kind === "pointer" && this.latest.pointer === f.req) this.pointer = null;
        if (kind === "viewport" && this.latest.viewport === f.req) this.viewport = null;
        if (kind === "focus" && this.latest.focus === f.req) this.focus = null;
        if (kind === "range" && this.latest.range === f.req) this.range = null;
        if (kind === "cells" && this.latest.cells === f.req) this.cells = null;
        if (kind === "view_address" && this.latest.view_address === f.req) this.viewAddress = null;
        if (kind === "time_position" && this.latest.time_position === f.req) this.timePosition = null;
        return this.refuse(kind, f.error);
      }
      case "roster":
        this.folded = f.entries.map(foldEntry);
        this.claimed = f.entries.flatMap(claimsOf);
        this.emitClaims();
        return this.emitChange();
      case "roster_event":
        this.foldWatching(f.event);
        if (this.foldClaim(f.event)) this.emitClaims();
        if (this.foldEvent(f.event)) this.emitChange();
        return;
      default:
        return; // the other frames are `LiveText`'s, `refused` among them
    }
  }

  private foldEvent(e: RosterEvent): boolean {
    const row = (who: Handle) => this.folded.find((r) => r.handle === who);
    const set = (who: Handle, client: number | undefined, patch: Partial<Values>): boolean => {
      const r = row(who);
      if (!r) return false;
      if (client === undefined) {
        r.top = { ...r.top, ...patch };
      } else {
        r.perConn = true;
        r.conns.set(client, { ...(r.conns.get(client) ?? noValues()), ...patch });
      }
      return true;
    };
    switch (e.kind) {
      case "snapshot":
        this.folded = e.members.map(foldEntry);
        return true;
      case "joined":
        if (row(e.who)) return false;
        this.folded.push({
          handle: e.who,
          name: e.participant.name,
          colour: e.participant.colour,
          rawKind: e.participant.kind,
          away: false,
          top: noValues(),
          conns: new Map(),
          perConn: false,
        });
        return true;
      case "left":
        this.folded = this.folded.filter((r) => r.handle !== e.who);
        return true;
      case "away": {
        const r = row(e.who);
        if (r) r.away = e.away;
        return !!r;
      }
      case "selection":
        return set(e.who, e.client, { selection: e.selection });
      case "pointer":
        return set(e.who, e.client, { pointer: e.pointer });
      case "viewport":
        return set(e.who, e.client, { viewport: e.viewport });
      case "focus":
        return set(e.who, e.client, { focus: e.focus });
      case "range":
        return set(e.who, e.client, { range: e.range });
      case "cells":
        return set(e.who, e.client, { cells: e.cells && copyCells(e.cells) });
      case "view_address":
        return set(e.who, e.client, { viewAddress: e.view_address });
      case "time_position":
        return set(e.who, e.client, { timePosition: e.time_position && { ...e.time_position } });
      default:
        return false;
    }
  }

  /** A snapshot's `watching` or a `watching` event; told when the count changes. */
  private foldWatching(e: RosterEvent): void {
    const n = e.kind === "snapshot" ? (e.watching ?? 0) : e.kind === "watching" ? e.count : this.watchers;
    if (n === this.watchers) return;
    this.watchers = n;
    for (const cb of [...this.watchingCbs]) cb(n);
  }

  /** A claim event or a snapshot, into `claimed`; whether it changed. */
  private foldClaim(e: RosterEvent): boolean {
    switch (e.kind) {
      case "snapshot":
        this.claimed = e.members.flatMap(claimsOf);
        return true;
      case "claim_started_elems": {
        const row = this.folded.find((r) => r.handle === e.who);
        const actor: ClaimActor = e.actor ?? { id: e.who, name: row?.name ?? e.who, kind: row?.rawKind ?? "", colour: row?.colour ?? "" };
        this.claimed = this.claimed.filter((c) => c.claim !== e.claim);
        this.claimed.push({
          claim: e.claim,
          actor,
          root: e.root,
          keys: [...e.keys],
          note: e.note,
          hold: e.hold,
          startedMs: typeof e.started_ms === "number" ? e.started_ms : null,
          expiresMs: e.expires_ms,
        });
        return true;
      }
      case "claim_started_cells": {
        this.claimed = this.claimed.filter((c) => c.claim !== e.claim);
        this.claimed.push({
          claim: e.claim,
          actor: this.cellsClaimActor(e),
          root: "",
          keys: [],
          note: e.note,
          hold: e.hold,
          startedMs: typeof e.started_ms === "number" ? e.started_ms : null,
          expiresMs: e.expires_ms,
          cells: { container: e.container, ranges: e.ranges.map(copyRange) },
        });
        return true;
      }
      case "claim_ended": {
        const before = this.claimed.length;
        this.claimed = this.claimed.filter((c) => c.claim !== e.claim);
        if (this.endedCbs.size > 0) {
          const ended: ClaimEnded = { claim: e.claim, who: e.who, how: claimEndReason(e.how), keys: Array.isArray(e.keys) ? [...e.keys] : [] };
          for (const cb of [...this.endedCbs]) cb(ended);
        }
        return this.claimed.length !== before;
      }
      default:
        return false;
    }
  }

  /** A claim on cells' actor: the event's own, so a view needs no roster row. */
  private cellsClaimActor(e: Extract<RosterEvent, { kind: "claim_started_cells" }>): ClaimActor {
    return { id: e.actor.id, name: e.actor.name, kind: e.actor.kind, colour: e.actor.colour };
  }

  private emitClaims(): void {
    if (this.claimCbs.size === 0) return;
    const claims = this.claims();
    for (const cb of [...this.claimCbs]) cb(claims);
  }

  private emitChange(): void {
    if (this.changeCbs.size === 0) return;
    const peers = this.peers();
    for (const cb of [...this.changeCbs]) cb(peers);
  }
}

/** A roster entry's live claims on elements, then on cells, its own row giving the actor. */
function claimsOf(m: RosterEntry): ClaimView[] {
  const cells = (m.cell_claims ?? []).map(
    (c): ClaimView => ({
      claim: c.claim,
      actor: { id: m.handle, name: m.name, kind: m.kind, colour: m.colour },
      root: "",
      keys: [],
      note: c.note,
      hold: c.hold,
      startedMs: typeof c.started_ms === "number" ? c.started_ms : null,
      expiresMs: c.expires_ms,
      cells: { container: c.container, ranges: c.ranges.map(copyRange) },
    }),
  );
  return [...elemClaimsOf(m), ...cells];
}

/** A roster entry's live claims on elements. */
function elemClaimsOf(m: RosterEntry): ClaimView[] {
  return (m.elem_claims ?? []).map((c) => ({
    claim: c.claim,
    actor: { id: m.handle, name: m.name, kind: m.kind, colour: m.colour },
    root: c.root,
    keys: [...c.keys],
    note: c.note,
    hold: c.hold,
    startedMs: typeof c.started_ms === "number" ? c.started_ms : null,
    expiresMs: c.expires_ms,
  }));
}

function foldEntry(m: RosterEntry): Folded {
  const conns = new Map<number, Values>();
  for (const d of m.devices ?? []) {
    conns.set(d.client, {
      selection: d.selection ?? null,
      pointer: d.pointer ?? null,
      viewport: d.viewport ?? null,
      focus: d.focus ?? null,
      range: d.range ?? null,
      cells: d.cells ?? null,
      viewAddress: d.view_address ?? null,
      timePosition: d.time_position ?? null,
    });
  }
  return {
    handle: m.handle,
    name: m.name,
    colour: m.colour,
    rawKind: m.kind,
    away: m.away,
    top: {
      selection: m.selection ?? null,
      pointer: m.pointer ?? null,
      viewport: m.viewport ?? null,
      focus: m.focus ?? null,
      range: m.range ?? null,
      cells: m.cells ?? null,
      viewAddress: m.view_address ?? null,
      timePosition: m.time_position ?? null,
    },
    conns,
    perConn: conns.size > 0,
  };
}
