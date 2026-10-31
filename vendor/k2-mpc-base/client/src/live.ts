// `LiveText`: one text root, edited live over the hunk channel (amendment E-LIVE). The client
// keeps the server's text at the latest version it has seen and its own unacknowledged
// changes on top: at most one in flight (rule 0), the rest queued and sent, composed per undo
// group, after the ack. Incoming changes are carried through them with the kit's rules
// (`rebase.ts`), so the client's text is the server's once everything is acknowledged.
//
// Nothing typed is dropped silently: a change the server refuses (or that the client can see
// it will refuse) leaves the text, and `onRejected` hands it to the host with where it was,
// so the person can re-apply it at the new base.

import { apply, compose, coalesce, invert, utf8Len, validate, type Hunk } from "./hunk";
import { Bias, transform } from "./position";
import { Carried } from "./rebase";
import {
  ERR,
  FRAME_MARGIN,
  FRAME_MAX,
  type ChangeEvent,
  type ClientFrame,
  type CloseReason,
  type ErrorReport,
  type Handle,
  type RosterEntry,
  type RosterEvent,
  type ServerFrame,
  type Transport,
} from "./wire";

/** A local change's id, as `applyLocal` returns it. */
export type OpId = number;

export interface ChangeInfo {
  /** The change, on the client's text before it (UTF-8 bytes). */
  hunks: Hunk[];
  /** Who made it; `null` for the client's own edits and for a rejected change leaving. */
  by: Handle | null;
  /** The client's own `applyLocal` (already in the editor). */
  local: boolean;
}

/** A local change the server refused (or would): not in the text any more. */
export interface Rejection {
  id: number;
  /** The `applyLocal` ids it held (one entry composes consecutive edits of a group). */
  ops: OpId[];
  /** The change as made, on the text at `base` (UTF-8 bytes). */
  hunks: Hunk[];
  /** The version the change was made at. */
  base: number;
  /** Where it was, in the current text (moves with every change until dismissed). */
  region: { start: number; end: number };
  /** The text it would have inserted (what the person typed). */
  text: string;
  /** The refusal, when the server sent one (`MPC-E111` has `details.conflicts`). */
  error: ErrorReport | null;
  /**
   * For a change whose reply was lost (`MPC-E000`): the version it was refused at, the client
   * ID of the connection it went out on, and whether an undo or redo was unanswered on that
   * connection when it went (its change would carry the same ID). `presence` and `onLanded`
   * read them.
   */
  asOf?: { version: number; client: number | undefined; undoInFlight: boolean };
}

/**
 * Whether single-hunk Re-apply can put a rejection back where it was (`LiveText.reapplyGuard`):
 * `ok`, or why not. Re-apply inserts the typed text at the region's start, which is the server's
 * place for it only for a pure insertion (`not-insertion`: it deleted or replaced text, which
 * Re-apply wouldn't redo), whose region came through the server's changes (`reloaded`: a reload
 * since moved it by a diff), with nothing else inserted at that point since (`tie`: the server
 * puts a later change after it; Re-apply would put it before), and no text around that point
 * deleted or replaced since (`deleted-around`: the region snapped to the deletion's start; the
 * server refuses an insertion inside a deletion it applied, and never saw one it refused).
 * `multi-hunk`: its text would go back in one place, not each hunk's.
 */
export type ReapplyGuard = "ok" | "multi-hunk" | "not-insertion" | "reloaded" | "tie" | "deleted-around";

/** Whether a rejected change whose reply was lost has landed (`LiveText.presence`). */
export type Presence = "present" | "absent" | "unknown";

/** A change whose reply was lost, seen landing after all (`onLanded`). */
export interface Landed {
  rejection: Rejection;
  /** The version it landed at. */
  version: number;
}

/** A change whose reply was lost landed after the person re-applied it (`onDuplicateRisk`):
 * its text may now be in the document twice. Only a host that breaks WIRE.md's reconnect rule
 * lets this happen. */
export interface DuplicateRisk {
  rejection: Rejection;
  /** The version it landed at. */
  landed: number;
  /** The version the client was at when the person re-applied it. */
  reapplied: number;
}

export interface RemoteCaret {
  handle: Handle;
  name: string;
  colour: string;
  /** In UTF-8 bytes of the client's current text. */
  anchor: number;
  head: number;
}

/** The reply to an `undo` or `redo` (the `undone` frame). */
export interface UndoResult {
  /** The request's wire op (`undo()` / `redo()` return it). */
  op: number;
  kind: "undo" | "redo";
  /** Steps undone or redone; 0: there was nothing to undo or redo. */
  steps: number;
  /** The doc's version after it. */
  version: number;
  /** Why there was nothing, when the kit says (`MPC-E206`, `MPC-E207`). */
  notice: ErrorReport | null;
}

export interface LiveOptions {
  /** The text root this replica edits (default `source`). */
  root?: string;
  /** This connection's client ID in the doc (`welcome.client`): identifies its own change in a
   * `since` reply whose ack was lost. A reconnect gives a new one: `reconnected`. */
  client?: number;
  /** This connection's participant handle, to tell its own roster entry apart. */
  self?: Handle;
}

/** A local change not yet acknowledged: `hunks` on `before`. */
interface Entry {
  hunks: Hunk[];
  /** `hunks` as the server will carry them (E-LIVE r2.3): as sent, through every change since
   * its base; reset to exactly what's sent when it's sent. `hunks` is its canonical form. */
  carried: Carried;
  before: string;
  group: string | undefined;
  ops: OpId[];
  /** Set once sent. */
  op?: number;
  base?: number;
  /** The as-made form, for a rejection. */
  made: { hunks: Hunk[]; base: number };
  /** The connection's client ID when it was sent: a change applied from it carries this one. */
  sentClient?: number;
  /** In flight, but the server will refuse it (it met competing text): not in the text. */
  doomed?: Rejection;
}

