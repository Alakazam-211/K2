// The live wire contract (WIRE.md) against the fake host, which follows the host rules exactly:
// reconnects (a new connection and client ID; frames sent while down are dropped, never
// replayed; the in-flight edit settled by `since`), the own-edit filter by version, undo coming
// back as a change, the caret re-sent, the reload past the window, and the 4 MiB frame cap.

import { describe, expect, it } from "vitest";
import type { Hunk } from "../src/hunk";
import { LiveText, frameBytes, type Rejection, type UndoResult } from "../src/live";
import { FRAME_MARGIN, FRAME_MAX, type ClientFrame, type CloseEvent, type ServerFrame, type Transport } from "../src/wire";
import { FakeServer, Net } from "./fake-server";
import { runrec } from "./runrec";

const h = (start: number, end: number, insert = ""): Hunk => ({ start, end, insert });
const run = (net: Net) => {
  while (net.step());
};

/** A transport that records what went each way. */
function tap(t: Transport) {
  const sent: ClientFrame[] = [];
  const got: ServerFrame[] = [];
  const wrapped: Transport = {
    send: (f) => {
      sent.push(f);
      t.send(f);
    },
    onFrame: (cb) =>
      t.onFrame((f) => {
        got.push(f);
        cb(f);
      }),
  };
  return { t: wrapped, sent, got };
}

/** A replica over a `FakeLive`, with its callbacks recorded. */
async function open(server: FakeServer, handle: string) {
  const link = server.live(handle, () => 5);
  const closes: CloseEvent[] = [];
  link.onClose((e) => closes.push(e));
  const w = await link.connect({ signal: new AbortController().signal });
  const live = new LiveText({ text: w.text!, version: w.version }, link, { client: w.client, self: w.self });
  const acks: number[][] = [];
  const rej: Rejection[] = [];
  const reloads: string[] = [];
  const undos: UndoResult[] = [];
  const errors: string[] = [];
  live.onAck((a) => acks.push(a.ops));
  live.onRejected((r) => rej.push(r));
  live.onReload((e) => reloads.push(e.error?.id ?? e.reason));
  live.onUndo((u) => undos.push(u));
  live.onError((e) => errors.push(e.id));
  return { link, live, welcome: w, closes, acks, rej, reloads, undos, errors };
}

