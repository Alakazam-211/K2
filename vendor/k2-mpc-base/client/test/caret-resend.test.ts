// @vitest-environment jsdom
// The caret's re-send after remote changes (client 0.11.1): a host that drops a `cursor` frame
// unless its version is the one that socket was last sent, and that forwards whatever is queued
// before it compares (the fake host's opt-in caret route, `FakeServer.caretRoute`), would leave
// an idle person's caret unseen while others type. The client re-sends its caret at each new
// version, mapped as CodeMirror maps a selection, at most once per 250 ms with a trailing send.

import { ChangeSet, EditorSelection } from "@codemirror/state";
import { afterEach, describe, expect, it, vi } from "vitest";
import "./jsdom-shims";
import { mountLiveEditor, type LiveEditorHandle } from "../src/editor/mount";
import type { Hunk } from "../src/hunk";
import { CARET_RESEND_MS, LiveText, setDefaultLiveTimers, type LiveTimers } from "../src/live";
import type { ChangeEvent, ClientFrame, ServerFrame, Transport } from "../src/wire";
import { FakeLiveHost, microtasks, netTimers, settle, type FakeLiveTransport } from "./fake-live";
import { FakeServer, Net, rng } from "./fake-server";

const NAMES = { p1: "Ada", p2: "Bo" };
const TEXT = "the quick brown fox jumps over the lazy dog\n";
/** Where A puts its caret (characters = bytes: the text is ASCII). */
const CARET = 20;

const mounted: LiveEditorHandle[] = [];

afterEach(() => {
  for (const h of mounted.splice(0)) h.destroy();
  setDefaultLiveTimers(null);
  document.body.replaceChildren();
});

function setup(handles: string[], opts: { olderHost?: boolean } = {}) {
  const net = new Net();
  const server = new FakeServer(net, TEXT);
  server.caretRoute = { names: NAMES, olderHost: opts.olderHost };
  const r = rng(5);
  const host = new FakeLiveHost(net, server, () => 2);
  setDefaultLiveTimers(netTimers(net, r.next)); // the replicas' throttle on the fake clock
  const eds: LiveEditorHandle[] = [];
  const ts: FakeLiveTransport[] = [];
  for (const who of handles) {
    const el = document.createElement("div");
    document.body.append(el);
    const t = host.transport(who);
    const h = mountLiveEditor(el, { transport: t, mode: "plain", timers: netTimers(net, r.next), reconnect: { minMs: 5, maxMs: 40 } });
    mounted.push(h);
    eds.push(h);
    ts.push(t);
  }
  return { net, server, eds, ts };
}

const doc = (h: LiveEditorHandle) => h.view.state.doc.toString();
const type = (h: LiveEditorHandle, pos: number, s: string) =>
  h.view.dispatch({ changes: { from: pos, insert: s }, selection: { anchor: pos + s.length }, userEvent: "input.type" });
const cursors = (t: FakeLiveTransport) => t.sent.filter((f): f is Extract<ClientFrame, { type: "cursor" }> => f.type === "cursor");

/** Where `name`'s caret is drawn in `h` (a character offset), or `null` when it isn't. */
function caretOf(h: LiveEditorHandle, name: string): number | null {
  const all = [...h.view.dom.querySelectorAll(".cm-mpc-caret")] as HTMLElement[];
  const el = all.find((e) => e.querySelector(".cm-mpc-caret-name")?.textContent === name);
  return el ? h.view.posAtDOM(el) : null;
}
const caretsNamed = (h: LiveEditorHandle, name: string) =>
  [...h.view.dom.querySelectorAll(".cm-mpc-caret-name")].filter((e) => e.textContent === name).length;

/** Step the fake network until `cond` holds. */
async function until(net: Net, cond: () => boolean, max = 10_000) {
  for (let i = 0; i < max; i++) {
    await microtasks();
    if (cond()) return;
    if (!net.step()) {
      await microtasks();
      if (cond()) return;
      throw new Error("the network went idle first");
    }
  }
  throw new Error("never reached");
}