type Listener<T> = (v: T) => void;

interface Events {
  change: ChangeInfo;
  roster: RosterEntry[];
  rejected: Rejection;
  ack: { ops: OpId[]; version: number };
  reload: { reason: string; error: ErrorReport | null };
  error: ErrorReport;
  rosterEvent: RosterEvent;
  undo: UndoResult;
  closed: { reason: CloseReason; error: ErrorReport | null };
  landed: Landed;
  duplicateRisk: DuplicateRisk;
}

/** At most this many lost changes are watched for a late landing (the oldest go first). */
const WATCH_MAX = 64;

const HISTORY = 512;

export class LiveText {
  readonly root: string;
  private serverText: string;
  private _version: number;
  private _text: string;
  private inflight: Entry | null = null;
  private queue: Entry[] = [];
  private nextOp = 1;
  private nextWire = 1;
  private nextRejection = 1;
  private sinceOp: number | null = null;
  /** The edit in flight when the `since` went out (its reply settles it). */
  private sinceFor: number | null = null;
  /** An ack that came before changes it follows (filled by `since` first). */
  private stash: { op: number; version: number; hunks: Hunk[] } | null = null;
  /** Edits already answered (late or duplicate replies are ignored). */
  private settled = new Set<number>();
  private watching = 0;
  private clientId: number | undefined;
  private readonly selfHandle: Handle | undefined;
  /** The server's changes, with the client ID each came from (own acks included). */
  private history: Array<{ version: number; hunks: Hunk[]; client: number | undefined }> = [];
  /** The history holds every change after this version. */
  private historyFrom = 0;
  /**
   * The roots whose event at `version` this replica has applied (`"all"`: every root's, as for
   * a snapshot at that version). A change touching several roots is one event per root, all
   * at one version: each (root, version) is applied once, in whatever order they come.
   */
  private seenAt: Set<string> | "all" = "all";
  /** Rejections whose region can't be trusted for Re-apply: made by a reload, or open across
   * one (their regions moved through a diff, not the server's changes). */
  private reloaded = new Set<number>();
  /** Rejections something else was inserted at exactly the point of, since: the server would
   * put the change on the other side of it (ours after, at a tie). */
  private tied = new Set<number>();
  /** Rejections whose point was inside (or at the edge of) text deleted or replaced since: the
   * region snapped to the deletion's start, and stays there if that deletion is undone or
   * refused. */
  private snapped = new Set<number>();
  /** Client IDs whose connection went with an undo or redo unanswered. */
  private undoLostOn = new Set<number>();
  /** Changes whose reply was lost, by the client ID they went out on, watched for a late
   * landing: open (in `rejected`), dismissed, or re-applied at a version. */
  private lost = new Map<number, { rejection: Rejection; state: "open" | "dismissed" | { reapplied: number } }>();
  private _roster: RosterEntry[] = [];
  private _rejected: Rejection[] = [];
  private cursor: { anchor: number; head: number } | null = null;
  private cursorSent: string | null = null;
  private visible: boolean | null = null;
  /** The connection is gone (`disconnected`): edits wait in the queue until `reconnected`. */
  private offline = false;
  private view: { root: string; first: number; last: number } | null = null;
  /** The `snapshot` request out (its reply resets the replica). */
  private snapshotOp: number | null = null;
  /** `undo` / `redo` requests not yet answered. */
  private undoOps = new Map<number, "undo" | "redo">();
  private dead = false;
  private readonly unsub: () => void;
  private readonly ls: { [K in keyof Events]: Set<Listener<Events[K]>> } = {
    change: new Set(),
    roster: new Set(),
    rejected: new Set(),
    ack: new Set(),
    reload: new Set(),
    error: new Set(),
    rosterEvent: new Set(),
    undo: new Set(),
    closed: new Set(),
    landed: new Set(),
    duplicateRisk: new Set(),
  };

  constructor(
    snapshot: { text: string; version: number },
    private readonly t: Transport,
    opts: LiveOptions = {},
  ) {
    this.root = opts.root ?? "source";
    this.clientId = opts.client;
    this.selfHandle = opts.self;
    this.serverText = snapshot.text;
    this._text = snapshot.text;
    this._version = snapshot.version;
    this.historyFrom = snapshot.version;
    this.unsub = t.onFrame((f) => this.onFrame(f));
  }

  /** The client's text: the server's at `version`, with its own pending changes. */
  get text(): string {
    return this._text;
  }
  /** The latest server version this client has seen. */
  get version(): number {
    return this._version;
  }
  /** Local changes not yet acknowledged (in flight plus queued). */
  get pending(): number {
    return (this.inflight ? 1 : 0) + this.queue.length;
  }
  /** Rejected changes not yet dismissed or re-applied. */
  get rejected(): readonly Rejection[] {
    return this._rejected;
  }
  get roster(): readonly RosterEntry[] {
    return this._roster;
  }
  /** Participants past the doc's limit, shown only as this count (K11). */
  get watchers(): number {
    return this.watching;
  }

