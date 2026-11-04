// @vitest-environment jsdom
// Undo and redo through the mounted editor while typing is still on its way: Ctrl+Z (and
// Ctrl+Shift+Z) must reach the server after the edits typed before it, never ahead of them.
//
// The server undoes the top of the person's undo stack when the request arrives. An `undo`
// frame sent at once, while an edit waits behind the one in flight, arrives first: the server
// undoes the earlier change, and the later edit, landing after it, clears the redo stack.
//
// The vendored fake server's undo takes the person's last log entry (its own undo included)
// and its redo does nothing, so this file gives it the kit server's per-person stacks
// (`crates/base/src/session/live.rs`, `undo_redo`): undo pops the person's undo stack when the
// request arrives and pushes onto their redo stack; redo pops the redo stack and pushes onto
// the undo stack; a new edit by the person clears their redo stack. Each edit is its own step
// here (no token grouping), so the typing below is chosen to be two steps on the kit server
// too: "c", then a space after a word character (U3 (c): a word boundary starts the next step).

import { afterEach, describe, expect, it } from "vitest";
import "../../../../../vendor/k2-mpc-base/client/test/jsdom-shims";
import { mountLiveEditor, type LiveEditorHandle } from "../mount";
import { invert, rebaseOurs, type ClientFrame, type Hunk, type ServerFrame } from "@k2/mpc-client";
import { FakeLiveHost, netTimers, settle, type FakeLiveTransport } from "../../../../../vendor/k2-mpc-base/client/test/fake-live";
import { FakeServer, Net, rng } from "../../../../../vendor/k2-mpc-base/client/test/fake-server";
import { frames } from "./editor-harness";

/** The fake server's internals this file drives (private in its class). */
interface Internals {
  receive(conn: number, f: ClientFrame, bytes: number): void;
  commit(conn: number, hunks: Hunk[], editor: number | null, kind: "edit" | "undo" | "redo"): number;
  drain(conn: number): void;
  send(conn: number, f: ServerFrame): void;
}

/** Give `server` the kit's per-person undo and redo stacks (log versions of each step). */
function withUndoStacks(server: FakeServer) {
  const s = server as unknown as Internals;
  const receive = s.receive.bind(server);
  const stacks = new Map<string, { undo: number[]; redo: number[] }>();
  s.receive = (conn, f, bytes) => {
    const pid = server.handleOf(conn);
    if (!pid || (f.type !== "edit" && f.type !== "undo" && f.type !== "redo")) return receive(conn, f, bytes);
    let st = stacks.get(pid);
    if (!st) stacks.set(pid, (st = { undo: [], redo: [] }));
    if (f.type === "edit") {
      const was = server.version;
      receive(conn, f, bytes);
      if (server.version > was) {
        // the edit landed: a step of its own; a new edit clears the person's redo stack
        st.undo.push(server.version);
        st.redo.length = 0;
      }
      return;
    }
    // the top of the person's stack, as it stands when the request arrives
    const top = (f.type === "undo" ? st.undo : st.redo).pop();
    if (top === undefined) {
      s.drain(conn);
      return s.send(conn, { type: "undone", op: f.op, steps: 0, version: server.version });
    }
    const a = server.log.find((x) => x.version === top)!;
    const later = server.log.filter((x) => x.version > top).map((x) => x.hunks);
    const r = rebaseOurs(invert(a.before, a.hunks), later);
    if ("conflict" in r || r.ok.length === 0) {
      s.drain(conn);
      return s.send(conn, { type: "refused", op: f.op, error: { id: "MPC-E201", message: "can't undo" } });
    }
    const v = s.commit(conn, r.ok, null, f.type);
    (f.type === "undo" ? st.redo : st.undo).push(v);
    s.drain(conn);
    return s.send(conn, { type: "undone", op: f.op, steps: 1, version: v });
  };
}

function setup(text: string) {
  const net = new Net();
  const server = new FakeServer(net, text);
  withUndoStacks(server);
  const host = new FakeLiveHost(net, server, () => 1);
  const el = document.createElement("div");
  document.body.append(el);
  const t: FakeLiveTransport = host.transport("p1");
  const timers = netTimers(net, rng(7).next);
  const h = mountLiveEditor(el, { transport: t, timers, schedule: frames(timers), reconnect: { minMs: 5, maxMs: 40 } });
  return { net, server, h, t };
}

const doc = (h: LiveEditorHandle) => h.view.state.doc.toString();
const type = (h: LiveEditorHandle, pos: number, s: string) =>
  h.view.dispatch({ changes: { from: pos, insert: s }, selection: { anchor: pos + s.length }, userEvent: "input.type" });
const key = (h: LiveEditorHandle, shift = false) =>
  h.view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: shift ? "Z" : "z", keyCode: 90, ctrlKey: true, shiftKey: shift, bubbles: true, cancelable: true }));
const sent = (t: FakeLiveTransport, type: ClientFrame["type"]) => t.sent.filter((f) => f.type === type).length;

afterEach(() => {
  document.body.replaceChildren();
});

describe("undo and redo behind the typing still on its way", () => {
  it("Ctrl+Z right after fast typing undoes the last thing typed, not the one before it", async () => {
    const { net, server, h, t } = setup("ab");
    await settle(net);
    expect(h.state).toBe("live");
    // typed fast: "c" goes out at once, the space waits behind it (nothing acked yet)
    type(h, 2, "c");
    type(h, 3, " ");
    expect(h.pending).toBeGreaterThan(0);
    key(h); // Ctrl+Z
    await settle(net);
    expect(sent(t, "undo"), "Ctrl+Z went to the server").toBe(1);
    expect(server.text, "the space (typed last) is the one undone").toBe("abc");
    expect(doc(h)).toBe(server.text);
  });

  it("Ctrl+Shift+Z then brings back what that Ctrl+Z took out", async () => {
    const { net, server, h, t } = setup("ab");
    await settle(net);
    type(h, 2, "c");
    type(h, 3, " ");
    key(h); // Ctrl+Z, while the space is still waiting
    await settle(net);
    key(h, true); // Ctrl+Shift+Z, everything settled
    await settle(net);
    expect(sent(t, "redo"), "Ctrl+Shift+Z went to the server").toBe(1);
    expect(server.text, "the redo puts the space back (no later edit cleared the redo stack)").toBe("abc ");
    expect(doc(h)).toBe(server.text);
  });
});
