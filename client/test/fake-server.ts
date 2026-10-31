// A fake host for tests: one doc with one text root, applying the E-LIVE rules as the kit's
// `Session::edit_at_base` does (rebase through every change since the base, server first at a
// tie, refusal on competing text, nothing applied when refused), and an undo per participant.
// Frames travel over links with per-frame latency, in order per direction (as a WebSocket).
//
// It follows WIRE.md's host rules exactly:
// - each connection has its own queue of change events (the kit's subscriber channel), drained
//   by its writer; on an `edit` that applies at version V the writer forwards every queued
//   event before V, skips the one at V (the edit itself, answered by `ack`), then sends the
//   `ack`; nothing applied: every queued event, then the `ack` at the version unchanged;
// - undo and redo come back as `change` to everyone, the undoer included, then `undone`;
// - a reconnect (`FakeLive.connect`) is a new connection with a new client ID, same pid;
// - no frame over `frameMax` is sent or accepted (WIRE.md, "Frame cap").

import { apply, coalesce, invert, utf8Len, validate, type Hunk } from "../src/hunk";
import { rebaseOurs } from "../src/rebase";
import {
  FRAME_MAX,
  canRetry,
  type ChangeEvent,
  type ClientFrame,
  type CloseEvent,
  type CloseReason,
  type ErrorReport,
  type LiveTransport,
  type RosterEntry,
  type ServerFrame,
  type Transport,
  type Welcome,
} from "../src/wire";

/** A deterministic PRNG (mulberry32). */
export function rng(seed: number) {
  let a = seed >>> 0;
  const next = () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  return {
    next,
    int: (n: number) => Math.floor(next() * n),
    pick: <T>(xs: readonly T[]) => xs[Math.floor(next() * xs.length)],
  };
}

/** A clock and a queue of timed deliveries. */
export class Net {
  now = 0;
  private q: Array<{ at: number; seq: number; f: () => void }> = [];
  private seq = 0;
  at(delay: number, f: () => void) {
    this.q.push({ at: this.now + delay, seq: this.seq++, f });
  }
  /** Run the next delivery; false when none is left. */
  step(): boolean {
    if (this.q.length === 0) return false;
    let i = 0;
    for (let j = 1; j < this.q.length; j++) {
      const a = this.q[j];
      const b = this.q[i];
      if (a.at < b.at || (a.at === b.at && a.seq < b.seq)) i = j;
    }
    const [d] = this.q.splice(i, 1);
    this.now = Math.max(this.now, d.at);
    d.f();
    return true;
  }
  get idle() {
    return this.q.length === 0;
  }
}

/** Faults on the host-to-client direction: frames dropped, or overtaking each other. */
export interface Faults {
  drop: number;
  reorder: boolean;
  next: () => number;
}

/** One direction of a link: frames in order, each after its latency (never overtaking unless
 * `faults.reorder`). `cut()` loses everything in flight and everything after (a dropped socket). */
class Pipe<T> {
  private last = 0;
  private cutAt = false;
  dropped = 0;
  /** Frames that reached the end after `cut` (dropped unapplied). */
  discarded = 0;
  /** Frames still on their way arrive this much later (a half-open socket delivering late). */
  lateBy = 0;
  constructor(
    private net: Net,
    private latency: () => number,
    private deliver: (f: T, bytes: number) => void,
    private faults?: Faults,
  ) {}
  send(f: T) {
    if (this.cutAt) return;
    const json = JSON.stringify(f); // the wire is JSON
    this.sendJson(json);
  }
  sendJson(json: string) {
    if (this.cutAt) return;
    if (this.faults && this.faults.next() < this.faults.drop) {
      this.dropped++;
      return;
    }
    const at = this.faults?.reorder
      ? this.net.now + this.latency()
      : Math.max(this.net.now + this.latency(), this.last);
    this.last = Math.max(this.last, at);
    this.net.at(at - this.net.now, () => this.arrive(json, false));
  }
  private arrive(json: string, delayed: boolean) {
    if (this.cutAt) {
      this.discarded++;
      return;
    }
    if (this.lateBy > 0 && !delayed) {
      this.net.at(this.lateBy, () => this.arrive(json, true));
      return;
    }
    this.deliver(JSON.parse(json) as T, utf8Len(json));
  }
  cut() {
    this.cutAt = true;
  }
}