  /**
   * A local edit: `hunks` on the current `text` (UTF-8 bytes, sorted, non-overlapping). Applied
   * at once (optimistic); sent when nothing else is in flight, else queued and composed with
   * the queued edits of the same `group` (an undo group token, K4). Throws on invalid hunks.
   */
  applyLocal(hunks: readonly Hunk[], opts: { group?: string } = {}): OpId {
    if (this.dead) throw new Error("LiveText is closed");
    const bad = validate(this._text, hunks);
    if (bad) throw new Error(`invalid hunks (${bad.kind} at ${bad.index})`);
    const id = this.nextOp++;
    const hs = coalesce(hunks);
    if (hs.length === 0) return id;
    const before = this._text;
    this._text = apply(before, hs);
    this.moveRejected(hs);
    this.emit("change", { hunks: hs, by: null, local: true });
    const last = this.queue[this.queue.length - 1];
    if (last && last.group === opts.group) {
      last.hunks = compose(last.before, last.hunks, hs);
      last.carried = Carried.of(last.hunks);
      last.made = { hunks: last.hunks, base: this._version };
      last.ops.push(id);
    } else {
      this.queue.push({
        hunks: hs,
        carried: Carried.of(hs),
        before,
        group: opts.group,
        ops: [id],
        made: { hunks: hs, base: this._version },
      });
    }
    this.flush();
    return id;
  }

  /** Set this connection's caret or selection (UTF-8 bytes of `text`). */
  setCursor(anchor: number, head: number): void {
    this.cursor = { anchor, head };
    this.sendCursor();
  }

  /** This connection's client ID, as last given (`LiveOptions.client`, `reconnected`). */
  get client(): number | undefined {
    return this.clientId;
  }

  /**
   * Ask the server to undo this participant's last change. Its text arrives as a `change`
   * (never filtered out: it isn't the edit an `ack` answers); then `onUndo` with the steps
   * (0: nothing to undo), or `onError` when refused. Returns the request's op.
   */
  undo(): number {
    return this.undoRedo("undo");
  }
  redo(): number {
    return this.undoRedo("redo");
  }

  private undoRedo(kind: "undo" | "redo"): number {
    const op = this.nextWire++;
    this.undoOps.set(op, kind);
    this.t.send({ type: kind, op });
    return op;
  }

  /** Whether this connection's view is in front (the host's `set_visible`); re-sent after a
   * reconnect. */
  setVisible(visible: boolean): void {
    this.visible = visible;
    this.t.send({ type: "visible", visible });
  }

  /** The lines this connection shows (1-based, inclusive; the host's `set_view`); re-sent after
   * a reconnect. */
  setView(first: number, last: number): void {
    this.view = { root: this.root, first, last };
    this.t.send({ type: "view", ...this.view });
  }

  /**
   * The transport reconnected (a new socket, so a new connection and a new client ID). Takes
   * the `welcome`: its `client` replaces this one's, the caret, visibility and view are sent
   * again, and the replica catches up: from `text` when the host sent it (resuming was
   * impossible: `reset`, pending changes go to `onRejected`), else with `since` (`resync`),
   * whose reply settles the edit in flight (listed: applied; not listed: `onRejected`,
   * `MPC-E000`, its text kept). Frames sent while disconnected were dropped, never replayed.
   */
  reconnected(w: { client: number; version?: number; text?: string }): void {
    if (this.dead) return;
    this.offline = false;
    // an undo or redo unanswered on the old connection may still land under its ID
    if (this.undoOps.size > 0 && this.clientId !== undefined) this.undoLostOn.add(this.clientId);
    this.clientId = w.client;
    this.cursorSent = null;
    this.undoOps.clear(); // their replies went with the old socket
    if (this.visible !== null) this.t.send({ type: "visible", visible: this.visible });
    if (this.view) this.t.send({ type: "view", ...this.view });
    if (w.text !== undefined && w.version !== undefined) {
      this.reset({ text: w.text, version: w.version });
      this.flush();
      this.sendCursor();
      return;
    }
    if (this.snapshotOp !== null) return this.reload(); // a read is safe to ask again
    this.resync();
    // edits made while disconnected go now, after the `since` (they were never sent)
    this.flush();
    this.sendCursor();
  }

  /**
   * The transport's connection is gone (its `onClose`): edits made from now on wait in the
   * queue, unsent, until `reconnected`, which sends them on the new connection (with its
   * client ID). An edit already handed to the transport stays in flight: the resync settles
   * it. Without this, an edit made while disconnected would be handed to the transport,
   * dropped there, and come back as `MPC-E000`.
   */
  disconnected(): void {
    this.offline = true;
  }

  /**
   * Whether a rejected change whose reply was lost (`asOf`) has landed, by the server's own
   * record, not by its text: `present` when a change from the client ID it went out on, at a
   * version after the one it was refused at, is in the history (that ID had at most this one
   * edit in flight: rule 0; an earlier, acknowledged edit from it doesn't count); `absent` when
   * none is, and the history covers every version since; `unknown` when it doesn't (a reset,
   * or past its length), when an undo or redo was unanswered on that connection (its change
   * would carry the same ID), or for any other rejection. Client IDs are never reused on a doc
   * (the kit draws 53 random bits and skips any the doc has used; an ID that joined and never
   * wrote could come again after an unload, at odds of 2^-53, which is accepted).
   */
  presence(id: number): Presence {
    const r = this._rejected.find((x) => x.id === id) ?? [...this.lost.values()].find((l) => l.rejection.id === id)?.rejection;
    const a = r?.asOf;
    if (!a || a.client === undefined) return "unknown";
    if (a.version < this.historyFrom) return "unknown";
    const from = a.client;
    const landed = this.history.some((h) => h.version > a.version && h.client === from);
    if (landed) return a.undoInFlight ? "unknown" : "present";
    return "absent";
  }

  /**
   * The person re-applied a change whose reply was lost (as a new edit, now): if the original
   * lands after all, `onDuplicateRisk` says so.
   */
  noteReapplied(id: number): void {
    for (const l of this.lost.values()) if (l.rejection.id === id) l.state = { reapplied: this._version };
  }