describe("reconnect (G4, G5)", () => {
  it("mid-edit, applied: the `since` reply lists it under the old client ID, and it settles as acked", async () => {
    const net = new Net();
    const server = new FakeServer(net, "hello");
    const c = await open(server, "ada");
    const first = c.welcome.client;
    c.live.setCursor(5, 5);
    const op = c.live.applyLocal([h(5, 5, " world")]);
    while (server.version < 1) net.step(); // the host applied it...
    c.link.drop(); // ...and the socket dropped before the ack arrived
    expect(c.acks).toEqual([]);
    expect(c.closes).toEqual([{ reason: "network", retry: true }]);
    const w = await c.link.connect({ resume: c.live.version, signal: new AbortController().signal });
    expect(w.text).toBeUndefined(); // a resume: no text
    expect(w.client).not.toBe(first); // a new connection, a new client ID
    c.live.reconnected(w);
    expect(c.live.client).toBe(w.client);
    run(net);
    expect(server.log[0].client).toBe(first); // applied under the old ID
    expect(c.acks).toEqual([[op]]);
    expect(c.rej).toEqual([]);
    expect(c.live.text).toBe("hello world");
    expect(c.live.version).toBe(1);
    expect(c.live.pending).toBe(0);
    // the caret went out again on the new connection
    const carets = server.presence.filter((p) => p.conn === c.link.conn && p.frame.type === "cursor");
    expect(carets.map((p) => p.frame)).toEqual([{ type: "cursor", root: "source", version: 1, anchor: 5, head: 5 }]);
  });

  it("mid-edit, not applied: rejected with MPC-E000, the text kept; frames sent while down are dropped, never replayed", async () => {
    const net = new Net();
    const server = new FakeServer(net, "ab");
    const c = await open(server, "ada");
    c.live.applyLocal([h(2, 2, "!")]);
    c.link.drop(); // the edit was still on the wire: lost
    c.live.applyLocal([h(0, 0, ">")], { group: "other" }); // queued behind it
    c.live.undo(); // sent while down: dropped
    c.live.setVisible(false); // dropped too, but re-sent on reconnect
    expect(c.link.dropped.map((f) => f.type)).toEqual(["undo", "visible"]);
    const w = await c.link.connect({ resume: c.live.version, signal: new AbortController().signal });
    c.live.reconnected(w);
    run(net);
    expect(c.rej.map((r) => [r.text, r.error?.id])).toEqual([["!", "MPC-E000"]]);
    expect(c.rej[0].region).toEqual({ start: 3, end: 3 }); // after ">ab" in the text now
    expect(server.log.map((a) => a.hunks)).toEqual([[h(0, 0, ">")]]); // never the "!", never an undo
    expect(c.live.text).toBe(">ab");
    expect(server.text).toBe(">ab");
    expect(c.undos).toEqual([]); // the dropped undo has no reply
    const resent = server.presence.filter((p) => p.conn === c.link.conn).map((p) => p.frame);
    expect(resent).toContainEqual({ type: "visible", visible: false });
    // the person re-applies it
    c.live.reapply(c.rej[0].id);
    run(net);
    expect(server.text).toBe(">ab!");
    expect(c.live.text).toBe(server.text);
  });

  it("re-sends the caret after a reconnect (the dedupe is cleared), at the same version", async () => {
    const net = new Net();
    const server = new FakeServer(net, "hello");
    const c = await open(server, "ada");
    c.live.setCursor(2, 2);
    c.live.setCursor(2, 2); // deduped
    run(net);
    expect(server.presence.filter((p) => p.frame.type === "cursor")).toHaveLength(1);
    c.link.drop();
    const w = await c.link.connect({ resume: c.live.version, signal: new AbortController().signal });
    c.live.reconnected(w);
    run(net);
    const second = server.presence.filter((p) => p.frame.type === "cursor" && p.conn === c.link.conn);
    expect(second.map((p) => p.frame)).toEqual([{ type: "cursor", root: "source", version: 0, anchor: 2, head: 2 }]);
  });

  it("resuming impossible: the welcome carries text, the replica resets, pending changes go to onRejected", async () => {
    const net = new Net();
    const server = new FakeServer(net, "abc");
    server.sinceWindow = 2;
    const c = await open(server, "ada");
    c.link.drop();
    for (const x of ["1", "2", "3"]) server.serverEdit([h(0, 0, x)]);
    c.live.applyLocal([h(3, 3, "!")]); // sent while down: dropped
    const w = await c.link.connect({ resume: c.live.version, signal: new AbortController().signal });
    expect(w.text).toBe("321abc");
    c.live.reconnected(w);
    run(net);
    expect(c.live.text).toBe("321abc");
    expect(c.live.version).toBe(3);
    expect(c.rej.map((r) => r.text)).toEqual(["!"]);
    expect(server.log).toHaveLength(3);
  });
});