interface Applied {
  version: number;
  conn: number;
  pid: string;
  client: number;
  hunks: Hunk[];
  /** For undo: the text before. */
  before: string;
}

interface Conn {
  out: Pipe<ServerFrame>;
  inbound: Pipe<ClientFrame>;
  handle: string;
  client: number;
  /** The kit's subscriber channel: events not yet forwarded by this connection's writer. */
  queue: ChangeEvent[];
  /** The writer lags: events wait in `queue` until this connection's next frame (or `drain`). */
  lag: boolean;
}

export class FakeServer {
  text: string;
  version: number;
  readonly log: Applied[] = [];
  private conns = new Map<number, Conn>();
  refused = 0;
  /** Edits whose base had left the window (`MPC-E112`). */
  window = Infinity;
  /** `since` requests past the window (`MPC-E112`): opt-in, so the seeded runs keep their
   * evidence counts. */
  sinceWindow = Infinity;
  /** The frame cap, both directions (UTF-8 bytes of JSON). */
  frameMax = FRAME_MAX;
  /** Every caret, visibility and view frame received, in order. */
  readonly presence: Array<{ conn: number; frame: ClientFrame }> = [];
  /** Every frame the server chose not to send (over the cap). */
  readonly withheld: ServerFrame["type"][] = [];
  private nextConn = 1000;
  /**
   * WIRE.md's reconnect rule: a connect with `prev` (the same login's previous connection on
   * this doc) leaves that connection first, before the welcome, and anything still arriving on
   * it is dropped unapplied. `false`: a host that doesn't follow the rule (it ignores `prev`).
   */
  leaveOldOnReconnect = true;
  /** Without the rule, when the host leaves a dropped connection whose socket is half-open:
   * when its close is seen (`close-seen`, as the client sees it), or at the keepalive timeout. */
  noRuleLeave: "close-seen" | "keepalive" = "close-seen";
  /** A half-open socket's end, in the fake clock's units (the 20 s keepalive). */
  keepaliveMs = 20_000;
  /** Inbound pipes of connections left because of `prev` (their late frames are counted). */
  private replaced: Array<{ discarded: number }> = [];
  /** Refuse `snapshot` requests (the client must fall back to a fresh connect). */
  refuseSnapshots = false;
  /** Every edit the server applied, as `participant:op` (a false MPC-E000 check reads it). */
  readonly appliedOps = new Set<string>();
  /** The version each applied edit landed at, by `participant:op`. */
  readonly appliedAt = new Map<string, number>();
  /** Each connection's participant, kept after the connection goes. */
  private handles = new Map<number, string>();

  constructor(
    private net: Net,
    text = "",
    version = 0,
  ) {
    this.text = text;
    this.version = version;
  }

  /** A transport for a new connection, with link latencies drawn by `latency`. The client ID
   * is `conn` unless given; the participant is `p<conn>` unless `handle` is given. */
  connect(
    conn: number,
    latency: () => number,
    faults?: Faults,
    opts: { handle?: string; client?: number } = {},
  ): Transport {
    const listeners = new Set<(f: ServerFrame) => void>();
    this.open(conn, latency, (f) => listeners.forEach((l) => l(f)), faults, opts);
    const c = this.conns.get(conn)!;
    return {
      send: (f) => c.inbound.send(f),
      onFrame: (cb) => {
        listeners.add(cb);
        return () => listeners.delete(cb);
      },
    };
  }