/** Step the fake network to time `t`, running `check` after every delivery. */
async function runTo(net: Net, t: number, check?: () => void) {
  net.at(Math.max(0, t - net.now), () => {});
  while (net.now < t && net.step()) {
    await microtasks();
    check?.();
  }
  await microtasks();
  check?.();
}

/** The host's periodic roster push, every `every` until `end`. */
function ticks(net: Net, server: FakeServer, end: number, every = 20) {
  const tick = () => {
    server.caretTick();
    if (net.now + every <= end) net.at(every, tick);
  };
  net.at(every, tick);
}

/**
 * The planted fault: no re-send after remote changes. A caret frame goes only when the person
 * moves the caret (or after their own edit): the remote path sends nothing, and the editor's
 * echo of the caret the replica moved isn't sent either.
 */
async function withoutRemoteResend<T>(f: () => Promise<T>): Promise<T> {
  type Inner = { resendCursor(): void; cursor: { anchor: number; head: number } | null };
  const proto = LiveText.prototype as unknown as Inner & { setCursor(a: number, h: number): void };
  const setCursor = proto.setCursor;
  const resend = vi.spyOn(proto, "resendCursor").mockImplementation(() => {});
  const echo = vi.spyOn(proto, "setCursor").mockImplementation(function (this: Inner, a: number, h: number) {
    if (this.cursor && this.cursor.anchor === a && this.cursor.head === h) return;
    setCursor.call(this, a, h);
  });
  try {
    return await f();
  } finally {
    resend.mockRestore();
    echo.mockRestore();
  }
}

/**
 * A puts its caret down while the host still holds one of B's changes for it (its frame is
 * read behind and dropped), then stays idle while B types a key every 50 at the start of the
 * line, before A's caret. Fails loudly unless A's caret is drawn in B's editor while B types,
 * stays drawn from then on at A's real position, and every frame A sent was mapped (one per
 * version, at the caret's place in that version's text).
 */
async function idleCaretWhileOtherTypes(keys = 20) {
  const { net, server, eds, ts } = setup(["p1", "p2"]);
  const [A, B] = eds;
  await settle(net);
  expect([A.state, B.state]).toEqual(["live", "live"]);
  // the host holds B's first key for A when A's caret frame is read
  server.setLag(ts[0].conn!, true);
  type(B, 0, "x");
  await until(net, () => server.version === 1);
  const t0 = net.now;
  A.view.dispatch({ selection: { anchor: CARET } });
  await until(net, () => server.caretDropped === 1);
  server.setLag(ts[0].conn!, false);
  expect(server.caretAccepted, "the caret frame was read behind and dropped").toBe(0);
  const typingEnds = t0 + 20 + 50 * keys;
  ticks(net, server, typingEnds + 100);
  const seen: { at: number | null } = { at: null };
  const check = () => {
    const at = caretOf(B, "Ada");
    if (at !== null && seen.at === null) seen.at = net.now;
    if (seen.at === null) return;
    expect(at, `A's caret left B's editor at ${net.now}`).not.toBeNull();
    // B types only at 0: A's caret is at its first place plus every x in B's text
    expect(at, `A's caret drawn at the wrong place at ${net.now}`).toBe(CARET + doc(B).length - TEXT.length);
  };
  for (let k = 0; k < keys; k++) {
    await runTo(net, t0 + 20 + 50 * k, check);
    type(B, 0, "x");
  }
  await runTo(net, typingEnds, check);
  const firstSeen = seen.at;
  expect(firstSeen, "A's caret never showed in B's editor while B typed").not.toBeNull();
  // everything lands; the last push
  await settle(net, check);
  server.caretTick();
  await settle(net, check);
  expect(doc(A)).toBe(doc(B));
  expect(caretOf(B, "Ada")).toBe(A.view.state.selection.main.head);
  // A's frames: one per version (the editor's echo of the mapped caret sends nothing), each at
  // the caret's place in that version's text (version v has v x's before it)
  const sent = cursors(ts[0]);
  const versions = sent.map((f) => f.version);
  expect(new Set(versions).size, `two caret frames at one version: ${JSON.stringify(sent)}`).toBe(versions.length);
  for (const f of sent.slice(1)) expect(f.anchor, `an unmapped caret frame ${JSON.stringify(f)}`).toBe(CARET + f.version);
  // throttled: the first, then at most one per window (and a trailing one)
  expect(sent.length).toBeLessThanOrEqual(2 + Math.ceil((net.now - t0) / CARET_RESEND_MS));
  return { t0, firstSeen: firstSeen ?? NaN, typingEnds, frames: sent.length, dropped: server.caretDropped, accepted: server.caretAccepted };
}

