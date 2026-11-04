// Presence and marks as data (`LiveText.participants()`, `LiveText.marks()`): what a host needs
// to draw other people's carets and the person's rejected changes in its own editor UI. No
// DOM: the library holds the data and says when it changed; the host draws. See PRESENCE.md.
//
// Offsets here are UTF-16 code units of the replica's current text (what CodeMirror and a JS
// string index use), not the UTF-8 bytes the wire and `carets()` use.

import type { OpId, Presence } from "./live";
import { utf8Len } from "./hunk";
import { ERR } from "./wire";
import type { ClientFrame, ElemSelection, ErrorReport, Handle, Point, Rect, RosterEntry, RosterEvent, ServerFrame, Transport, Welcome } from "./wire";

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
  away: boolean;
}

/** A presence frame the host refused, by the `req` the channel sent it with. */
export interface PresenceRefusal {
  /** The refused frame's type; `null` when the refusal names a request too old to remember. */
  frame: PresenceFrameType | null;
  error: ErrorReport;
}

/** The frames a `PresenceChannel` sends, each refusable. */
export type PresenceFrameType = "select" | "pointer" | "viewport" | "claim" | "release";

/** The name a host lists in `welcome.features` when it takes the presence frames. */
export const PRESENCE_FEATURE = "presence";

/** The name a host lists in `welcome.features` when it takes the claim frames. */
export const CLAIMS_FEATURE = "claims";

/** The most keys in one claim. */
export const CLAIM_KEYS_MAX = 256;

/** The longest note on a claim, in UTF-8 bytes. */
export const CLAIM_NOTE_MAX = 1024;

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
}

const noValues = (): Values => ({ selection: null, pointer: null, viewport: null });
const hasValue = (v: Values) => v.selection !== null || v.pointer !== null || v.viewport !== null;

/** What the host would refuse (`MPC-E304`) about a claim, in its words. */
function claimProblem(keys: readonly string[], opts: ClaimOptions): string | null {
  if (keys.length === 0) return "a claim on elements names at least one";
  if (keys.length > CLAIM_KEYS_MAX) return `${keys.length} elements in one claim; at most ${CLAIM_KEYS_MAX}`;
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
  /** What is waiting for the next batch. */
  private pointerDue = false;
  private viewportDue = false;
  private batching = false;
  private folded: Folded[] = [];
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
    if (this.selection) this.sendSelection();
    if (this.pointer) this.pointerDue = true;
    if (this.viewport) this.viewportDue = true;
    this.flush();
  }

  /** Stop listening and sending. `claim` and `release` still waiting reject with `closed`. */
  close(): void {
    this.closed = true;
    this.unsub();
    this.rejectAsked("closed", "the channel closed before the host answered, so this request's fate is unknown.");
  }

  /** Why nothing can be asked now, if so. */
  private unavailable(): ClaimRejected | null {
    if (this.closed) return new ClaimRejected("closed", "the channel is closed");
    if (!this.claimsSupported) return new ClaimRejected("unsupported", "this connection's host doesn't take claim frames (no \"claims\" in welcome.features)");
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
        return this.refuse(kind, f.error);
      }
      case "roster":
        this.folded = f.entries.map(foldEntry);
        return this.emitChange();
      case "roster_event":
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
      default:
        return false;
    }
  }

  private emitChange(): void {
    if (this.changeCbs.size === 0) return;
    const peers = this.peers();
    for (const cb of [...this.changeCbs]) cb(peers);
  }
}

function foldEntry(m: RosterEntry): Folded {
  const conns = new Map<number, Values>();
  for (const d of m.devices ?? []) {
    conns.set(d.client, { selection: d.selection ?? null, pointer: d.pointer ?? null, viewport: d.viewport ?? null });
  }
  return {
    handle: m.handle,
    name: m.name,
    colour: m.colour,
    rawKind: m.kind,
    away: m.away,
    top: { selection: m.selection ?? null, pointer: m.pointer ?? null, viewport: m.viewport ?? null },
    conns,
    perConn: conns.size > 0,
  };
}
