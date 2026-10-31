// A fake host for the live editor: `LiveTransport`s over the fake server. Every `connect` is a
// new connection with a new client ID (as a host's join); the test drops sockets, closes
// connections with a reason, makes connects fail, and lets a resume past the server's window
// come back with the text (no resume).

import { canRetry, type ClientFrame, type ConnectOptions, type CloseEvent, type CloseReason, type LiveTransport, type Rights, type ServerFrame, type Transport, type Welcome } from "../src/wire";
import type { Faults, FakeServer, Net } from "./fake-server";

export class FakeLiveHost {
  private nextConn: number;
  readonly transports: FakeLiveTransport[] = [];
  constructor(
    readonly net: Net,
    readonly server: FakeServer,
    readonly latency: () => number,
    /** The first connection's ID (client IDs are the connection IDs here). */
    firstConn = 1,
  ) {
    this.nextConn = firstConn;
  }

  /** A participant's transport (one per editor, for its whole life). */
  transport(handle: string, opts: { rights?: Rights; faults?: () => Faults | undefined } = {}): FakeLiveTransport {
    const t = new FakeLiveTransport(this, handle, opts.rights ?? "edit", opts.faults);
    this.transports.push(t);
    return t;
  }

  newConn(): number {
    return this.nextConn++;
  }
}

export class FakeLiveTransport implements LiveTransport {
  conn: number | null = null;
  private inner: Transport | null = null;
  private innerOff: (() => void) | null = null;
  /** The socket died: sends are lost until the editor connects again. */
  private dead = false;
  private readonly frameLs = new Set<(f: ServerFrame) => void>();
  private readonly closeLs = new Set<(e: CloseEvent) => void>();
  /** Connects to fail before one succeeds (each a retryable network failure). */
  failConnects = 0;
  /** Refuse every connect with this (e.g. `{reason: "auth", retry: false}`). */
  refuseWith: CloseEvent | null = null;
  connects = 0;
  /** The `resume` of each connect. */
  readonly resumes: Array<number | undefined> = [];
  closedByEditor = false;
  /** Resume whatever the server's window (its `since` may then be refused: MPC-E112). */
  resumeAlways = false;
  /** Every frame the editor handed over (sent or dropped). */
  readonly sent: ClientFrame[] = [];

  constructor(
    private readonly host: FakeLiveHost,
    readonly handle: string,
    public rights: Rights,
    private readonly faults?: () => Faults | undefined,
  ) {}

  /** The `prev` of each connect. */
  readonly prevs: Array<number | undefined> = [];

  connect(opts: ConnectOptions): Promise<Welcome> {
    const { net, server } = this.host;
    this.connects++;
    this.resumes.push(opts.resume);
    this.prevs.push(opts.prev);
    return new Promise((resolve, reject) => {
      net.at(this.host.latency(), () => {
        if (opts.signal.aborted) return reject(new Error("aborted"));
        if (this.refuseWith) return reject(this.refuseWith);
        if (this.failConnects > 0) {
          this.failConnects--;
          return reject({ reason: "network", retry: true } satisfies CloseEvent);
        }
        this.detach();
        // the reconnect rule (WIRE.md): the host leaves the previous connection first
        server.replacePrev(this.handle, opts.prev);
        const conn = this.host.newConn();
        const inner = server.connect(conn, this.host.latency, this.faults?.(), { handle: this.handle });
        this.inner = inner;
        this.conn = conn;
        this.dead = false;
        this.innerOff = inner.onFrame((f) => this.fromHost(conn, f));
        const canResume =
          opts.resume !== undefined &&
          opts.resume <= server.version &&
          (this.resumeAlways || server.version - opts.resume <= server.sinceWindow);
        resolve({
          root: "source",
          version: canResume ? opts.resume! : server.version,
          ...(canResume ? {} : { text: server.text }),
          client: conn,
          self: this.handle,
          rights: this.rights,
        });
      });
    });
  }