describe("the caret's re-send after remote changes", () => {
  it("an idle person's dropped caret shows in the typist's editor and stays, at its real place (fails with the re-send planted out)", async () => {
    const s = await idleCaretWhileOtherTypes();
    expect(s.firstSeen).toBeLessThan(s.typingEnds);
    console.log(`caret re-send: ${s.frames} frames from A, host dropped ${s.dropped}, accepted ${s.accepted}; seen ${s.firstSeen - s.t0} after A's first frame`);
    await expect(withoutRemoteResend(() => idleCaretWhileOtherTypes()), "the scenario passes without the re-send: it doesn't bite").rejects.toThrow(
      /never showed/,
    );
  });

  it("a caret frame read behind recovers within one throttle window while the other person types", async () => {
    const s = await idleCaretWhileOtherTypes(10);
    // A's first frame went at t0; the trailing re-send at t0 + 250, then a host tick (20) and
    // the links (2 each way)
    expect(s.firstSeen - s.t0, "the caret took longer than one throttle window to recover").toBeLessThanOrEqual(CARET_RESEND_MS + 50);
  });

  it("an older host keeps 0.11's limit: the same person on two devices never sees the other device's caret", async () => {
    // A host without per-connection carets (kit before v0.10.1, the route's `olderHost`): the
    // roster is one row per person (the first connection with a caret wins), and a replica
    // skips its own handle's row (`LiveText.carets`), exactly as 0.11 did.
    const { net, server, eds } = setup(["p1", "p1", "p2"], { olderHost: true });
    const [A1, A2, B] = eds;
    await settle(net);
    A2.view.dispatch({ selection: { anchor: 10 } });
    await settle(net);
    server.caretTick();
    await settle(net);
    expect(caretsNamed(B, "Ada"), "one row per person").toBe(1);
    expect(caretOf(B, "Ada")).toBe(10);
    expect(A1.view.dom.querySelectorAll(".cm-mpc-caret").length, "the other device's caret is not drawn (today)").toBe(0);
    // the first device's caret, once it has one, is the person's row: the second's is shown nowhere
    A1.view.dispatch({ selection: { anchor: 30 } });
    await settle(net);
    server.caretTick();
    await settle(net);
    expect(caretsNamed(B, "Ada")).toBe(1);
    expect(caretOf(B, "Ada")).toBe(30);
    expect(A2.view.dom.querySelectorAll(".cm-mpc-caret").length).toBe(0);
    expect(A1.view.dom.querySelectorAll(".cm-mpc-caret").length).toBe(0);
  });

  it("per-connection carets (kit v0.10.1): the same person on two devices sees the other device's caret", async () => {
    // A current host: a `Device` per connection and `client` on `cursor` events. Each device
    // shows the other's caret (drawn here by the kit's own editor, named as the person), never
    // its own; another person sees both.
    const { net, server, eds } = setup(["p1", "p1", "p2"]);
    const [A1, A2, B] = eds;
    await settle(net);
    A2.view.dispatch({ selection: { anchor: 10 } });
    await settle(net);
    server.caretTick();
    await settle(net);
    expect(caretOf(A1, "Ada"), "the other device's caret, in the first").toBe(10);
    expect(A2.view.dom.querySelectorAll(".cm-mpc-caret").length, "its own caret is not drawn").toBe(0);
    expect(caretOf(B, "Ada")).toBe(10);
    A1.view.dispatch({ selection: { anchor: 30 } });
    await settle(net);
    server.caretTick();
    await settle(net);
    expect(caretOf(A2, "Ada"), "the first device's caret, in the second").toBe(30);
    expect(caretOf(A1, "Ada"), "the second's stays in the first").toBe(10);
    expect(caretsNamed(A1, "Ada")).toBe(1);
    expect(caretsNamed(A2, "Ada")).toBe(1);
    expect(caretsNamed(B, "Ada"), "one caret a connection").toBe(2);
    // B types before both carets: each device's caret of the other moves with the text
    type(B, 0, "xy");
    await settle(net);
    expect(caretOf(A1, "Ada")).toBe(12);
    expect(caretOf(A2, "Ada")).toBe(32);
  });
});