  /** Ask the host for the root's text now (`snapshot`) and `reset` to it: the answer to
   * `onReload`. */
  reload(): void {
    if (this.dead) return;
    this.snapshotOp = this.nextWire++;
    this.t.send({ type: "snapshot", op: this.snapshotOp });
  }

  /** Remote carets, mapped to the current text. */
  carets(): RemoteCaret[] {
    const out: RemoteCaret[] = [];
    for (const r of this._roster) {
      const c = r.cursor;
      if (r.self || r.handle === this.selfHandle || !c || c.root !== this.root) continue;
      const v = c.version ?? this._version;
      const anchor = this.mapFromVersion(c.anchor, v);
      const head = this.mapFromVersion(c.head, v);
      if (anchor === null || head === null) continue;
      out.push({ handle: r.handle, name: r.name, colour: r.colour, anchor, head });
    }
    return out;
  }

  /** Forget a rejection (the host showed it, or the person declined it). */
  dismiss(id: number): void {
    for (const l of this.lost.values()) if (l.rejection.id === id && l.state === "open") l.state = "dismissed";
    this._rejected = this._rejected.filter((r) => r.id !== id);
    this.forget(id);
  }

  /** Whether Re-apply can put rejection `id` back where it was (`ReapplyGuard`); `null` when
   * there's no such rejection. Presence (`presence`) is the caller's, before this. */
  reapplyGuard(id: number): ReapplyGuard | null {
    const r = this._rejected.find((x) => x.id === id);
    if (!r) return null;
    if (r.hunks.length > 1) return "multi-hunk";
    const h = r.hunks[0];
    if (!h || h.start !== h.end) return "not-insertion";
    if (this.reloaded.has(id)) return "reloaded";
    if (this.tied.has(id)) return "tie";
    if (this.snapped.has(id)) return "deleted-around";
    return "ok";
  }

  /** Re-apply a rejection's typed text at the start of its region, as a new local edit. Only
   * when `reapplyGuard` says `ok`; anything else is refused (`null`, the rejection kept): a
   * later version restores changes through the server's rebase rule. */
  reapply(id: number, opts: { group?: string } = {}): OpId | null {
    const r = this._rejected.find((x) => x.id === id);
    if (!r) return null;
    if (this.reapplyGuard(id) !== "ok") return null;
    this.dismiss(id);
    if (!r.text) return null;
    return this.applyLocal([{ start: r.region.start, end: r.region.start, insert: r.text }], opts);
  }

  /**
   * Start again from a fresh snapshot (after `onReload`). Pending changes were already
   * handed to `onRejected`.
   */
  reset(snapshot: { text: string; version: number }): void {
    this.rejectAll(null);
    // every open region now moves by a diff of the texts, not the server's changes
    for (const r of this._rejected) this.reloaded.add(r.id);
    const old = this._text;
    this.serverText = snapshot.text;
    this._text = snapshot.text;
    this._version = snapshot.version;
    this.seenAt = "all"; // the snapshot has every root's change at its version
    this.history = [];
    this.historyFrom = snapshot.version;
    this.sinceOp = null;
    this.sinceFor = null;
    this.stash = null;
    this.snapshotOp = null;
    const d = diffText(old, snapshot.text);
    if (d.length) {
      this.moveRejected(d);
      this.emit("change", { hunks: d, by: null, local: false });
    }
  }

  close(): void {
    this.dead = true;
    this.unsub();
  }

  onChange(cb: Listener<ChangeInfo>): () => void {
    return this.on("change", cb);
  }
  onRoster(cb: Listener<RosterEntry[]>): () => void {
    return this.on("roster", cb);
  }
  /** A local change left the text, refused: the host should show it and offer re-apply. */
  onRejected(cb: Listener<Rejection>): () => void {
    return this.on("rejected", cb);
  }
  /** Local changes the server applied. */
  onAck(cb: Listener<{ ops: OpId[]; version: number }>): () => void {
    return this.on("ack", cb);
  }
  /** The client can't continue from its version (`MPC-E112`, or a divergence): fetch a
   * snapshot and call `reset`. */
  onReload(cb: Listener<{ reason: string; error: ErrorReport | null }>): () => void {
    return this.on("reload", cb);
  }
  /** Every roster delta as the host sent it (claims and notes included). */
  onRosterEvent(cb: Listener<RosterEvent>): () => void {
    return this.on("rosterEvent", cb);
  }
  /** A refused undo, redo or other request that wasn't typed text, and the host's `error`
   * frames. */
  onError(cb: Listener<ErrorReport>): () => void {
    return this.on("error", cb);
  }
  /** The reply to `undo()` / `redo()`: how many steps (0: nothing to undo). */
  onUndo(cb: Listener<UndoResult>): () => void {
    return this.on("undo", cb);
  }
  /** A `closed` frame came through the transport (most hosts report it via
   * `LiveTransport.onClose` instead). */
  /** A change whose reply was lost (`MPC-E000`) landed after all: it left `rejected`. */
  onLanded(cb: Listener<Landed>): () => void {
    return this.on("landed", cb);
  }
  /** A change whose reply was lost landed after the person re-applied it: it may be in the
   * text twice. */
  onDuplicateRisk(cb: Listener<DuplicateRisk>): () => void {
    return this.on("duplicateRisk", cb);
  }
  onClosed(cb: Listener<{ reason: CloseReason; error: ErrorReport | null }>): () => void {
    return this.on("closed", cb);
  }

  // ---- frames