describe("the own-edit filter (G3) and undo (G9)", () => {
  it("the host forwards events before V, skips V, then acks: no gap, no `since`", () => {
    const net = new Net();
    const server = new FakeServer(net, "abc");
    const a = tap(server.connect(1, () => 5));
    const la = new LiveText(server.snapshot(), a.t, { client: 1 });
    const lb = new LiveText(server.snapshot(), server.connect(2, () => 5), { client: 2 });
    server.setLag(1, true); // a's writer lags: b's change waits in a's queue
    lb.applyLocal([h(0, 0, "B")]);
    run(net);
    expect(a.got).toEqual([]);
    la.applyLocal([h(3, 3, "A")]);
    run(net);
    // b's change (v1) first, then the ack for v2; the event at v2 (a's own) never came
    expect(a.got.map((f) => f.type)).toEqual(["change", "ack"]);
    expect(a.got[0]).toMatchObject({ type: "change", event: { version: 1, pid: "p2" } });
    expect(a.got[1]).toMatchObject({ type: "ack", version: 2 });
    expect(a.sent.map((f) => f.type)).toEqual(["edit"]); // no `since`
    expect(la.text).toBe("BabcA");
    expect(la.text).toBe(server.text);

    // nothing applied (b deleted the same "a" first): every queued event, then the ack at the
    // version unchanged, nothing skipped
    lb.applyLocal([h(1, 2)]);
    run(net);
    la.applyLocal([h(1, 2)]);
    run(net);
    const last = a.got.slice(2);
    expect(last.map((f) => f.type)).toEqual(["change", "ack"]);
    expect(last[0]).toMatchObject({ event: { version: 3, pid: "p2" } });
    expect(last[1]).toEqual({ type: "ack", op: 2, version: 3, hunks: [] });
    expect(la.text).toBe("BbcA");
    expect(la.pending).toBe(0);
    server.setLag(1, false);
  });

  it("the client's own undo comes back as a `change` (same pid and client ID) and is applied; then `undone`", () => {
    const net = new Net();
    const server = new FakeServer(net, "abc");
    const a = tap(server.connect(1, () => 5));
    const la = new LiveText(server.snapshot(), a.t, { client: 1 });
    const undos: UndoResult[] = [];
    la.onUndo((u) => undos.push(u));
    la.applyLocal([h(3, 3, "!")]);
    run(net);
    expect(la.text).toBe("abc!");
    const op = la.undo();
    run(net);
    const back = a.got.filter((f) => f.type === "change");
    expect(back).toHaveLength(1);
    expect(back[0]).toMatchObject({ event: { version: 2, pid: "p1", client: 1, hunks: [h(3, 4)] } });
    expect(la.text).toBe("abc");
    expect(la.version).toBe(2);
    expect(undos).toEqual([{ op, kind: "undo", steps: 1, version: 2, notice: null }]);
    expect(a.got[a.got.length - 1].type).toBe("undone"); // after the change it made
  });

  it("nothing to undo: `undone` with 0 steps (no change, no error)", () => {
    const net = new Net();
    const server = new FakeServer(net, "abc");
    const l = new LiveText(server.snapshot(), server.connect(3, () => 5), { client: 3 });
    const undos: UndoResult[] = [];
    const errors: string[] = [];
    l.onUndo((u) => undos.push(u));
    l.onError((e) => errors.push(e.id));
    const op = l.redo();
    run(net);
    expect(undos).toEqual([{ op, kind: "redo", steps: 0, version: 0, notice: null }]);
    expect(errors).toEqual([]);
  });

  it("an undo whose text was changed since is refused (MPC-E201): onError", () => {
    const net = new Net();
    const server = new FakeServer(net, "abc");
    const l = new LiveText(server.snapshot(), server.connect(1, () => 5), { client: 1 });
    const errors: string[] = [];
    l.onError((e) => errors.push(e.id));
    l.applyLocal([h(3, 3, "XY")]);
    run(net);
    server.serverEdit([h(3, 5, "Z")]); // someone replaced it
    run(net);
    l.undo();
    run(net);
    expect(errors).toEqual(["MPC-E201"]);
    expect(l.text).toBe(server.text);
  });
});