// ---- the replica alone, on a clock the test drives

/** Timers the test advances by hand. */
function manualClock(): LiveTimers & { advance(ms: number): void } {
  let now = 0;
  let seq = 0;
  const q = new Map<number, { at: number; f: () => void }>();
  return {
    set: (f, ms) => {
      q.set(++seq, { at: now + ms, f });
      return seq;
    },
    clear: (h) => {
      q.delete(h as number);
    },
    now: () => now,
    advance(ms) {
      const end = now + ms;
      for (;;) {
        let next: [number, { at: number; f: () => void }] | null = null;
        for (const e of q) if (e[1].at <= end && (!next || e[1].at < next[1].at)) next = e;
        if (!next) break;
        q.delete(next[0]);
        now = next[1].at;
        next[1].f();
      }
      now = end;
    },
  };
}

function wire() {
  const sent: ClientFrame[] = [];
  const ls = new Set<(f: ServerFrame) => void>();
  const t: Transport = {
    send: (f) => sent.push(f),
    onFrame: (cb) => {
      ls.add(cb);
      return () => ls.delete(cb);
    },
  };
  const deliver = (f: ServerFrame) => ls.forEach((l) => l(f));
  const carets = () => sent.filter((f) => f.type === "cursor");
  return { t, sent, deliver, carets };
}

const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
const ev = (version: number, hunks: Hunk[], root = "source"): ChangeEvent => ({
  version,
  pid: "p2",
  client: 2,
  via: "tab",
  root,
  hunks,
  at_ms: 0,
});