  private onFrame(f: ServerFrame): void {
    if (this.dead) return;
    switch (f.type) {
      case "ack":
        return this.onAckFrame(f.op, f.version, f.hunks);
      case "refused":
        return this.onRefused(f.op, f.error);
      case "change":
        // while a `since` is out, its reply covers every change up to then
        if (this.sinceOp === null) this.onEvent(f.event);
        return;
      case "events":
        return this.onEvents(f.op, f.events);
      case "roster":
        this._roster = f.entries.map((e) => this.stamp(e));
        this.emit("roster", this._roster);
        return;
      case "roster_event":
        return this.foldRosterEvent(f.event);
      case "snapshot":
        if (f.op !== this.snapshotOp) return;
        this.snapshotOp = null;
        this.reset({ text: f.text, version: f.version });
        this.sendCursor();
        return;
      case "undone":
        return this.onUndone(f.op, f.steps, f.version, f.notice ?? null);
      case "error":
        this.emit("error", f.error);
        return;
      case "closed":
        this.emit("closed", { reason: f.reason, error: f.error ?? null });
        return;
      case "welcome":
        return; // the transport's (`LiveTransport.connect`); a reconnect is `reconnected`
    }
  }

  private onUndone(op: number, steps: number, version: number, notice: ErrorReport | null): void {
    const kind = this.undoOps.get(op);
    if (!kind) return;
    this.undoOps.delete(op);
    // its changes come first; one missing means lost frames
    if (version > this._version) this.askSince();
    this.emit("undo", { op, kind, steps, version, notice });
  }

  /**
   * Ask for the changes since this client's version (`events_since`): after a gap, an ack
   * past the next version, or when the host suspects lost frames (a reconnect). The reply
   * also settles the change in flight: sent after it, it lists it if it was applied.
   */
  resync(): void {
    if (this.dead) return;
    // a `since` already out may have lost its reply: a new one replaces it
    this.sinceOp = this.nextWire++;
    this.sinceFor = this.inflight?.op ?? null;
    this.t.send({ type: "since", op: this.sinceOp, version: this._version });
  }

  private askSince(): void {
    if (this.sinceOp === null) this.resync();
  }

  private onAckFrame(op: number, version: number, hunks: Hunk[]): void {
    const e = this.inflight;
    if (!e || e.op !== op) return; // settled already (e.g. by a `since` reply)
    // changes before ours haven't reached this client: fill the gap first (an ack that
    // applied nothing is at the server's version then, so any version ahead is a gap)
    if (version > this._version + (hunks.length > 0 ? 1 : 0)) {
      this.stash = { op, version, hunks };
      return this.askSince();
    }
    this.settleInflight(e, version, hunks);
  }

  /** The change in flight was applied as `hunks`, now at `version` (or nothing applied). */
  private settleInflight(e: Entry, version: number, hunks: Hunk[]): void {
    this.inflight = null;
    this.settled.add(e.op!);
    if (this.stash?.op === e.op) this.stash = null;
    if (e.doomed) return this.diverged("the server applied a change the client saw conflict");
    const after = apply(e.before, e.hunks);
    if (hunks.length > 0) {
      const at = this.placeOf(version, this.root);
      if (at === "seen" || at === "gap") return this.diverged(`ack at ${version} after ${this._version}`);
      const got = apply(this.serverText, hunks);
      if (got !== after) return this.diverged("the server's rebase differs from the client's");
      this.serverText = got;
      this.advance(version, this.root, hunks, e.sentClient ?? this.clientId);
    } else if (after !== this.serverText) {
      return this.diverged("the server applied nothing where the client changed text");
    }
    this.emit("ack", { ops: e.ops, version: this._version });
    this.flush();
  }

  private onRefused(op: number, error: ErrorReport): void {
    this.undoOps.delete(op);
    if (op === this.snapshotOp) {
      this.snapshotOp = null;
      this.emit("error", error);
      return;
    }
    if (op === this.sinceOp) {
      this.sinceOp = null;
      this.sinceFor = null;
      return this.reloadNeeded("the changes since aren't available", error);
    }
    const e = this.inflight;
    if (!e || e.op !== op) {
      if (!this.settled.has(op)) this.emit("error", error);
      return;
    }
    this.refuseInflight(e, error);
  }

  private refuseInflight(e: Entry, error: ErrorReport): void {
    this.inflight = null;
    this.settled.add(e.op!);
    if (this.stash?.op === e.op) this.stash = null;
    if (error.id === ERR.BASE_EXPIRED) {
      this.queue.unshift(e);
      return this.reloadNeeded("base expired", error);
    }
    if (e.doomed) {
      e.doomed.error = error;
      this._rejected.push(e.doomed);
      this.emit("rejected", e.doomed);
    } else {
      // not foreseen (a rate limit, a bad hunk, a conflict the client didn't see): take it out
      const rej = this.rejection(e, error);
      if (error.id === ERR.NOT_APPLIED) {
        const client = e.sentClient;
        rej.asOf = { version: this._version, client, undoInFlight: client !== undefined && this.undoLostOn.has(client) };
        if (client !== undefined) {
          this.lost.set(client, { rejection: rej, state: "open" });
          if (this.lost.size > WATCH_MAX) this.lost.delete(this.lost.keys().next().value!);
        }
      }
      const inv = invert(e.before, e.hunks);
      const reg = this.regionOf(e);
      this.recarry(inv, this.serverText, [{ rej, ...reg }], null);
    }
    this.flush();
  }