  private open(
    conn: number,
    latency: () => number,
    deliver: (f: ServerFrame) => void,
    faults: Faults | undefined,
    opts: { handle?: string; client?: number },
  ): Conn {
    const out = new Pipe<ServerFrame>(this.net, latency, (f) => deliver(f), faults);
    const inbound = new Pipe<ClientFrame>(this.net, latency, (f, bytes) => this.receive(conn, f, bytes));
    const c: Conn = {
      out,
      inbound,
      handle: opts.handle ?? `p${conn}`,
      client: opts.client ?? conn,
      queue: [],
      lag: false,
    };
    this.conns.set(conn, c);
    this.handles.set(conn, c.handle);
    return c;
  }

  /**
   * A live connection for participant `handle` (`FakeLive`): its `connect` joins as a new
   * connection, with a new client ID, every time.
   */
  live(handle: string, latency: () => number): FakeLive {
    return new FakeLive(this, handle, latency);
  }

  /** `FakeLive.connect`'s host side: join, subscribe, read, welcome (all at once here, so
   * nothing can fall between the read and the stream). Throws a `CloseEvent` when refused. */
  join(
    handle: string,
    latency: () => number,
    deliver: (f: ServerFrame) => void,
    resume?: number,
    prev?: number,
  ): { conn: number; welcome: Welcome } {
    this.replacePrev(handle, prev);
    const conn = this.nextConn++;
    const c = this.open(conn, latency, deliver, undefined, { handle, client: 7_000_000_000_000 + conn });
    const resumable = resume !== undefined && resume <= this.version && this.version - resume <= this.sinceWindow;
    const welcome: Welcome = {
      root: "source",
      version: this.version,
      ...(resumable ? {} : { text: this.text }),
      client: c.client,
      self: handle,
      rights: "edit",
    };
    const bytes = utf8Len(JSON.stringify({ type: "welcome", ...welcome }));
    if (bytes > this.frameMax) {
      this.leave(conn);
      const e: CloseEvent = { reason: "declined", retry: false, error: this.tooLarge(bytes) };
      throw e;
    }
    return { conn, welcome };
  }

  /** The connection is gone (the socket dropped, or `close`): frames in flight are lost. */
  leave(conn: number) {
    const c = this.conns.get(conn);
    if (!c) return;
    c.out.cut();
    c.inbound.cut();
    this.conns.delete(conn);
  }

  /** The host closes the socket after its last frame: frames already sent still arrive,
   * nothing more is read or sent. */
  private closeAfterSent(conn: number) {
    const c = this.conns.get(conn);
    if (!c) return;
    c.inbound.cut();
    this.conns.delete(conn);
  }

  /** Make a connection's writer lag (its events wait until its next frame), or catch up. */
  setLag(conn: number, lag: boolean) {
    const c = this.conns.get(conn)!;
    c.lag = lag;
    if (!lag) this.drain(conn);
  }

  /**
   * The socket of `conn` is gone. Frames to the client in flight are lost; frames to the
   * server in flight arrive (`inboundArrives`: an edit applied whose ack is lost) or are lost
   * too. Nothing new is read: the client's transport sends nothing more on it.
   */
  drop(conn: number, inboundArrives = false, leaveAfter = 0, late = 0) {
    const c = this.conns.get(conn);
    if (!c) return;
    c.out.cut();
    c.inbound.lateBy = late;
    if (!inboundArrives) return this.leave(conn);
    // half-open: the host reads what reaches it until it leaves the connection (replies go
    // nowhere): at the next connect's `prev` (the rule), else when it sees the close
    const at = this.leaveOldOnReconnect || this.noRuleLeave === "keepalive" ? this.keepaliveMs : leaveAfter;
    this.net.at(at, () => this.leave(conn));
  }

  /**
   * The reconnect rule, at a new connection's connect (before its welcome): leave the
   * connection whose client ID is `prev` when it is `handle`'s on this doc; anything else, and
   * a host without the rule, ignores `prev` silently.
   */
  replacePrev(handle: string, prev: number | undefined): void {
    if (!this.leaveOldOnReconnect || prev === undefined) return;
    for (const [conn, c] of this.conns) {
      if (c.client !== prev || c.handle !== handle) continue;
      this.replaced.push(c.inbound);
      this.leave(conn);
      return;
    }
  }