describe("reload (MPC-E112 on `since`)", () => {
  it("past the window: onReload, then `reload()` asks for a snapshot and resets", () => {
    const net = new Net();
    const server = new FakeServer(net, "abc");
    server.sinceWindow = 2;
    const l = new LiveText(server.snapshot(), server.connect(1, () => 5), { client: 1 });
    const reloads: string[] = [];
    const changes: number[] = [];
    l.onReload((e) => reloads.push(e.error?.id ?? e.reason));
    l.onChange((e) => changes.push(e.hunks.length));
    server.setLag(1, true); // three changes the client hasn't seen
    for (const x of ["1", "2", "3"]) server.serverEdit([h(0, 0, x)]);
    l.resync(); // a wake (focus, `online`): ask what changed
    run(net);
    expect(reloads).toEqual(["MPC-E112"]);
    expect(l.version).toBe(0);
    l.reload();
    run(net);
    expect(l.text).toBe("321abc");
    expect(l.version).toBe(3);
    expect(changes).toEqual([1]); // the reset's one change
    server.setLag(1, false); // the queued events arrive late: already in the snapshot
    run(net);
    expect(l.text).toBe("321abc");
    expect(l.version).toBe(3);
  });
});

describe("the frame cap (G10: 4 MiB)", () => {
  it("an over-cap local edit is rejected before sending (MPC-E113), its text kept", async () => {
    const net = new Net();
    const server = new FakeServer(net, "ab");
    const c = await open(server, "ada");
    const big = "x".repeat(FRAME_MAX - FRAME_MARGIN);
    c.live.applyLocal([h(2, 2, big)]);
    expect(c.rej).toHaveLength(1);
    expect(c.rej[0].error?.id).toBe("MPC-E113");
    expect(c.rej[0].text).toBe(big);
    expect(c.rej[0].error?.details).toMatchObject({ limit: FRAME_MAX - FRAME_MARGIN, frame_max: FRAME_MAX });
    expect(c.link.sent.filter((s) => s.frame.type === "edit")).toEqual([]);
    expect(c.live.text).toBe("ab");
    // an edit under the limit goes
    c.live.applyLocal([h(2, 2, "x".repeat(1000))]);
    run(net);
    expect(server.text.length).toBe(1002);
    expect(c.acks).toHaveLength(1);
  });

  it("the host refuses an over-cap frame: `refused` for an `edit`, `error` for the rest", async () => {
    const net = new Net();
    const server = new FakeServer(net, "ab");
    const c = await open(server, "ada");
    const got: ServerFrame[] = [];
    c.link.onFrame((f) => got.push(f));
    c.link.send({ type: "edit", op: 99, root: "source", base: 0, hunks: [h(0, 0, "y".repeat(FRAME_MAX))] });
    c.link.send({ type: "hello", root: "z".repeat(FRAME_MAX) });
    run(net);
    expect(got.map((f) => f.type)).toEqual(["refused", "error"]);
    expect(got[0]).toMatchObject({ type: "refused", op: 99, error: { id: "MPC-E113" } });
    expect(got[1]).toMatchObject({ type: "error", error: { id: "MPC-E113" } });
    // a refusal of an op the replica never sent, and the `error` frame, both reach onError
    expect(c.errors).toEqual(["MPC-E113", "MPC-E113"]);
    expect(server.log).toEqual([]);
  });

  it("a doc too big for one frame: `welcome` is declined (MPC-E401), no retry", async () => {
    const net = new Net();
    const server = new FakeServer(net, "x".repeat(FRAME_MAX));
    const link = server.live("ada", () => 5);
    const closes: CloseEvent[] = [];
    link.onClose((e) => closes.push(e));
    await expect(link.connect({ signal: new AbortController().signal })).rejects.toMatchObject({
      reason: "declined",
      retry: false,
      error: { id: "MPC-E401" },
    });
    expect(closes).toHaveLength(1);
  });

  it("a doc that grows past the cap: the change is withheld, `since` refused, the snapshot declined", async () => {
    const net = new Net();
    const server = new FakeServer(net, "ab");
    const c = await open(server, "ada");
    c.live.onReload(() => c.live.reload());
    server.serverEdit([h(2, 2, "y".repeat(FRAME_MAX))]); // over the cap: not sent
    server.serverEdit([h(0, 0, ">")]); // the client sees the gap
    run(net);
    expect(server.withheld).toEqual(["change", "events", "snapshot"]);
    expect(c.reloads).toEqual(["MPC-E112"]);
    expect(c.closes).toEqual([
      { reason: "declined", retry: false, error: expect.objectContaining({ id: "MPC-E401" }) },
    ]);
    expect(c.live.text).toBe("ab"); // what it had; the component shows the file another way
  });

  it("sizes: the largest frames the kit makes fit, escapes included", () => {
    // a 1 MiB Markdown note: the `welcome` that carries it
    const line = (i: number) => `- [ ] item ${i}: "quoted" text, a \`code\` span, café\t— and a backslash \\ here\n`;
    let md = "# Notes\n\n";
    for (let i = 0; md.length < 1 << 20; i++) md += line(i);
    const welcome: Extract<ServerFrame, { type: "welcome" }> = {
      type: "welcome",
      root: "source",
      version: 123456,
      text: md,
      client: 2 ** 53 - 1,
      self: "ada",
      rights: "edit",
    };
    const welcomeBytes = new TextEncoder().encode(JSON.stringify(welcome)).length;
    expect(welcomeBytes).toBeLessThan(FRAME_MAX);
    // the worst escape: 1 MiB of control characters is 6 MiB of JSON (over the cap)
    const worst = { ...welcome, text: "\u0001".repeat(1 << 20) };
    const worstBytes = new TextEncoder().encode(JSON.stringify(worst)).length;
    expect(worstBytes).toBeGreaterThan(FRAME_MAX);
    // a roster snapshot carrying a person's waiting removed-text notes at their bounds: 64
    // notes of 16 KiB of text each (1 MiB, the per-doc bound), the text in the note and in its
    // one-line message
    const chunk = md.slice(0, 16 * 1024);
    const notes = Array.from({ length: 64 }, (_, i) => ({
      rule: "removed_text",
      change: 100 + i,
      version: 99 + i,
      yours: chunk,
      not_shown: 0,
      message: `An outside save may have removed your text. Version ${99 + i} holds it. It read ${JSON.stringify(chunk)}.`,
    }));
    const members = Array.from({ length: 32 }, (_, i) => ({
      handle: `participant-${i}`,
      name: `Person ${i}`,
      colour: "#7c3aed",
      kind: "person",
      role: "editor",
      away: false,
      cursor: { root: "source", anchor: i * 100, head: i * 100 + 7, version: 123456, anchor_lc: "12:3", head_lc: "12:10" },
      rights: "edit",
    }));
    const roster = { type: "roster_event", event: { kind: "snapshot", version: 123456, members, watching: 0, overwritten: notes } };
    const rosterBytes = new TextEncoder().encode(JSON.stringify(roster)).length;
    expect(rosterBytes).toBeLessThan(FRAME_MAX);
    // a whole-text replacement of the 1 MiB note as one `change`
    const change = {
      type: "change",
      event: { version: 1, pid: "agent-7", client: 2, via: "cli", root: "source", hunks: [{ start: 0, end: 1 << 20, insert: md }], at_ms: 1, on_behalf_of: "ada" },
    };
    const changeBytes = new TextEncoder().encode(JSON.stringify(change)).length;
    expect(changeBytes).toBeLessThan(FRAME_MAX);
    // `frameBytes` agrees with the encoder
    expect(frameBytes(welcome, 0)).toBe(welcomeBytes);
    console.log(`frame sizes: welcome(1 MiB md) ${welcomeBytes}, worst-escape ${worstBytes}, roster+notes ${rosterBytes}, change ${changeBytes}`);
    runrec("client_frame_sizes", {
      frame_max: FRAME_MAX,
      welcome_1mib: welcomeBytes,
      worst_escape_1mib: worstBytes,
      roster_notes_1mib: rosterBytes,
      change_1mib: changeBytes,
    });
  });
});