  private onEvents(op: number, events: ChangeEvent[]): void {
    if (op !== this.sinceOp) return;
    const forOp = this.sinceFor;
    this.sinceOp = null;
    this.sinceFor = null;
    for (const ev of events) {
      const at = this.placeOf(ev.version, ev.root);
      if (at === "seen") continue;
      const e = this.inflight;
      if (e && e.op === forOp && at !== "gap" && ev.root === this.root && this.isOwn(ev, e)) {
        this.settleInflight(e, ev.version, coalesce(ev.hunks));
        continue;
      }
      this.onEvent(ev);
      if (this.sinceOp !== null) return; // a gap inside the reply: asked again
    }
    // sent after the change in flight, the reply would list it had it been applied: it wasn't
    const e = this.inflight;
    const st = this.stash;
    if (e && st && st.op === e.op && st.hunks.length === 0 && this._version >= st.version) {
      // its ack said nothing applied (already in the text): settled, now that the gap is filled
      this.settleInflight(e, this._version, []);
    } else if (e && e.op === forOp) {
      this.refuseInflight(e, {
        id: ERR.NOT_APPLIED,
        message: "the change wasn't applied (its reply was lost)",
      });
    }
    this.stash = null;
  }

  /** Whether `ev` is the change in flight, as the server applied it. Applied, it carries the
   * client ID of the connection it was sent on (`sentClient`), which a reconnect since
   * replaced: either ID is this client's for this edit. */
  private isOwn(ev: ChangeEvent, e: Entry): boolean {
    if (this.stash && this.stash.op === e.op) return this.stash.hunks.length > 0 && ev.version === this.stash.version;
    const ids = [e.sentClient, this.clientId].filter((c): c is number => c !== undefined);
    if (ids.length > 0 && !ids.includes(ev.client)) return false;
    const hs = coalesce(ev.hunks);
    return hs.length === e.hunks.length && hs.every((h, i) => sameHunk(h, e.hunks[i]));
  }

  private onEvent(ev: ChangeEvent): void {
    const at = this.placeOf(ev.version, ev.root);
    if (at === "seen") return; // this root's event at that version is in already
    if (at === "gap") return this.askSince();
    if (ev.root !== this.root) {
      this.advance(ev.version, ev.root, [], ev.client);
      return;
    }
    const bad = validate(this.serverText, ev.hunks);
    if (bad) return this.diverged(`an incoming change doesn't fit (${bad.kind})`);
    const hs = coalesce(ev.hunks);
    this.advance(ev.version, ev.root, hs, ev.client);
    const old = this.serverText;
    this.serverText = apply(old, hs);
    this.recarry(hs, this.serverText, [], ev.pid);
    this.checkLanded(ev);
    this.sendCursor();
  }

  /**
   * A change from the client ID a lost change went out on, after it was refused: that change
   * landing (the ID had at most that edit in flight). Open, it leaves `rejected` (`onLanded`);
   * re-applied, its text may be in twice (`onDuplicateRisk`). With an undo or redo unanswered
   * on that connection, the change may be the undo's: nothing is concluded (`presence` says
   * `unknown`).
   */
  private checkLanded(ev: ChangeEvent): void {
    const l = this.lost.get(ev.client);
    if (!l || l.rejection.asOf!.undoInFlight || ev.version <= l.rejection.asOf!.version) return;
    this.lost.delete(ev.client);
    if (typeof l.state === "object") {
      this.emit("duplicateRisk", { rejection: l.rejection, landed: ev.version, reapplied: l.state.reapplied });
      return;
    }
    this._rejected = this._rejected.filter((r) => r !== l.rejection);
    this.forget(l.rejection.id);
    this.emit("landed", { rejection: l.rejection, version: ev.version });
  }

  // ---- the roster

  /** A caret without a version is in the text at the version this client has now. */
  private stamp(e: RosterEntry): RosterEntry {
    if (e.cursor && e.cursor.version === undefined) return { ...e, cursor: { ...e.cursor, version: this._version } };
    return e;
  }

  private foldRosterEvent(ev: RosterEvent): void {
    this.emit("rosterEvent", ev);
    const list = [...this._roster];
    const at = (who: Handle) => list.findIndex((r) => r.handle === who);
    const set = (who: Handle, patch: Partial<RosterEntry>) => {
      const i = at(who);
      if (i >= 0) list[i] = this.stamp({ ...list[i], ...patch });
    };
    switch (ev.kind) {
      case "snapshot":
        list.length = 0;
        for (const m of ev.members) list.push(this.stamp({ ...m, self: m.handle === this.selfHandle }));
        this.watching = ev.watching ?? 0;
        break;
      case "joined":
        if (at(ev.who) < 0) {
          list.push({
            handle: ev.who,
            name: ev.participant.name,
            colour: ev.participant.colour,
            kind: ev.participant.kind,
            role: "viewer",
            away: false,
            cursor: null,
            self: ev.who === this.selfHandle,
          });
        }
        break;
      case "left": {
        const i = at(ev.who);
        if (i >= 0) list.splice(i, 1);
        break;
      }
      case "role":
        set(ev.who, { role: ev.role });
        break;
      case "away":
        set(ev.who, { away: ev.away });
        break;
      case "cursor":
        // the kit gives the version beside the caret; the host may put it inside
        set(ev.who, {
          cursor: ev.cursor ? { ...ev.cursor, version: ev.cursor.version ?? ev.version } : null,
        });
        break;
      case "watching":
        this.watching = ev.count;
        break;
      default:
        return; // rights, claims and notes: onRosterEvent only
    }
    this._roster = list;
    this.emit("roster", list);
  }

  // ---- the pending chain