  /** Frames that arrived on a connection after `prev` replaced it: dropped, never applied. */
  get lateDiscarded(): number {
    return this.replaced.reduce((n, p) => n + p.discarded, 0);
  }

  /** Whether `conn` is open on the host. */
  has(conn: number): boolean {
    return this.conns.has(conn);
  }

  /** The host closes `conn` with `reason`: `closed` is its last frame, then the socket goes. */
  closeWith(conn: number, reason: CloseReason, error?: ErrorReport, extra: { code?: string; path?: string } = {}) {
    if (!this.conns.has(conn)) return;
    this.send(conn, { type: "closed", reason, ...extra, ...(error ? { error } : {}) });
    this.closeAfterSent(conn);
  }

  /** Send one frame to `conn` (roster events, notes), under the cap as any frame. */
  sendTo(conn: number, f: ServerFrame) {
    this.send(conn, f);
  }

  /** The participant of `conn`, also after it's gone. */
  handleOf(conn: number): string | undefined {
    return this.handles.get(conn);
  }

  /** The client ID a connection was given. */
  clientOf(conn: number): number {
    return this.conns.get(conn)!.client;
  }

  snapshot() {
    return { text: this.text, version: this.version };
  }

  roster(entries: RosterEntry[]) {
    for (const [conn] of this.conns) this.send(conn, { type: "roster", entries });
  }

  /** Send a frame, never over the cap: what doesn't fit is replaced as WIRE.md says. */
  private send(conn: number, f: ServerFrame): void {
    const c = this.conns.get(conn);
    if (!c) return;
    const json = JSON.stringify(f);
    const bytes = utf8Len(json);
    if (bytes <= this.frameMax) return c.out.sendJson(json);
    this.withheld.push(f.type);
    switch (f.type) {
      case "change":
        return; // not sent: the client sees the version gap and asks `since`
      case "events":
        return this.send(conn, {
          type: "refused",
          op: f.op,
          error: {
            id: "MPC-E112",
            message: "the changes since don't fit one frame; reload",
            details: { limit: "frame", bytes, frame_max: this.frameMax },
          },
        });
      case "snapshot":
        // `closed` is the last frame: it still goes out, then the socket closes
        this.send(conn, { type: "closed", reason: "declined", error: this.tooLarge(bytes) });
        return this.closeAfterSent(conn);
      default:
        throw new Error(`the fake host has no rule for a ${f.type} frame over the cap`);
    }
  }

  private tooLarge(bytes: number): ErrorReport {
    return {
      id: "MPC-E401",
      exit: 4,
      message: "live editing is off: the text doesn't fit one frame (too_large)",
      details: { limit: "frame", bytes, frame_max: this.frameMax },
    };
  }

  private event(a: Applied): ChangeEvent {
    return {
      version: a.version,
      pid: a.pid,
      client: a.client,
      via: "test",
      root: "source",
      hunks: a.hunks,
      at_ms: this.net.now,
      on_behalf_of: null,
    };
  }

  /** The writer: forward what's queued, skipping the event at `skip` (the connection's own
   * edit, answered by its `ack`). */
  private drain(conn: number, skip?: number) {
    const c = this.conns.get(conn);
    if (!c) return;
    const q = c.queue.splice(0);
    for (const ev of q) if (ev.version !== skip) this.send(conn, { type: "change", event: ev });
  }

  /** Apply a change and queue its event to every connection; writers that don't lag drain at
   * once, except `editor`'s (its edit's reply drains it). */
  private commit(conn: number, hunks: Hunk[], editor: number | null): number {
    const before = this.text;
    this.text = apply(before, hunks);
    this.version++;
    const by = this.conns.get(conn);
    const a: Applied = {
      version: this.version,
      conn,
      pid: by?.handle ?? "kit:server",
      client: by?.client ?? 0,
      hunks,
      before,
    };
    this.log.push(a);
    const ev = this.event(a);
    for (const [k, c] of this.conns) {
      c.queue.push(ev);
      if (k !== editor && !c.lag) this.drain(k);
    }
    return this.version;
  }