describe("the caret's re-send (replica)", () => {
  it("throttles: 50 remote changes in 100 ms send the caret once, then once trailing at the last version", () => {
    const clock = manualClock();
    const w = wire();
    const l = new LiveText({ text: "abcdef", version: 1 }, w.t, { timers: clock });
    l.setCursor(3, 3);
    expect(w.carets()).toEqual([{ type: "cursor", root: "source", version: 1, anchor: 3, head: 3 }]);
    clock.advance(1000);
    for (let i = 0; i < 50; i++) {
      w.deliver({ type: "change", event: ev(2 + i, [h(0, 0, "x")]) });
      clock.advance(2);
    }
    expect(w.carets().slice(1)).toEqual([{ type: "cursor", root: "source", version: 2, anchor: 4, head: 4 }]);
    clock.advance(CARET_RESEND_MS);
    const after = w.carets().slice(1);
    console.log(`caret throttle: 50 remote changes in 100 ms, ${after.length} caret frames`);
    expect(after).toEqual([
      { type: "cursor", root: "source", version: 2, anchor: 4, head: 4 },
      { type: "cursor", root: "source", version: 51, anchor: 53, head: 53 },
    ]);
    // another root's change moves the version too: re-sent at it
    clock.advance(1000);
    w.deliver({ type: "change", event: ev(52, [h(0, 0, "n")], "notes") });
    expect(w.carets().at(-1)).toEqual({ type: "cursor", root: "source", version: 52, anchor: 53, head: 53 });
    expect(w.carets().length).toBe(4);
  });

  it("sends nothing on remote changes while its own edit is in flight; the ack sends the mapped caret", () => {
    const clock = manualClock();
    const w = wire();
    const l = new LiveText({ text: "abcdef", version: 1 }, w.t, { timers: clock });
    l.setCursor(3, 3);
    l.applyLocal([h(6, 6, "!")]);
    clock.advance(100);
    w.deliver({ type: "change", event: ev(2, [h(0, 0, "x")]) }); // inside the window: a trailing re-send...
    clock.advance(10);
    w.deliver({ type: "change", event: ev(3, [h(0, 0, "y")]) });
    clock.advance(CARET_RESEND_MS * 4); // ... comes due while the edit is still out
    expect(w.carets().length, "a caret sent with an edit in flight").toBe(1);
    w.deliver({ type: "change", event: ev(4, [h(0, 0, "z")]) }); // past the window: at once, held too
    expect(w.carets().length, "a caret sent with an edit in flight").toBe(1);
    w.deliver({ type: "ack", op: 1, version: 5, hunks: [h(9, 9, "!")] });
    expect(l.text).toBe("zyxabcdef!");
    expect(w.carets().slice(1)).toEqual([{ type: "cursor", root: "source", version: 5, anchor: 6, head: 6 }]);
  });

  it("maps the caret through a remote change exactly as CodeMirror maps a selection", () => {
    const r = rng(41);
    const alpha = "ab \n";
    let cases = 0;
    for (let run = 0; run < 600; run++) {
      const len = r.int(24);
      let text = "";
      for (let i = 0; i < len; i++) text += alpha[r.int(alpha.length)];
      // sorted, non-overlapping hunks (adjacent ones too)
      const cuts = Array.from({ length: 2 * (1 + r.int(3)) }, () => r.int(len + 1)).sort((a, b) => a - b);
      const hunks: Hunk[] = [];
      let insertAt: number | null = null;
      for (let i = 0; i < cuts.length; i += 2) {
        const start = cuts[i];
        const end = r.int(3) === 0 ? start : cuts[i + 1];
        const insertion = start === end;
        if (insertion && insertAt === start) continue; // one insertion per place (rule 7)
        hunks.push(h(start, end, insertion ? ["q", "qq"][r.int(2)] : ["", "q", "qq"][r.int(3)]));
        insertAt = insertion ? start : null;
      }
      const anchor = r.int(len + 1);
      const head = r.next() < 0.4 ? anchor : r.int(len + 1);
      const clock = manualClock();
      const w = wire();
      const l = new LiveText({ text, version: 1 }, w.t, { timers: clock });
      const got: { x: Hunk[] | null } = { x: null };
      l.onChange((c) => {
        if (!c.local) got.x = c.hunks;
      });
      l.setCursor(anchor, head);
      clock.advance(1000);
      w.deliver({ type: "change", event: ev(2, hunks) });
      const x = got.x;
      if (!x) continue; // nothing changed
      // the binding hands CodeMirror the change the replica emits (`changesOf`)
      const changes = ChangeSet.of(
        x.map((k) => ({ from: k.start, to: k.end, insert: k.insert })),
        len,
      );
      const want = EditorSelection.single(anchor, head).map(changes).main;
      expect(w.carets().at(-1), `run ${run}: ${JSON.stringify({ text, hunks, anchor, head })}`).toMatchObject({ version: 2, anchor: want.anchor, head: want.head });
      cases++;
    }
    expect(cases).toBeGreaterThan(400);
  });
});