  /**
   * Carry change `x` (on the text the chain's first entry was on) through the chain: each
   * entry is rebased past it as the server would (ours after), and `x` past each entry as the
   * mirror (incoming before). An entry that meets competing text leaves the text: in flight,
   * it is marked doomed (its refusal is coming); queued, it is rejected now. `x` ends as the
   * change from the old text to the new one, which is emitted.
   */
  private recarry(
    x: Hunk[],
    newBase: string,
    regions: Array<{ rej: Rejection; start: number; end: number }>,
    by: Handle | null,
  ): void {
    const oldText = this._text;
    let base = newBase;
    const chain: Entry[] = [];
    if (this.inflight && !this.inflight.doomed) chain.push(this.inflight);
    chain.push(...this.queue);
    const keep: Entry[] = [];
    for (const e of chain) {
      // ours through x as the server carries it (as sent: r2.3); x through ours' gap form
      const r = e.carried.through(x, 0);
      if ("ok" in r) {
        const nx = Carried.of(x).mirrored(e.carried.gaps()).hunks();
        e.carried = r.ok;
        const hs = r.ok.hunks();
        for (const g of regions) {
          this.notePlace(g.rej.id, g.start, hs);
          g.start = transform(g.start, hs, Bias.Before);
          g.end = transform(g.end, hs, Bias.After);
        }
        e.hunks = hs;
        e.before = base;
        base = apply(base, e.hunks);
        x = nx;
        keep.push(e);
        continue;
      }
      // competing text: e leaves; later entries see "undo e, then x"
      const oldAfter = apply(e.before, e.hunks);
      const rej = this.rejection(e, null);
      const reg = this.regionOf(e);
      // the region in the new base: e's range carried through x
      this.notePlace(rej.id, reg.start, x);
      regions.push({
        rej,
        start: transform(reg.start, x, Bias.Before),
        end: transform(reg.end, x, Bias.After),
      });
      x = compose(oldAfter, invert(e.before, e.hunks), x);
      if (e === this.inflight) e.doomed = rej;
    }
    this.queue = keep.filter((e) => e !== this.inflight);
    this._text = base;
    const check = apply(oldText, x);
    if (check !== base) return this.diverged("the carried change doesn't reproduce the text");
    if (x.length) {
      this.moveRejected(x);
      this.emit("change", { hunks: x, by, local: false });
    }
    for (const g of regions) {
      g.rej.region = { start: g.start, end: Math.max(g.start, g.end) };
      if (g.rej !== this.inflight?.doomed) {
        this._rejected.push(g.rej);
        this.emit("rejected", g.rej);
      }
    }
  }

  /** Send the next queued entry when nothing is in flight. */
  private flush(): void {
    while (!this.offline && !this.inflight && this.queue.length) {
      const e = this.queue.shift()!;
      if (e.hunks.length === 0) {
        this.emit("ack", { ops: e.ops, version: this._version });
        continue;
      }
      e.op = this.nextWire++;
      e.base = this._version;
      e.carried = Carried.of(e.hunks); // the server carries exactly what's sent
      e.sentClient = this.clientId;
      this.inflight = e;
      const frame: ClientFrame = {
        type: "edit",
        op: e.op,
        root: this.root,
        base: this._version,
        hunks: e.hunks,
        ...(e.group !== undefined ? { group: e.group } : {}),
      };
      const bytes = frameBytes(frame, FRAME_MAX - FRAME_MARGIN);
      if (bytes !== null) {
        // never sent: over the frame cap, refused here as the host would (MPC-E113)
        this.refuseInflight(e, {
          id: ERR.BAD_EDIT,
          message: `bad edit: the edit is ${bytes} bytes of JSON, over the ${FRAME_MAX - FRAME_MARGIN}-byte limit for one frame`,
          details: { bytes, limit: FRAME_MAX - FRAME_MARGIN, frame_max: FRAME_MAX },
        });
        continue;
      }
      this.t.send(frame);
    }
    this.sendCursor();
  }

  private sendCursor(): void {
    if (!this.cursor || this.inflight || this.queue.length) return;
    const len = utf8Len(this._text);
    const a = Math.min(this.cursor.anchor, len);
    const h = Math.min(this.cursor.head, len);
    const key = `${this._version}:${a}:${h}`;
    if (key === this.cursorSent) return;
    this.cursorSent = key;
    this.t.send({ type: "cursor", root: this.root, version: this._version, anchor: a, head: h });
  }

  private rejection(e: Entry, error: ErrorReport | null): Rejection {
    return {
      id: this.nextRejection++,
      ops: e.ops,
      hunks: e.made.hunks,
      base: e.made.base,
      region: { start: 0, end: 0 },
      text: e.hunks.map((h) => h.insert).join(""),
      error,
    };
  }

  /** The span of an entry's hunks, on `before`. */
  private regionOf(e: Entry): { start: number; end: number } {
    const first = e.hunks[0];
    const last = e.hunks[e.hunks.length - 1];
    return { start: first ? first.start : 0, end: last ? last.end : 0 };
  }

  private rejectAll(error: ErrorReport | null): void {
    const all = [...(this.inflight && !this.inflight.doomed ? [this.inflight] : []), ...this.queue];
    if (this.inflight?.doomed) {
      this.reloaded.add(this.inflight.doomed.id);
      this._rejected.push(this.inflight.doomed);
      this.emit("rejected", this.inflight.doomed);
    }
    this.inflight = null;
    this.queue = [];
    if (all.length === 0) return;
    const old = this._text;
    for (const e of all) {
      const rej = this.rejection(e, error);
      rej.region = { start: 0, end: 0 }; // no place: Re-apply refuses it (`reloaded`)
      this.reloaded.add(rej.id);
      this._rejected.push(rej);
      this.emit("rejected", rej);
    }
    this._text = this.serverText;
    const d = diffText(old, this._text);
    if (d.length) {
      this.moveRejected(d);
      this.emit("change", { hunks: d, by: null, local: false });
    }
  }

  private reloadNeeded(reason: string, error: ErrorReport | null): void {
    this.rejectAll(error);
    this.emit("reload", { reason, error });
  }

  private diverged(reason: string): void {
    this.reloadNeeded(`diverged: ${reason}`, null);
  }