  /** A change made on the server itself (an agent, an outside edit): broadcast to all. */
  serverEdit(hunks: Hunk[]) {
    const bad = validate(this.text, hunks);
    if (bad) throw new Error(`bad server edit ${JSON.stringify(bad)}`);
    if (hunks.length) this.commit(0, coalesce(hunks), null);
  }

  private receive(conn: number, f: ClientFrame, bytes: number) {
    const c = this.conns.get(conn);
    if (!c) return;
    if (bytes > this.frameMax) {
      const error: ErrorReport = {
        id: "MPC-E113",
        exit: 2,
        message: `bad edit: the frame is ${bytes} bytes, over the ${this.frameMax}-byte cap`,
        details: { bytes, frame_max: this.frameMax },
      };
      if (f.type === "edit") return this.send(conn, { type: "refused", op: f.op, error });
      return this.send(conn, { type: "error", error });
    }
    switch (f.type) {
      case "edit": {
        const since = this.log.filter((a) => a.version > f.base);
        if (f.base > this.version || this.version - f.base > this.window) {
          this.drain(conn);
          return this.send(conn, {
            type: "refused",
            op: f.op,
            error: { id: "MPC-E112", message: "base expired" },
          });
        }
        // the base's text, to validate the hunks against (rule 7)
        let baseText = this.text;
        for (const a of [...since].reverse()) baseText = a.before;
        const bad = validate(baseText, f.hunks);
        if (bad) {
          this.drain(conn);
          return this.send(conn, {
            type: "refused",
            op: f.op,
            error: { id: "MPC-E151", message: `bad hunk ${bad.kind}` },
          });
        }
        const r = rebaseOurs(f.hunks, since.map((a) => a.hunks));
        if ("conflict" in r) {
          this.refused++;
          this.drain(conn);
          return this.send(conn, {
            type: "refused",
            op: f.op,
            error: {
              id: "MPC-E111",
              message: "write conflict",
              details: { base: f.base, conflicts: [{ ...r.conflict, kind: "overlap" }] },
            },
          });
        }
        if (validate(this.text, r.ok)) throw new Error("a rebase that doesn't fit the text");
        if (r.ok.length === 0) {
          // nothing applied: forward everything queued, skip nothing, ack the version unchanged
          this.drain(conn);
          return this.send(conn, { type: "ack", op: f.op, version: this.version, hunks: [] });
        }
        const v = this.commit(conn, r.ok, conn);
        this.appliedOps.add(`${c.handle}:${f.op}`);
        this.appliedAt.set(`${c.handle}:${f.op}`, v);
        // the writer: every event before V, never the one at V, then the ack
        this.drain(conn, v);
        return this.send(conn, { type: "ack", op: f.op, version: v, hunks: r.ok });
      }
      case "since": {
        if (f.version > this.version || this.version - f.version > this.sinceWindow) {
          return this.send(conn, {
            type: "refused",
            op: f.op,
            error: { id: "MPC-E112", message: "base expired" },
          });
        }
        c.queue.length = 0; // the reply covers them
        const events = this.log.filter((a) => a.version > f.version).map((a) => this.event(a));
        return this.send(conn, { type: "events", op: f.op, events });
      }
      case "snapshot":
        if (this.refuseSnapshots) {
          return this.send(conn, { type: "refused", op: f.op, error: { id: "MPC-E000", message: "no snapshots here" } });
        }
        // queued events at or before its version may follow it: the client ignores them
        return this.send(conn, { type: "snapshot", op: f.op, text: this.text, version: this.version });
      case "undo": {
        // the participant's last change (any of their connections: undo is per participant), if
        // the text it inserted is still where it put it
        const last = [...this.log].reverse().find((a) => a.pid === c.handle);
        if (!last) {
          this.drain(conn);
          return this.send(conn, { type: "undone", op: f.op, steps: 0, version: this.version });
        }
        const later = this.log.filter((a) => a.version > last.version);
        const inv = invert(last.before, last.hunks);
        const r = rebaseOurs(inv, later.map((a) => a.hunks));
        if ("conflict" in r || r.ok.length === 0) {
          this.drain(conn);
          return this.send(conn, { type: "refused", op: f.op, error: { id: "MPC-E201", message: "can't undo" } });
        }
        // the undo comes back to the undoer as a `change` too: never filtered by client ID
        const v = this.commit(conn, r.ok, null);
        this.drain(conn);
        return this.send(conn, { type: "undone", op: f.op, steps: 1, version: v });
      }
      case "redo":
        this.drain(conn);
        return this.send(conn, { type: "undone", op: f.op, steps: 0, version: this.version });
      case "cursor":
      case "visible":
      case "view":
        this.presence.push({ conn, frame: f });
        return;
      case "hello":
        return;
    }
  }

