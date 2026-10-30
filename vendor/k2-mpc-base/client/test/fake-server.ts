// A fake host for tests: one doc with one text root, applying the E-LIVE rules as the kit's
// `Session::edit_at_base` does (rebase through every change since the base, server first at a
// tie, refusal on competing text, nothing applied when refused), and an undo per connection.
// Frames travel over links with per-frame latency, in order per direction (as a WebSocket).

import { apply, coalesce, invert, utf8Len, validate, type Hunk } from "../src/hunk";
import { rebaseOurs } from "../src/rebase";
import type { ChangeEvent, ClientFrame, RosterEntry, ServerFrame, Transport } from "../src/wire";

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

/** One direction of a link: frames in order, each after its latency (never overtaking). */
/** Faults on the host-to-client direction: frames dropped, or overtaking each other. */
export interface Faults {
  drop: number;
  reorder: boolean;
  next: () => number;
}

class Pipe<T> {
  private last = 0;
  dropped = 0;
  constructor(
    private net: Net,
    private latency: () => number,
    private deliver: (f: T) => void,
    private faults?: Faults,
  ) {}
  send(f: T) {
    const copy = JSON.parse(JSON.stringify(f)) as T; // the wire is JSON
    if (this.faults && this.faults.next() < this.faults.drop) {
      this.dropped++;
      return;
    }
    const at = this.faults?.reorder
      ? this.net.now + this.latency()
      : Math.max(this.net.now + this.latency(), this.last);
    this.last = Math.max(this.last, at);
    this.net.at(at - this.net.now, () => this.deliver(copy));
  }
}

interface Applied {
  version: number;
  conn: number;
  hunks: Hunk[];
  /** For undo: the text before. */
  before: string;
}

export class FakeServer {
  text: string;
  version: number;
  readonly log: Applied[] = [];
  private conns = new Map<number, { out: Pipe<ServerFrame>; handle: string }>();
  refused = 0;
  /** Edits whose base had left the window (`MPC-E112`). */
  window = Infinity;

  constructor(
    private net: Net,
    text = "",
    version = 0,
  ) {
    this.text = text;
    this.version = version;
  }

  /** A transport for a new connection, with link latencies drawn by `latency`. */
  connect(conn: number, latency: () => number, faults?: Faults): Transport {
    const listeners = new Set<(f: ServerFrame) => void>();
    const out = new Pipe<ServerFrame>(
      this.net,
      latency,
      (f) => {
        for (const l of [...listeners]) l(f);
      },
      faults,
    );
    const inbound = new Pipe<ClientFrame>(this.net, latency, (f) => this.receive(conn, f));
    this.conns.set(conn, { out, handle: `p${conn}` });
    return {
      send: (f) => inbound.send(f),
      onFrame: (cb) => {
        listeners.add(cb);
        return () => listeners.delete(cb);
      },
    };
  }

  snapshot() {
    return { text: this.text, version: this.version };
  }

  roster(entries: RosterEntry[]) {
    for (const c of this.conns.values()) c.out.send({ type: "roster", entries });
  }

  private send(conn: number, f: ServerFrame) {
    this.conns.get(conn)!.out.send(f);
  }

  private event(conn: number, version: number, hunks: Hunk[]): ChangeEvent {
    return {
      version,
      pid: this.conns.get(conn)?.handle ?? "kit:server",
      client: conn,
      via: "test",
      root: "source",
      hunks,
      at_ms: this.net.now,
    };
  }

  private commit(conn: number, hunks: Hunk[], notify: number | null) {
    const before = this.text;
    this.text = apply(before, hunks);
    this.version++;
    this.log.push({ version: this.version, conn, hunks, before });
    for (const [c] of this.conns) {
      if (c !== notify) this.send(c, { type: "change", event: this.event(conn, this.version, hunks) });
    }
  }

  /** A change made on the server itself (an agent, an outside edit): broadcast to all. */
  serverEdit(hunks: Hunk[]) {
    const bad = validate(this.text, hunks);
    if (bad) throw new Error(`bad server edit ${JSON.stringify(bad)}`);
    if (hunks.length) this.commit(0, coalesce(hunks), null);
  }

  private receive(conn: number, f: ClientFrame) {
    switch (f.type) {
      case "edit": {
        const since = this.log.filter((a) => a.version > f.base);
        if (f.base > this.version || this.version - f.base > this.window) {
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
          return this.send(conn, {
            type: "refused",
            op: f.op,
            error: { id: "MPC-E151", message: `bad hunk ${bad.kind}` },
          });
        }
        const r = rebaseOurs(f.hunks, since.map((a) => a.hunks));
        if ("conflict" in r) {
          this.refused++;
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
          return this.send(conn, { type: "ack", op: f.op, version: this.version, hunks: [] });
        }
        this.commit(conn, r.ok, conn);
        return this.send(conn, { type: "ack", op: f.op, version: this.version, hunks: r.ok });
      }
      case "since": {
        const events = this.log.filter((a) => a.version > f.version).map((a) => this.event(a.conn, a.version, a.hunks));
        return this.send(conn, { type: "events", op: f.op, events });
      }
      case "undo": {
        // undo this connection's last change if the text it inserted is still where it put it
        const last = [...this.log].reverse().find((a) => a.conn === conn);
        if (!last) return this.send(conn, { type: "refused", op: f.op, error: { id: "MPC-E201", message: "nothing to undo" } });
        const later = this.log.filter((a) => a.version > last.version);
        const inv = invert(last.before, last.hunks);
        const r = rebaseOurs(inv, later.map((a) => a.hunks));
        if ("conflict" in r || r.ok.length === 0) {
          return this.send(conn, { type: "refused", op: f.op, error: { id: "MPC-E202", message: "can't undo" } });
        }
        return this.commit(conn, r.ok, null);
      }
      case "redo":
        return this.send(conn, { type: "refused", op: f.op, error: { id: "MPC-E203", message: "nothing to redo" } });
      case "cursor":
        return;
    }
  }

  /** Total UTF-8 length (for random edits). */
  get bytes() {
    return utf8Len(this.text);
  }
}