  // ---- positions

  private moveRejected(hs: readonly Hunk[]): void {
    const doomed = this.inflight?.doomed;
    for (const r of doomed ? [...this._rejected, doomed] : this._rejected) {
      this.notePlace(r.id, r.region.start, hs);
      const start = transform(r.region.start, hs, Bias.Before);
      r.region = { start, end: Math.max(start, transform(r.region.end, hs, Bias.After)) };
    }
  }

  /**
   * Where an event for `root` at `version` stands: `seen` (that root's event at that version is
   * applied, or the version is behind), `same` (the current version, another root's event came
   * first), `next`, or `gap` (a version is missing: ask `since`).
   */
  private placeOf(version: number, root: string): "seen" | "same" | "next" | "gap" {
    if (version < this._version) return "seen";
    if (version === this._version) return this.seenAt === "all" || this.seenAt.has(root) ? "seen" : "same";
    return version === this._version + 1 ? "next" : "gap";
  }

  /** `root`'s change at `version` (`placeOf`: `next` or `same`) is applied: the version, the
   * roots seen at it, and the history (one entry per version: this root's hunks, `[]` until
   * they come). */
  private advance(version: number, root: string, hunks: Hunk[], client: number | undefined): void {
    if (version !== this._version) {
      this._version = version;
      this.seenAt = new Set([root]);
      this.pushHistory(version, hunks, client);
      return;
    }
    if (this.seenAt !== "all") this.seenAt.add(root);
    const last = this.history[this.history.length - 1];
    if (last && last.version === version) {
      if (root === this.root) last.hunks = hunks;
    } else this.pushHistory(version, hunks, client);
  }

  /**
   * Change `hs` moves a rejection's point `at`: something inserted exactly there (a tie: the
   * server would put the rejected change after it, Re-apply at the region's start before it), or
   * text deleted or replaced around it or touching it (the region snaps to the deletion's start).
   * Touching counts: a change reaches the replica mirrored around the person's own pending text,
   * so a deletion the server sees around the point can end (or start) exactly at it here.
   */
  private notePlace(id: number, at: number, hs: readonly Hunk[]): void {
    for (const h of hs) {
      if (h.start > at) break;
      if (h.start === at && h.insert.length > 0) this.tied.add(id);
      else if (h.start < h.end && at <= h.end) this.snapped.add(id);
    }
  }

  private forget(id: number): void {
    this.reloaded.delete(id);
    this.tied.delete(id);
    this.snapped.delete(id);
  }

  private pushHistory(version: number, hunks: Hunk[], client: number | undefined): void {
    this.history.push({ version, hunks, client });
    if (this.history.length > HISTORY) {
      this.history.splice(0, this.history.length - HISTORY);
      this.historyFrom = this.history[0].version - 1;
    }
  }

  /** A position in the root's text at `version`, in the client's current text (or `null`). */
  private mapFromVersion(pos: number, version: number): number | null {
    return this.mapFrom(pos, version, Bias.After);
  }

  /** As `mapFromVersion`, with `bias` through the server's changes since `version`. */
  private mapFrom(pos: number, version: number, bias: Bias): number | null {
    if (version > this._version) return null;
    if (version < this._version) {
      const i = this.history.findIndex((h) => h.version === version + 1);
      if (i < 0) return null;
      for (const h of this.history.slice(i)) pos = transform(pos, h.hunks, bias);
    }
    if (this.inflight && !this.inflight.doomed) pos = transform(pos, this.inflight.hunks, Bias.Before);
    for (const e of this.queue) pos = transform(pos, e.hunks, Bias.Before);
    return pos;
  }

  private on<K extends keyof Events>(k: K, cb: Listener<Events[K]>): () => void {
    this.ls[k].add(cb);
    return () => {
      this.ls[k].delete(cb);
    };
  }

  private emit<K extends keyof Events>(k: K, v: Events[K]): void {
    for (const cb of [...this.ls[k]]) cb(v);
  }
}

/** One hunk turning `a` into `b` (common prefix and suffix kept), in UTF-8 bytes of `a`. */
export function diffText(a: string, b: string): Hunk[] {
  if (a === b) return [];
  let p = 0;
  const n = Math.min(a.length, b.length);
  while (p < n && a.charCodeAt(p) === b.charCodeAt(p)) p++;
  if (p > 0 && isHigh(a.charCodeAt(p - 1))) p--;
  let s = 0;
  while (s < n - p && a.charCodeAt(a.length - 1 - s) === b.charCodeAt(b.length - 1 - s)) s++;
  if (s > 0 && isLow(a.charCodeAt(a.length - s))) s--;
  const start = utf8Len(a.slice(0, p));
  return [
    {
      start,
      end: start + utf8Len(a.slice(p, a.length - s)),
      insert: b.slice(p, b.length - s),
    },
  ];
}

/** A frame's JSON size in UTF-8 bytes when it is over `limit`, else `null` (cheap for small
 * frames: a JSON string is at most 6 bytes per UTF-16 unit). */
export function frameBytes(frame: ClientFrame | ServerFrame, limit: number): number | null {
  if (frame.type === "edit") {
    let units = 0;
    for (const h of frame.hunks) units += h.insert.length;
    if (units * 6 + frame.hunks.length * 64 + 512 <= limit) return null;
  }
  const n = utf8Len(JSON.stringify(frame));
  return n > limit ? n : null;
}

const isHigh = (c: number) => c >= 0xd800 && c < 0xdc00;
const isLow = (c: number) => c >= 0xdc00 && c < 0xe000;

const sameHunk = (a: Hunk, b: Hunk) => a.start === b.start && a.end === b.end && a.insert === b.insert;