  /** A client frame on `conn`'s socket (what `FakeLive.send` writes). */
  sendFrom(conn: number, f: ClientFrame) {
    this.conns.get(conn)?.inbound.send(f);
  }

  /** Total UTF-8 length (for random edits). */
  get bytes() {
    return utf8Len(this.text);
  }
}

/** The fake host's `LiveTransport`: one stable object; each `connect` is a new connection. */
export class FakeLive implements LiveTransport {
  private frames = new Set<(f: ServerFrame) => void>();
  private closes = new Set<(e: CloseEvent) => void>();
  conn: number | null = null;
  /** Frames `send` dropped while no connection was open (never replayed). */
  dropped: ClientFrame[] = [];
  /** Every frame sent while open, per connection. */
  sent: Array<{ conn: number; frame: ClientFrame }> = [];

  constructor(
    private server: FakeServer,
    readonly handle: string,
    private latency: () => number,
  ) {}

  async connect(opts: { resume?: number; prev?: number; signal?: AbortSignal } = {}): Promise<Welcome> {
    // `connect` while open replaces the connection (WIRE.md, "The live transport")
    if (this.conn !== null) {
      this.server.leave(this.conn);
      this.conn = null;
    }
    try {
      const { conn, welcome } = this.server.join(this.handle, this.latency, (f) => this.deliver(f), opts.resume, opts.prev);
      this.conn = conn;
      return welcome;
    } catch (e) {
      const ce = e as CloseEvent;
      for (const l of [...this.closes]) l(ce);
      throw ce;
    }
  }

  private deliver(f: ServerFrame) {
    if (f.type === "closed") {
      const e: CloseEvent = { reason: f.reason, retry: canRetry(f.reason), ...(f.error ? { error: f.error } : {}) };
      this.conn = null;
      for (const l of [...this.closes]) l(e);
      return;
    }
    for (const l of [...this.frames]) l(f);
  }

  send(frame: ClientFrame): void {
    if (this.conn === null) {
      this.dropped.push(frame); // dropped, never buffered for the next socket
      return;
    }
    this.sent.push({ conn: this.conn, frame });
    this.server.sendFrom(this.conn, frame);
  }

  onFrame(cb: (f: ServerFrame) => void): () => void {
    this.frames.add(cb);
    return () => this.frames.delete(cb);
  }

  onClose(cb: (e: CloseEvent) => void): () => void {
    this.closes.add(cb);
    return () => this.closes.delete(cb);
  }

  /** The network dropped: frames in flight both ways are lost. */
  drop(): void {
    if (this.conn === null) return;
    this.server.leave(this.conn);
    this.conn = null;
    for (const l of [...this.closes]) l({ reason: "network", retry: true });
  }

  close(): void {
    if (this.conn !== null) this.server.leave(this.conn);
    this.conn = null;
  }
}