  send(frame: ClientFrame): void {
    this.sent.push(frame);
    if (!this.inner || this.dead) return; // dropped, never kept
    this.inner.send(frame);
  }

  onFrame(cb: (f: ServerFrame) => void): () => void {
    this.frameLs.add(cb);
    return () => this.frameLs.delete(cb);
  }

  onClose(cb: (e: CloseEvent) => void): () => void {
    this.closeLs.add(cb);
    return () => this.closeLs.delete(cb);
  }

  close(): void {
    this.closedByEditor = true;
    if (this.conn !== null) this.host.server.drop(this.conn);
    this.detach();
  }

  // ---- what tests do

  /** The socket dies: frames to the client in flight are lost; frames to the server in flight
   * arrive or not (`inboundArrives`); the editor hears of it after `notifyAfter`. */
  drop(inboundArrives = false, notifyAfter = this.host.latency(), late = 0): void {
    if (this.conn === null) return;
    this.host.server.drop(this.conn, inboundArrives, notifyAfter, late);
    this.dead = true;
    const conn = this.conn;
    this.host.net.at(notifyAfter, () => {
      if (this.conn !== conn) return;
      this.detach();
      this.fireClose({ reason: "network", retry: true });
    });
  }

  /** The socket ends with no `closed` frame and the transport reports `e` itself (e.g. its own
   * pre-check after a failed dial saying `auth`), `retry` as the transport claims it. */
  endAs(e: CloseEvent): void {
    if (this.conn === null) return;
    this.host.server.drop(this.conn);
    this.dead = true;
    const conn = this.conn;
    this.host.net.at(this.host.latency(), () => {
      if (this.conn !== conn) return;
      this.detach();
      this.fireClose(e);
    });
  }

  /** The host closes the connection with `reason` (a `closed` frame, then the close). */
  closeWith(reason: CloseReason, extra: { code?: string; path?: string } = {}): void {
    if (this.conn === null) return;
    this.host.server.closeWith(this.conn, reason, undefined, extra);
  }

  private fromHost(conn: number, f: ServerFrame): void {
    if (this.conn !== conn) return;
    for (const l of [...this.frameLs]) l(f);
    if (f.type === "closed") {
      this.detach();
      this.fireClose({
        reason: f.reason,
        retry: canRetry(f.reason),
        ...(f.code !== undefined ? { code: f.code } : {}),
        ...(f.path !== undefined ? { path: f.path } : {}),
        ...(f.error ? { error: f.error } : {}),
      });
    }
  }

  private fireClose(e: CloseEvent): void {
    for (const l of [...this.closeLs]) l(e);
  }

  private detach(): void {
    this.innerOff?.();
    this.innerOff = null;
    this.inner = null;
    this.conn = null;
  }
}

/** Timers on the fake clock, for `LiveEditorOptions.timers`. */
export function netTimers(net: Net, random: () => number) {
  return {
    set: (f: () => void, ms: number) => {
      const h = { cancelled: false };
      net.at(ms, () => {
        if (!h.cancelled) f();
      });
      return h;
    },
    clear: (h: unknown) => {
      if (h) (h as { cancelled: boolean }).cancelled = true;
    },
    random,
    now: () => net.now,
  };
}

/** Let promise callbacks and queued microtasks run (the editor's welcome and flushes). */
export async function microtasks(n = 6): Promise<void> {
  for (let i = 0; i < n; i++) await Promise.resolve();
}

/** Run the fake network until nothing is left, letting microtasks run after each delivery. */
export async function settle(net: Net, check?: () => void, max = 1_000_000): Promise<void> {
  await microtasks();
  for (let i = 0; i < max; i++) {
    if (!net.step()) {
      await microtasks();
      if (net.idle) return;
      continue;
    }
    await microtasks();
    check?.();
  }
  throw new Error("the fake network never settled");
}

export type { ServerFrame };
