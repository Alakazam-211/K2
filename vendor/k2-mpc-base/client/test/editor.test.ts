// @vitest-environment jsdom
// The live editor, mounted (jsdom), against the fake server: the connection's states, rights,
// carets and labels, rejections and their actions, reconnects (an edit kept while down, an edit
// handed over before a drop), MPC-E112 by snapshot, closed, the host's history kept out, notes,
// and an IME composition with remote changes in the middle.

import { history, historyKeymap, undo as cmUndo } from "@codemirror/commands";
import { EditorState, StateEffect, Text } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import { afterEach, describe, expect, it, vi } from "vitest";
import "./jsdom-shims";
import { ByteIndex } from "../src/editor/bytes";
import { readableOn } from "../src/editor/colour";
import {
  CLOSE_RETRY_ERROR,
  DEFAULT_STRINGS,
  HISTORY_ERROR,
  LiveEditorError,
  mountLiveEditor,
  type LiveEditorEvents,
  type LiveEditorHandle,
  type LiveEditorOptions,
} from "../src/editor/mount";
import type { RemovedTextNote } from "../src/editor/notes";
import { LiveText } from "../src/live";
import { rebaseOurs } from "../src/rebase";
import { byteToIndex, indexToByte, utf8Len } from "../src/hunk";
import type { ClientFrame, LiveTransport, ServerFrame } from "../src/wire";
import { FakeLiveHost, type FakeLiveTransport, microtasks, netTimers, settle } from "./fake-live";
import { withoutPresenceCheck, withoutReapplyGuard } from "./editor-harness";
import { FakeServer, Net, rng } from "./fake-server";
import { randomEdit, randomText } from "./random-edits";

type Log = { [K in keyof LiveEditorEvents]: LiveEditorEvents[K][] };

function setup(text: string, n = 1, o: Partial<LiveEditorOptions> & { rights?: "read" | "edit" } = {}) {
  const net = new Net();
  const server = new FakeServer(net, text);
  const r = rng(7);
  const host = new FakeLiveHost(net, server, () => 1);
  const eds: LiveEditorHandle[] = [];
  const ts: FakeLiveTransport[] = [];
  const logs: Log[] = [];
  for (let i = 0; i < n; i++) {
    const el = document.createElement("div");
    document.body.append(el);
    const t = host.transport(`p${i + 1}`, { rights: o.rights });
    const h = mountLiveEditor(el, { transport: t, timers: netTimers(net, r.next), reconnect: { minMs: 5, maxMs: 40 }, ...o });
    const log = {} as Log;
    for (const k of ["state", "roster", "rights", "change", "pending", "ack", "rejected", "conflict", "note", "claim", "reload", "closed", "error", "participantClick"] as const) {
      (log as Record<string, unknown[]>)[k] = [];
      h.on(k, (v: unknown) => (log as Record<string, unknown[]>)[k].push(v));
    }
    eds.push(h);
    ts.push(t);
    logs.push(log);
  }
  return { net, server, host, eds, ts, logs };
}

const doc = (h: LiveEditorHandle) => h.view.state.doc.toString();
const type = (h: LiveEditorHandle, pos: number, s: string) =>
  h.view.dispatch({ changes: { from: pos, insert: s }, selection: { anchor: pos + s.length }, userEvent: "input.type" });
const edits = (t: FakeLiveTransport) => t.sent.filter((f) => f.type === "edit");

/** Step the fake network until `cond` holds (microtasks run between deliveries). */
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

afterEach(() => {
  document.body.replaceChildren();
});

describe("the byte index", () => {
  it("matches the reference conversions through random edits", () => {
    const r = rng(11);
    for (let run = 0; run < 150; run++) {
      let text = randomText(r, 30);
      let state = EditorState.create({ doc: text });
      let idx = ByteIndex.of(state.doc);
      for (let k = 0; k < 12; k++) {
        const hs = randomEdit(r, text);
        if (!hs.length) continue;
        const changes = hs.map((h) => ({ from: byteToIndex(text, h.start), to: byteToIndex(text, h.end), insert: h.insert }));
        const tr = state.update({ changes });
        idx = idx.update(state.doc, tr.state.doc, tr.changes);
        state = tr.state;
        text = state.doc.toString();
        expect(idx.total(), `run ${run}`).toBe(utf8Len(text));
        for (let p = 0; p <= text.length; p++) expect(idx.byteAt(state.doc, p)).toBe(indexToByte(text, p));
        for (let b = 0; b <= utf8Len(text) + 1; b++) expect(idx.posAt(state.doc, b), `run ${run} byte ${b}`).toBe(byteToIndex(text, b));
        // a fresh index agrees
        const fresh = ByteIndex.of(Text.of(text.split("\n")));
        expect(fresh.total()).toBe(idx.total());
      }
    }
  });
});

describe("mountLiveEditor", () => {
  it("connects, goes live, sends typing, shows remote changes, and reports pending and acks", async () => {
    const { net, server, eds, logs, ts } = setup("hello\nworld\n");
    const [h] = eds;
    expect(h.state).toBe("connecting");
    expect(h.view.state.readOnly).toBe(true);
    await settle(net);
    expect(h.state).toBe("live");
    expect(logs[0].state).toEqual(["live"]);
    expect(doc(h)).toBe("hello\nworld\n");
    expect(h.view.state.readOnly).toBe(false);
    expect(h.view.dom.closest(".mpc-editor")).not.toBeNull();
    type(h, 5, " there");
    expect(logs[0].pending).toEqual([1]);
    await settle(net);
    expect(server.text).toBe("hello there\nworld\n");
    expect(logs[0].ack.length).toBe(1);
    expect(logs[0].pending).toEqual([1, 0]);
    server.serverEdit([{ start: 0, end: 0, insert: ">> " }]);
    await settle(net);
    expect(doc(h)).toBe(server.text);
    expect(h.text).toBe(server.text);
    expect(h.version).toBe(server.version);
    expect(logs[0].change.some((c) => !c.local)).toBe(true);
    expect(logs[0].error).toEqual([]);
    expect(ts[0].connects).toBe(1);
    h.destroy();
    expect(ts[0].closedByEditor).toBe(true);
  });

  it("is read-only without the edit right, and goes read-only when a rights event takes it", async () => {
    const ro = setup("text", 1, { rights: "read" });
    await settle(ro.net);
    expect(ro.eds[0].rights).toBe("read");
    expect(ro.eds[0].view.state.readOnly).toBe(true);
    expect(ro.eds[0].view.dom.parentElement!.parentElement!.querySelector(".mpc-banner")!.textContent).toContain("Read-only");

    const { net, server, eds, ts, logs } = setup("text");
    await settle(net);
    expect(eds[0].view.state.readOnly).toBe(false);
    server.sendTo(ts[0].conn!, { type: "roster_event", event: { kind: "rights", who: "p1", rights: "read" } });
    await settle(net);
    expect(logs[0].rights).toContain("read");
    expect(eds[0].view.state.readOnly).toBe(true);
    // someone else's rights change nothing here
    server.sendTo(ts[0].conn!, { type: "roster_event", event: { kind: "rights", who: "p9", rights: "edit" } });
    await settle(net);
    expect(eds[0].rights).toBe("read");

    // readOnly in the options wins over the edit right
    const forced = setup("text", 1, { readOnly: true });
    await settle(forced.net);
    expect(forced.eds[0].rights).toBe("edit");
    expect(forced.eds[0].view.state.readOnly).toBe(true);
  });

  it("draws remote carets with readable labels, below the caret on line 1, and a roster strip", async () => {
    const { net, server, eds, ts, logs } = setup("first line\nsecond line\n", 1, { labels: "always" });
    await settle(net);
    const v = server.version;
    const roster: ServerFrame = {
      type: "roster_event",
      event: {
        kind: "snapshot",
        version: v,
        members: [
          { handle: "p1", name: "Me", colour: "#000000", kind: "person", role: "editor", away: false, cursor: null },
          { handle: "p2", name: "Ada", colour: "#ffeb3b", kind: "person", role: "editor", away: false, cursor: { root: "source", anchor: 2, head: 2, version: v } },
          { handle: "p3", name: "Bot", colour: "#1a237e", kind: "agent", role: "editor", away: true, cursor: { root: "source", anchor: 11, head: 17, version: v } },
        ],
      },
    };
    server.sendTo(ts[0].conn!, roster);
    await settle(net);
    await microtasks();
    const dom = eds[0].view.dom;
    const labels = [...dom.querySelectorAll(".cm-mpc-caret-name")] as HTMLElement[];
    expect(labels.map((l) => l.textContent).sort()).toEqual(["Ada", "Bot"]);
    const ada = labels.find((l) => l.textContent === "Ada")!;
    const bot = labels.find((l) => l.textContent === "Bot")!;
    expect(ada.classList.contains("cm-mpc-caret-name-below")).toBe(true); // line 1
    expect(bot.classList.contains("cm-mpc-caret-name-below")).toBe(false);
    expect(ada.classList.contains("cm-mpc-caret-name-shown")).toBe(true);
    expect((ada.parentElement as HTMLElement).style.getPropertyValue("--mpc-user-fg")).toBe("#000000");
    expect((bot.parentElement as HTMLElement).style.getPropertyValue("--mpc-user-fg")).toBe("#ffffff");
    expect(dom.querySelector(".cm-mpc-selection")?.textContent).toBe("second");
    // the roster strip: everyone, self marked, a click reported
    const frame = dom.closest(".mpc-editor")!;
    const chips = [...frame.querySelectorAll(".mpc-roster button")] as HTMLButtonElement[];
    expect(chips.map((c) => c.textContent)).toEqual(["Me (you)", "Ada", "Bot"]);
    expect(chips[2].dataset.away).toBe("true");
    chips[1].click();
    expect(logs[0].participantClick.map((p) => p.handle)).toEqual(["p2"]);
    expect(logs[0].roster.length).toBeGreaterThan(0);
  });

  it("labels `active`: a caret's name shows after it moves, then hides", async () => {
    const { net, server, eds, ts } = setup("abc\ndef\n", 1, { labels: "active" });
    await settle(net);
    const v = server.version;
    const at = (n: number): ServerFrame => ({ type: "roster_event", event: { kind: "cursor", who: "p2", cursor: { root: "source", anchor: n, head: n, version: v } } });
    server.sendTo(ts[0].conn!, { type: "roster_event", event: { kind: "joined", who: "p2", participant: { name: "Ada", colour: "#7c3aed", kind: "person" } } });
    server.sendTo(ts[0].conn!, at(5));
    const label = () => eds[0].view.dom.querySelector(".cm-mpc-caret-name") as HTMLElement | null;
    // shown once the caret moved (settling all the way would also run the expiry timer)
    await until(net, () => label()?.classList.contains("cm-mpc-caret-name-shown") ?? false);
    net.at(5000, () => {});
    await settle(net);
    await microtasks();
    expect(label()!.classList.contains("cm-mpc-caret-name-shown")).toBe(false);
  });

  it("shows a rejection with Re-apply and Dismiss buttons, and reports MPC-E111 as a conflict", async () => {
    const { net, server, eds, ts, logs } = setup("hello world\n");
    await settle(net);
    const [h] = eds;
    // the person types inside "world" while an outside edit deletes it first
    h.view.dispatch({ changes: { from: 8, insert: "WORLD" }, userEvent: "input.type" });
    server.serverEdit([{ start: 6, end: 11, insert: "" }]);
    await settle(net);
    expect(server.text).toBe("hello \n");
    expect(doc(h)).toBe(server.text);
    expect(logs[0].rejected.length).toBe(1);
    expect(logs[0].conflict.length).toBe(1);
    expect(logs[0].conflict[0].error.id).toBe("MPC-E111");
    const frame = h.view.dom.closest(".mpc-editor")!;
    const list = frame.querySelector(".mpc-rejections") as HTMLElement;
    expect(list.hidden).toBe(false);
    const again = list.querySelector("button.mpc-reapply") as HTMLButtonElement;
    const dismiss = list.querySelector("button.mpc-dismiss") as HTMLButtonElement;
    expect(again.textContent).toBe("Re-apply");
    expect(dismiss.textContent).toBe("Dismiss");
    // reachable from the keyboard: real buttons, focusable, enabled
    for (const b of [again, dismiss]) {
      expect(b.tabIndex).toBeGreaterThanOrEqual(0);
      expect(b.disabled).toBe(false);
      b.focus();
      expect(document.activeElement).toBe(b);
    }
    // its place was inside the deleted "world": Re-apply refuses (check and retype), keeps it
    again.click();
    await settle(net);
    expect(server.text).toBe("hello \n");
    expect(h.rejected.length).toBe(1);
    expect((frame.querySelector(".mpc-note") as HTMLElement).textContent).toBe(DEFAULT_STRINGS.deletedAround);
    dismiss.click();
    expect(h.rejected.length).toBe(0);
    expect(list.hidden).toBe(true);

    // a lost insertion (its reply gone with the socket): Re-apply puts it back, once
    type(h, 6, "WORLD");
    ts[0].drop(false);
    await settle(net);
    expect(h.rejected.length).toBe(1);
    (list.querySelector("button.mpc-reapply") as HTMLButtonElement).click();
    await settle(net);
    expect(server.text).toBe("hello WORLD\n");
    expect(doc(h)).toBe(server.text);
    expect(h.rejected.length).toBe(0);
    expect(list.hidden).toBe(true);

    // a second one, dismissed
    h.view.dispatch({ changes: { from: 0, to: 5, insert: "HELLO" }, userEvent: "input.type" });
    server.serverEdit([{ start: 0, end: 5, insert: "howdy" }]);
    await settle(net);
    expect(h.rejected.length).toBe(1);
    (list.querySelector("button.mpc-dismiss") as HTMLButtonElement).click();
    expect(h.rejected.length).toBe(0);
    expect(list.hidden).toBe(true);
    expect(doc(h)).toBe(server.text);
  });

  it("after a drop: reconnecting, then a new connection resumes and resyncs", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n", 2);
    await settle(net);
    ts[0].failConnects = 2;
    ts[0].drop();
    server.serverEdit([{ start: 0, end: 0, insert: "1" }]);
    type(eds[1], 0, "B");
    await settle(net);
    expect(logs[0].state).toEqual(["live", "reconnecting", "live"]);
    expect(ts[0].connects).toBe(4);
    expect(ts[0].resumes.slice(1).every((r) => typeof r === "number")).toBe(true);
    expect(doc(eds[0])).toBe(server.text);
    expect(doc(eds[1])).toBe(server.text);
    const banner = eds[0].view.dom.closest(".mpc-editor")!.querySelector(".mpc-banner") as HTMLElement;
    expect(banner.hidden).toBe(true);
    type(eds[0], 0, "A");
    await settle(net);
    expect(doc(eds[1])).toBe(server.text);
    expect(server.text.startsWith("A")).toBe(true);
    expect(logs[0].rejected).toEqual([]);
    expect(logs[0].error).toEqual([]);
  });

  it("an edit typed while the editor knows it's down is sent once, after the welcome", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n");
    await settle(net);
    ts[0].failConnects = 3;
    ts[0].drop();
    await until(net, () => eds[0].state === "reconnecting");
    const before = edits(ts[0]).length;
    type(eds[0], 3, "X");
    type(eds[0], 4, "Y");
    expect(eds[0].pending).toBeGreaterThan(0);
    expect(ts[0].conn).toBeNull();
    await settle(net);
    const sent = edits(ts[0]).slice(before) as Array<{ op: number }>;
    expect(new Set(sent.map((f) => f.op)).size).toBe(sent.length); // no op sent twice
    expect(server.text).toBe("abcXY\n");
    expect(doc(eds[0])).toBe(server.text);
    expect(logs[0].rejected).toEqual([]);
    expect(eds[0].pending).toBe(0);
  });

  it("an edit handed over before a drop that the server applied is recognised (no MPC-E000)", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n", 2);
    await settle(net);
    type(eds[0], 3, "Y");
    ts[0].drop(true); // the edit reaches the server; its ack is lost
    server.serverEdit([{ start: 0, end: 0, insert: "s" }]);
    await settle(net);
    expect(server.text).toBe("sabcY\n");
    expect(doc(eds[0])).toBe(server.text);
    expect(doc(eds[1])).toBe(server.text);
    expect(logs[0].rejected).toEqual([]);
    expect(edits(ts[0]).length).toBe(1); // never sent again
    expect(eds[0].pending).toBe(0);
  });

  it("an edit handed over before a drop that never arrived is MPC-E000, its text kept to re-apply", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n");
    await settle(net);
    type(eds[0], 3, "Y");
    ts[0].drop(false); // lost with the socket
    await settle(net);
    expect(server.text).toBe("abc\n");
    expect(edits(ts[0]).length).toBe(1);
    expect(logs[0].rejected.length).toBe(1);
    expect(logs[0].rejected[0].error?.id).toBe("MPC-E000");
    expect(logs[0].rejected[0].text).toBe("Y");
    expect(doc(eds[0])).toBe("abc\n");
    // its reply was lost with the socket: marked, and Re-apply checks first (it isn't there)
    expect(logs[0].rejected[0].mayHaveApplied).toBe(true);
    expect(eds[0].reapply(logs[0].rejected[0].id)).toEqual({ status: "applied" });
    await settle(net);
    expect(server.text).toBe("abcY\n");
    expect(doc(eds[0])).toBe(server.text);
  });

  describe("seed 27's shape: an edit kept while disconnected, a second drop before it settles", () => {
    /**
     * The script: three editors; p1's socket A drops; while p1 is disconnected it types "K", an
     * outside edit lands and p2 undoes its own change; p1 reconnects as B (prev A) and "K" goes
     * out on B; B drops half-open with "K" in flight, and "K" lands under B's client ID; p1
     * reconnects as C (prev B) and resyncs. `oldKept`: the pre-4b3816c editor, which kept the
     * frame itself and sent it after the welcome, under an ID LiveText didn't know.
     */
    async function script(rule: boolean, oldKept: boolean) {
      const net = new Net();
      const server = new FakeServer(net, "one two\n");
      server.leaveOldOnReconnect = rule;
      server.noRuleLeave = "keepalive";
      const host = new FakeLiveHost(net, server, () => 1);
      const eds: LiveEditorHandle[] = [];
      const ts: FakeLiveTransport[] = [];
      const rejected: Array<LiveEditorEvents["rejected"]> = [];
      for (const p of ["p1", "p2", "p3"]) {
        const el = document.createElement("div");
        document.body.append(el);
        const t = host.transport(p);
        const transport = p === "p1" && oldKept ? keptOutside(t) : t;
        const h = mountLiveEditor(el, { transport, timers: netTimers(net, () => 0.5), reconnect: { minMs: 5, maxMs: 5 } });
        if (p === "p1") h.on("rejected", (r) => rejected.push(r));
        eds.push(h);
        ts.push(t);
      }
      await settle(net);
      type(eds[1], 0, "B ");
      await settle(net);
      // A drops (nothing in flight); p1 is disconnected
      ts[0].drop(false, 1);
      await until(net, () => eds[0].state === "reconnecting");
      ts[0].failConnects = 1;
      type(eds[0], eds[0].view.state.doc.length - 1, "K");
      server.serverEdit([{ start: 0, end: 0, insert: "O " }]);
      eds[1].undo();
      // p1 reconnects as B: "K" goes out on B
      await until(net, () => eds[0].state === "live");
      const kOp = (ts[0].sent.filter((f) => f.type === "edit").at(-1) as { op: number }).op;
      // B drops half-open with "K" in flight: it lands under B's ID, its ack is lost
      ts[0].drop(true, 1);
      await settle(net);
      return { server, eds, ts, rejected, kOp };
    }

    /** The pre-4b3816c editor's kept frame: an edit handed over while the socket is known closed
     * is held and sent right after the next welcome, outside LiveText. */
    function keptOutside(t: FakeLiveTransport): LiveTransport {
      let open = false;
      let held: ClientFrame | null = null;
      t.onClose(() => (open = false));
      return {
        connect: async (o) => {
          const w = await t.connect(o);
          open = true;
          if (held) t.send(held);
          held = null;
          return w;
        },
        send: (f) => {
          if (!open && f.type === "edit") held = f;
          else t.send(f);
        },
        onFrame: (cb) => t.onFrame(cb),
        onClose: (cb) => t.onClose(cb),
        close: () => t.close(),
      };
    }

    for (const rule of [false, true]) {
      it(`${rule ? "with" : "without"} the host rule: "K" is recognised as ours, no MPC-E000, nothing lost`, async () => {
        const { server, eds, rejected, kOp } = await script(rule, false);
        expect(server.appliedOps.has(`p1:${kOp}`)).toBe(true);
        expect(rejected.filter((r) => r.error?.id === "MPC-E000")).toEqual([]);
        expect(server.text.split("K").length - 1).toBe(1);
        for (const h of eds) expect(doc(h)).toBe(server.text);
        expect(eds[0].pending).toBe(0);
      });
    }

    it("planted fault (the frame kept outside LiveText): a false MPC-E000", async () => {
      const spy = vi.spyOn(LiveText.prototype, "disconnected").mockImplementation(() => {});
      try {
        const { server, rejected, kOp } = await script(true, true);
        expect(server.appliedOps.has(`p1:${kOp}`)).toBe(true);
        expect(rejected.some((r) => r.error?.id === "MPC-E000"), "the fault went unnoticed").toBe(true);
      } finally {
        spy.mockRestore();
      }
    });
  });

  describe("the reconnect race: an edit on the old socket landing after the resync", () => {
    /**
     * The script: (1) p1's edit "X" is in flight on connection A; (2) A's socket drops half-open;
     * (3) p1 reconnects as B with prev=A; (4) the host answers B's since without "X"; (5) A's
     * socket delivers "X" late (40 units after it was sent). `reapplyFirst`: the person presses
     * Re-apply between (4) and (5).
     */
    async function race(rule: boolean, opts: { reapplyFirst?: boolean; between?: (eds: LiveEditorHandle[]) => void } = {}) {
      const net = new Net();
      const server = new FakeServer(net, "abc\n");
      server.leaveOldOnReconnect = rule;
      server.noRuleLeave = "keepalive"; // without the rule: the half-open socket stays read
      let lat = 1;
      const host = new FakeLiveHost(net, server, () => lat);
      const eds: LiveEditorHandle[] = [];
      const ts: FakeLiveTransport[] = [];
      const ev = { rejected: [] as Array<LiveEditorEvents["rejected"]>, landed: [] as Array<LiveEditorEvents["landed"]>, dup: [] as Array<LiveEditorEvents["duplicateRisk"]> };
      for (const p of ["p1", "p2"]) {
        const el = document.createElement("div");
        document.body.append(el);
        const t = host.transport(p);
        const h = mountLiveEditor(el, { transport: t, timers: netTimers(net, () => 0.5), reconnect: { minMs: 5, maxMs: 5 } });
        if (p === "p1") {
          h.on("rejected", (r) => ev.rejected.push(r));
          h.on("landed", (l) => ev.landed.push(l));
          h.on("duplicateRisk", (d) => ev.dup.push(d));
        }
        eds.push(h);
        ts.push(t);
      }
      await settle(net);
      lat = 40;
      type(eds[0], 3, "X"); // (1)
      lat = 1;
      const op = (edits(ts[0]).at(-1) as { op: number }).op;
      ts[0].drop(true, 1); // (2)
      // (3), (4): reconnected and refused as not applied, before "X" arrives
      await until(net, () => ev.rejected.length > 0);
      expect(ts[0].prevs.at(-1)).toBeGreaterThan(0);
      const reapplied = opts.reapplyFirst ? eds[0].reapply(ev.rejected[0].id) : null;
      opts.between?.(eds);
      await settle(net); // (5)
      return { net, server, eds, ts, ev, op, reapplied };
    }
    const count = (t: string) => t.split("X").length - 1;

    it("without the host rule: the late edit is recognised by its client ID; Re-apply does nothing", async () => {
      const { net, server, eds, ev, op } = await race(false);
      expect(server.appliedOps.has(`p1:${op}`)).toBe(true); // it landed, after the resync
      expect(ev.rejected.length).toBe(1);
      expect(ev.rejected[0].error?.id).toBe("MPC-E000");
      expect(ev.rejected[0].mayHaveApplied).toBe(true);
      expect(ev.landed.map((l) => l.rejection.id)).toEqual([ev.rejected[0].id]);
      expect(eds[0].rejected).toEqual([]); // it left the list
      const frame = eds[0].view.dom.closest(".mpc-editor")!;
      expect((frame.querySelector(".mpc-note") as HTMLElement).textContent).toBe("A change that seemed lost went through after all.");
      expect(eds[0].reapply(ev.rejected[0].id)).toEqual({ status: "already-present" });
      await settle(net);
      for (const t of [server.text, doc(eds[0]), doc(eds[1])]) expect(count(t)).toBe(1);
      expect(ev.dup).toEqual([]);
    });

    it("with the host rule: the late frame is dropped unapplied and counted; Re-apply puts the text in once", async () => {
      const { net, server, eds, ev, op, ts } = await race(true);
      expect(ts[0].prevs.slice(1).every((p) => typeof p === "number")).toBe(true); // prev on every reconnect
      expect(server.lateDiscarded).toBe(1);
      expect(server.appliedOps.has(`p1:${op}`)).toBe(false);
      expect(server.text).toBe("abc\n");
      // a true MPC-E000 (it never landed), still marked: the client can't know which host it has
      expect(ev.rejected.length).toBe(1);
      expect(ev.rejected[0].mayHaveApplied).toBe(true);
      expect(ev.landed).toEqual([]);
      expect(eds[0].reapply(ev.rejected[0].id)).toEqual({ status: "applied" });
      await settle(net);
      expect(server.text).toBe("abcX\n");
      for (const t of [doc(eds[0]), doc(eds[1])]) expect(t).toBe(server.text);
    });

    it("without the host rule, Re-apply before the late edit lands: the duplicate-risk notice, with both versions", async () => {
      const { server, eds, ev, reapplied } = await race(false, { reapplyFirst: true });
      expect(reapplied).toEqual({ status: "applied" }); // absent when pressed
      expect(count(server.text)).toBe(2); // the one case only a host following the rule prevents
      expect(ev.dup.length).toBe(1);
      expect(ev.dup[0].rejection.id).toBe(ev.rejected[0].id);
      expect(ev.dup[0].landed).toBeGreaterThan(ev.dup[0].reapplied);
      const frame = eds[0].view.dom.closest(".mpc-editor")!;
      expect((frame.querySelector(".mpc-note") as HTMLElement).textContent).toBe(
        "A change you re-applied had also gone through: it may now be in the document twice. Check it.",
      );
    });

    it("planted fault (presence out), without the host rule: the tie guard still refuses (the late edit went in at its very place)", async () => {
      await withoutPresenceCheck(async () => {
        const { net, server, eds, ev } = await race(false);
        expect(ev.landed).toEqual([]);
        expect(eds[0].reapply(ev.rejected[0].id)).toEqual({ status: "unknown", reason: "tie" });
        await settle(net);
        expect(count(server.text)).toBe(1);
      });
    });

    it("planted fault (presence and the tie guard out), without the host rule: Re-apply doubles the text", async () => {
      await withoutPresenceCheck(() =>
        withoutReapplyGuard("tie", async () => {
          const { net, server, eds, ev } = await race(false);
          expect(ev.landed).toEqual([]);
          expect(eds[0].reapply(ev.rejected[0].id)).toEqual({ status: "applied" });
          await settle(net);
          expect(count(server.text), "the fault went unnoticed").toBe(2);
        }),
      );
    });

    for (const rule of [false, true]) {
      it(`${rule ? "with" : "without"} the host rule: someone else's identical change after the resync is never taken for ours`, async () => {
        // p2 types the very same text at the same place, after p1's resync, before "X" lands
        const { net, server, eds, ev } = await race(rule, { between: (eds) => type(eds[1], 3, "X") });
        const id = ev.rejected[0].id;
        const p2X = [...server.appliedAt.entries()].find(([k]) => k.startsWith("p2:"))![1];
        expect(ev.landed.every((l) => l.version !== p2X), "p2's change taken for p1's").toBe(true);
        if (!rule) {
          // p1's own "X" landed later, recognised by its client ID: two X, one each
          expect(ev.landed.length).toBe(1);
          expect(eds[0].reapply(id)).toEqual({ status: "already-present" });
        } else {
          expect(ev.landed).toEqual([]);
          expect(eds[0].rejected.map((r) => r.id)).toEqual([id]); // still there
          // p2's "X" went in at its very place: the server would put p1's after it, Re-apply
          // before; refused (a tie), kept
          expect(eds[0].reapply(id)).toEqual({ status: "unknown", reason: "tie" });
          expect(eds[0].rejected.map((r) => r.id)).toEqual([id]);
        }
        await settle(net);
        expect(count(server.text)).toBe(rule ? 1 : 2);
        for (const h of eds) expect(doc(h)).toBe(server.text);
      });
    }

    it("an earlier acknowledged edit from the same client ID doesn't count; a lost two-hunk edit is refused, a single-hunk one applies once", async () => {
      const { net, server, eds, ts, logs } = setup("abc\n");
      await settle(net);
      type(eds[0], 0, "a"); // acknowledged on this connection
      await settle(net);
      eds[0].view.dispatch({ changes: [{ from: 1, insert: "Q" }, { from: 4, insert: "Z" }], userEvent: "input.type" });
      ts[0].drop(false); // lost with the socket
      await settle(net);
      const rej = logs[0].rejected[0];
      expect(rej.mayHaveApplied).toBe(true);
      expect(server.text).toBe("aabc\n");
      // two hunks: refused rather than put back in one place; the text unchanged, still listed
      expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "multi-hunk" });
      const frame = eds[0].view.dom.closest(".mpc-editor")!;
      expect((frame.querySelector(".mpc-note") as HTMLElement).textContent).toBe(
        "This change touched more than one place: nothing was re-applied. Check it, then type what's missing.",
      );
      expect(eds[0].rejected.map((r) => r.id)).toEqual([rej.id]);
      await settle(net);
      expect(server.text).toBe("aabc\n");
      expect(doc(eds[0])).toBe(server.text);
      // on the new connection: "T" acknowledged, then "S" lost; the earlier acked edit from the
      // same client ID doesn't count: absent, and Re-apply puts "S" in once
      type(eds[0], 0, "T");
      await settle(net);
      type(eds[0], 5, "S");
      ts[0].drop(false);
      await settle(net);
      const one = logs[0].rejected.find((r) => r.text === "S")!;
      expect(one.mayHaveApplied).toBe(true);
      expect(server.text).toBe("Taabc\n");
      expect(eds[0].reapply(one.id)).toEqual({ status: "applied" });
      await settle(net);
      expect(server.text).toBe("TaabcS\n");
      expect(doc(eds[0])).toBe(server.text);
      // an ordinary rejection keeps today's Re-apply (no check)
      eds[0].view.dispatch({ changes: { from: 2, to: 5, insert: "XYZ" }, userEvent: "input.type" });
      server.serverEdit([{ start: 2, end: 5, insert: "abd" }]);
      await settle(net);
      const conflict = logs[0].rejected.find((r) => r.error?.id === "MPC-E111")!;
      expect(conflict.mayHaveApplied).toBe(false);
      // a replacement: Re-apply would only re-insert its text beside what it replaced
      expect(eds[0].reapply(conflict.id)).toEqual({ status: "unknown", reason: "not-insertion" });
    });

    it("an undo unanswered on the lost connection: the client can't tell, so Re-apply refuses", async () => {
      const { net, server, eds, ts, logs } = setup("abc\n", 1);
      server.leaveOldOnReconnect = false;
      server.noRuleLeave = "keepalive";
      await settle(net);
      type(eds[0], 0, "u");
      await settle(net);
      // an edit and an undo in flight, then the socket goes half-open with both arriving late
      type(eds[0], 4, "X");
      eds[0].undo();
      ts[0].drop(true, 1, 40);
      await settle(net);
      const rej = logs[0].rejected.find((r) => r.error?.id === "MPC-E000")!;
      expect(rej.asOf?.undoInFlight).toBe(true);
      expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "cant-tell" });
      const frame = eds[0].view.dom.closest(".mpc-editor")!;
      expect((frame.querySelector(".mpc-note") as HTMLElement).textContent).toContain("Can't tell");
      expect(doc(eds[0])).toBe(server.text);
    });

    it("prev: only the same login's connection on this doc is left; anything else is ignored silently", async () => {
      const net = new Net();
      const server = new FakeServer(net, "doc one\n");
      const other = new FakeServer(net, "doc two\n");
      const host = new FakeLiveHost(net, server, () => 1);
      const host2 = new FakeLiveHost(net, other, () => 1, 500);
      const signal = new AbortController().signal;
      const connect = async (t: FakeLiveTransport, prev?: number) => {
        const p = t.connect({ signal, ...(prev !== undefined ? { prev } : {}) });
        await settle(net);
        return p;
      };
      const a = host.transport("p1");
      const wa = await connect(a);
      const aConn = a.conn!;
      let aClosed = 0;
      a.onClose(() => aClosed++);
      // another login's prev: ignored
      const b = host.transport("p2");
      const wb = await connect(b, wa.client);
      // an unknown prev: ignored
      const c = host.transport("p1");
      const wc = await connect(c, 99_999);
      // a prev from another doc (the same login's connection there): ignored here
      const d2 = host2.transport("p1");
      const wd2 = await connect(d2);
      const e = host.transport("p1");
      const we = await connect(e, wd2.client);
      const reply = (w: typeof wa) => ({ root: w.root, version: w.version, text: w.text, hasText: "text" in w, rights: w.rights });
      for (const w of [wb, wc, we]) expect(reply(w)).toEqual(reply(wa)); // the reply is no different
      expect(server.has(aConn)).toBe(true);
      expect(other.has(d2.conn!)).toBe(true);
      expect(aClosed).toBe(0);
      expect(server.lateDiscarded).toBe(0);
      // the same login's prev on this doc: that connection is left
      const f = host.transport("p1");
      await connect(f, wa.client);
      expect(server.has(aConn)).toBe(false);
      // without prev, a second connection from the same login is a second tab: both live
      expect(server.has(c.conn!)).toBe(true);
      expect(server.has(f.conn!)).toBe(true);
    });
  });

  it("MPC-E112 on the resync: reload by snapshot", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n", 1);
    await settle(net);
    server.sinceWindow = 1;
    ts[0].resumeAlways = true;
    ts[0].failConnects = 1;
    ts[0].drop();
    for (let i = 0; i < 4; i++) server.serverEdit([{ start: 0, end: 0, insert: `${i}` }]);
    await settle(net);
    expect(logs[0].reload.length).toBeGreaterThanOrEqual(1);
    expect(ts[0].sent.some((f) => f.type === "snapshot")).toBe(true);
    expect(doc(eds[0])).toBe(server.text);
    expect(eds[0].version).toBe(server.version);
    type(eds[0], 0, "Z");
    await settle(net);
    expect(server.text.startsWith("Z3210")).toBe(true);
  });

  it("a refused snapshot falls back to a fresh connect (no resume)", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n", 1);
    await settle(net);
    server.sinceWindow = 1;
    server.refuseSnapshots = true;
    ts[0].resumeAlways = true;
    ts[0].drop();
    for (let i = 0; i < 4; i++) server.serverEdit([{ start: 0, end: 0, insert: `${i}` }]);
    // after the fallback the host can't resume either: it sends the text
    await until(net, () => ts[0].sent.some((f) => f.type === "snapshot"));
    ts[0].resumeAlways = false;
    await settle(net);
    expect(ts[0].resumes[ts[0].resumes.length - 1]).toBeUndefined();
    expect(doc(eds[0])).toBe(server.text);
    expect(eds[0].state).toBe("live");
    expect(logs[0].reload.length).toBeGreaterThanOrEqual(1);
  });

  it("closed by the host: the closed event, a banner, read-only, and no reconnect", async () => {
    const { net, eds, ts, logs } = setup("abc\n");
    await settle(net);
    ts[0].closeWith("deleted");
    await settle(net);
    expect(eds[0].state).toBe("closed");
    expect(logs[0].closed.map((c) => c.reason)).toEqual(["deleted"]);
    expect(eds[0].view.state.readOnly).toBe(true);
    const banner = eds[0].view.dom.closest(".mpc-editor")!.querySelector(".mpc-banner") as HTMLElement;
    expect(banner.hidden).toBe(false);
    expect(banner.textContent).toBe("This document was deleted.");
    expect(ts[0].connects).toBe(1);
    eds[0].reconnectNow();
    await settle(net);
    expect(ts[0].connects).toBe(1);
  });

  it("a welcome too big for one frame: closed (declined, MPC-E401), no retry", async () => {
    const net = new Net();
    const server = new FakeServer(net, "x".repeat(5000));
    server.frameMax = 1024;
    const el = document.createElement("div");
    document.body.append(el);
    const t = server.live("p1", () => 1); // the wire contract's own fake transport
    const h = mountLiveEditor(el, { transport: t, timers: netTimers(net, rng(3).next), reconnect: { minMs: 5, maxMs: 40 } });
    const closed: Array<{ reason: string; error?: { id: string } }> = [];
    h.on("closed", (c) => closed.push(c));
    await settle(net);
    expect(h.state).toBe("closed");
    expect(closed.length).toBe(1);
    expect(closed[0].reason).toBe("declined");
    expect(closed[0].error?.id).toBe("MPC-E401");
    expect(h.view.state.readOnly).toBe(true);
    const banner = el.querySelector(".mpc-banner") as HTMLElement;
    expect(banner.textContent).toBe("This document can't be edited live.");
  });

  it("a connect refused for good (auth) closes; reconnect false closes on a drop", async () => {
    const a = setup("abc\n");
    a.ts[0].refuseWith = { reason: "auth", retry: false };
    await settle(a.net);
    expect(a.eds[0].state).toBe("closed");
    expect(a.logs[0].closed[0].reason).toBe("auth");

    const b = setup("abc\n", 1, { reconnect: false });
    await settle(b.net);
    b.ts[0].drop();
    await settle(b.net);
    expect(b.eds[0].state).toBe("closed");
    expect(b.ts[0].connects).toBe(1);
  });

  describe("whether to retry is the close table's, not the transport's", () => {
    // a transport that claims `retry: true` for `auth` (say, its pre-check after a failed dial)
    const auth = { reason: "auth", retry: true } as const;
    const retryErrors = (log: Log) => log.error.filter((e) => e.id === CLOSE_RETRY_ERROR);

    it("a connect refused as auth with retry: true closes, no retry, one error", async () => {
      const a = setup("abc\n");
      a.ts[0].refuseWith = { ...auth };
      await settle(a.net);
      expect(a.eds[0].state).toBe("closed");
      expect(a.ts[0].connects).toBe(1);
      expect(a.logs[0].closed.map((c) => [c.reason, c.retry])).toEqual([["auth", false]]);
      expect(retryErrors(a.logs[0]).length).toBe(1);
    });

    it("a socket the transport ends as auth with retry: true closes, no retry, one error", async () => {
      const a = setup("abc\n");
      await settle(a.net);
      expect(a.eds[0].state).toBe("live");
      a.ts[0].endAs({ ...auth });
      await settle(a.net);
      expect(a.eds[0].state).toBe("closed");
      expect(a.ts[0].connects).toBe(1);
      expect(a.logs[0].closed.map((c) => [c.reason, c.retry])).toEqual([["auth", false]]);
      expect(retryErrors(a.logs[0]).length).toBe(1);
    });

    it("the other way round: network with retry: false still reconnects (one error)", async () => {
      const a = setup("abc\n");
      await settle(a.net);
      a.ts[0].endAs({ reason: "network", retry: false });
      await settle(a.net);
      expect(a.eds[0].state).toBe("live");
      expect(a.ts[0].connects).toBe(2);
      expect(a.logs[0].closed).toEqual([]);
      expect(retryErrors(a.logs[0]).length).toBe(1);
    });

    it("planted fault (the editor trusts the transport's flag): auth with retry: true retries", async () => {
      const a = setup("abc\n");
      const proto = Object.getPrototypeOf(a.eds[0]) as { retryOf(e: unknown): unknown };
      const spy = vi.spyOn(proto, "retryOf").mockImplementation((e) => e);
      try {
        a.ts[0].refuseWith = { ...auth };
        // it never stops trying, so the network never settles: run a bounded number of steps
        await settle(a.net, undefined, 400).catch(() => {});
        expect(a.ts[0].connects, "the fault went unnoticed").toBeGreaterThan(1);
        expect(a.eds[0].state).not.toBe("closed");
      } finally {
        spy.mockRestore();
        a.eds[0].destroy();
      }
    });

    it("a closed frame's code and path reach the host; code never changes the retry", async () => {
      const a = setup("abc\n", 2);
      await settle(a.net);
      a.ts[0].closeWith("closed", { code: "core_off" });
      a.ts[1].closeWith("moved", { path: "notes/renamed.md" });
      await settle(a.net);
      expect(a.logs[0].closed).toEqual([{ reason: "closed", retry: false, code: "core_off" }]);
      expect(a.logs[1].closed).toEqual([{ reason: "moved", retry: false, path: "notes/renamed.md" }]);
      expect(a.ts[0].connects + a.ts[1].connects).toBe(2);
      expect(retryErrors(a.logs[0]).length + retryErrors(a.logs[1]).length).toBe(0);
    });
  });

  it("refuses to mount with a host's CodeMirror history (the server owns undo)", () => {
    const net = new Net();
    const host = new FakeLiveHost(net, new FakeServer(net, "abc\n"), () => 1);
    const el = document.createElement("div");
    document.body.append(el);
    let err: unknown = null;
    try {
      mountLiveEditor(el, { transport: host.transport("p1"), extensions: [history(), keymap.of(historyKeymap)] });
    } catch (e) {
      err = e;
    }
    expect(err).toBeInstanceOf(LiveEditorError);
    expect((err as LiveEditorError).code).toBe(HISTORY_ERROR);
    expect(HISTORY_ERROR).toBe("MPC-EDITOR-HISTORY");
    expect(el.childElementCount).toBe(0); // nothing mounted
  });

  it("a history added later: an error once; undo keys go to the server; its commands are the person's own edits", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n");
    await settle(net);
    const [h] = eds;
    h.view.dispatch({ effects: StateEffect.appendConfig.of([history(), keymap.of(historyKeymap)]) });
    h.view.dispatch({ effects: StateEffect.appendConfig.of([]) }); // reconfigured again: no second error
    expect(logs[0].error.map((e) => e.id)).toEqual([HISTORY_ERROR]);
    type(h, 3, "XYZ");
    await settle(net);
    expect(server.text).toBe("abcXYZ\n");
    h.view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "z", keyCode: 90, ctrlKey: true, bubbles: true, cancelable: true }));
    expect(doc(h)).toBe("abcXYZ\n"); // the key went to the server, ahead of the history's keymap
    expect(ts[0].sent.filter((f) => f.type === "undo").length).toBe(1);
    await settle(net);
    expect(server.text).toBe("abc\n");
    expect(doc(h)).toBe(server.text);
    // the history's command, called directly, is the person's own edit, sent like typing
    type(h, 3, "Q");
    await settle(net);
    expect(server.text).toBe("abcQ\n");
    cmUndo(h.view);
    expect(doc(h)).toBe("abc\n");
    expect(h.text).toBe("abc\n");
    await settle(net);
    expect(server.text).toBe("abc\n");
    expect(ts[0].sent.filter((f) => f.type === "undo").length).toBe(1);
    // remote changes never enter the history: undoing locally can't take them out
    server.serverEdit([{ start: 0, end: 0, insert: "R" }]);
    await settle(net);
    cmUndo(h.view);
    cmUndo(h.view);
    await settle(net);
    expect(server.text.startsWith("R")).toBe(true);
    expect(doc(h)).toBe(server.text);
    expect(h.text).toBe(server.text);
    expect(logs[0].error.map((e) => e.id)).toEqual([HISTORY_ERROR]);
  });

  it("an identical change by someone else is never taken for its own (a resync after a drop)", async () => {
    const { net, server, eds, ts, logs } = setup("hello\n", 2);
    await settle(net);
    // the host's own view of the frames: real client IDs only
    const seenClients = new Set<number>();
    ts[0].onFrame((f) => {
      if (f.type === "change") seenClients.add(f.event.client);
      if (f.type === "events") for (const ev of f.events) seenClients.add(ev.client);
    });
    const [a, b] = eds;
    // A replaces "h"; its frame is lost with the socket; B makes the very same change
    a.view.dispatch({ changes: { from: 0, to: 1, insert: "J" }, userEvent: "input.type" });
    const aOp = (edits(ts[0])[0] as { op: number }).op;
    ts[0].drop(false);
    b.view.dispatch({ changes: { from: 0, to: 1, insert: "J" }, userEvent: "input.type" });
    await settle(net);
    expect(server.text).toBe("Jello\n");
    expect(server.appliedOps.has(`p1:${aOp}`)).toBe(false);
    // A's change wasn't applied: never acked as if it were, handed back instead
    expect(logs[0].ack).toEqual([]);
    expect(logs[0].rejected.length).toBe(1);
    expect(logs[0].rejected[0].text).toBe("J");
    expect(doc(a)).toBe(server.text);
    expect(doc(b)).toBe(server.text);
    // what the host saw kept the real IDs; the change's author is B
    expect([...seenClients].every((c) => c > 0)).toBe(true);
    expect(seenClients.size).toBeGreaterThan(0);
    expect(logs[0].change.some((c) => !c.local && c.by === "p2")).toBe(true);
  });

  it("LiveText only ever reads the server's real client IDs (no stand-in for its own)", async () => {
    // LiveText's frame handler, spied: what the replica reads, against what the host sent
    const spy = vi.spyOn(LiveText.prototype as unknown as { onFrame(f: ServerFrame): void }, "onFrame");
    try {
      const { net, server, eds, ts, logs } = setup("hello\n", 2);
      await settle(net);
      const raw = new Set<number>();
      for (const t of ts)
        t.onFrame((f) => {
          if (f.type === "change") raw.add(f.event.client);
          if (f.type === "events") for (const ev of f.events) raw.add(ev.client);
        });
      // the seed-51 shape: a lost edit, an identical change by someone else, a resync
      eds[0].view.dispatch({ changes: { from: 0, to: 1, insert: "J" }, userEvent: "input.type" });
      ts[0].drop(false);
      eds[1].view.dispatch({ changes: { from: 0, to: 1, insert: "J" }, userEvent: "input.type" });
      type(eds[1], 2, "x");
      await settle(net);
      type(eds[0], 0, "A");
      await settle(net);
      const read: number[] = [];
      for (const [f] of spy.mock.calls) {
        if (f.type === "change") read.push(f.event.client);
        if (f.type === "events") read.push(...f.events.map((ev) => ev.client));
      }
      expect(read.length).toBeGreaterThan(0);
      expect(spy.mock.calls.some(([f]) => f.type === "events"), "a since reply was read").toBe(true);
      for (const c of read) {
        expect(c, "a client ID LiveText read").toBeGreaterThan(0);
        expect(raw.has(c), `client ${c} isn't one the host sent`).toBe(true);
      }
      expect(logs[0].ack.length).toBeGreaterThan(0);
      expect(doc(eds[0])).toBe(server.text);
      expect(doc(eds[1])).toBe(server.text);
    } finally {
      spy.mockRestore();
    }
  });

  it("hands on removed-text notes for this participant only, once each", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n");
    await settle(net);
    const note = (to: string, change: number): RemovedTextNote => ({
      kind: "overwritten",
      to,
      change,
      version: 41,
      restore_from: 41,
      source: "an outside save",
      rule: "removed_text",
      row: null,
      line: 2,
      cells: [{ region: "L2", col: null, yours: "sam's line\n", file: "", cut: false }],
      more: 0,
      not_shown: 0,
      withheld: false,
      no_history: false,
      message: 'An outside save may have removed your text (L2). It read "sam\'s line\\n".',
    });
    const send = (ev: unknown) => server.sendTo(ts[0].conn!, { type: "roster_event", event: ev as never });
    send(note("p1", 8));
    send(note("p1", 8)); // the same note again
    send(note("p2", 9)); // someone else's
    send({ kind: "snapshot", version: server.version, members: [], overwritten: [note("p1", 8), note("p1", 10)] });
    send({ kind: "claim_started", who: "p2", claim: "c1", lines: { first: 1, last: 1 }, version: server.version });
    await settle(net);
    expect(logs[0].note.map((n) => n.change)).toEqual([8, 10]);
    expect(logs[0].note[0].message).toBe('An outside save may have removed your text (L2). It read "sam\'s line\\n".');
    expect(logs[0].note[0].rule).toBe("removed_text");
    expect(logs[0].claim.map((c) => c.kind)).toEqual(["claim_started"]);
    expect(eds[0].state).toBe("live");
  });

  it("holds remote changes during an IME composition: nothing lost or doubled, and all converge", async () => {
    const { net, server, eds, logs } = setup("alpha beta\n", 2);
    await settle(net);
    const [a, b] = eds;
    const dom = a.view.contentDOM;
    a.view.dispatch({ selection: { anchor: 10 } });
    dom.dispatchEvent(new CompositionEvent("compositionstart", { data: "" }));
    a.view.dispatch({ changes: { from: 10, insert: "に" }, selection: { anchor: 11 }, userEvent: "input.type.compose" });
    // remote changes arrive mid-composition, before it: another editor types, an outside edit
    type(b, 0, "B: ");
    await settle(net);
    server.serverEdit([{ start: 0, end: 0, insert: "S " }]);
    await settle(net);
    // held: the editor still shows the composition's text as the person left it
    expect(doc(a)).toBe("alpha betaに\n");
    expect(a.text).not.toBe(doc(a));
    a.view.dispatch({ changes: { from: 10, to: 11, insert: "日本" }, selection: { anchor: 12 }, userEvent: "input.type.compose" });
    await settle(net);
    expect(doc(a)).toBe("alpha beta日本\n");
    dom.dispatchEvent(new CompositionEvent("compositionend", { data: "日本" }));
    await new Promise((r) => setTimeout(r, 20));
    await settle(net);
    expect(server.text).toBe("S B: alpha beta日本\n");
    expect(doc(a)).toBe(server.text);
    expect(a.text).toBe(server.text);
    expect(doc(b)).toBe(server.text);
    expect(server.text.split("日本").length - 1).toBe(1);
    expect(logs[0].rejected).toEqual([]);
    expect(logs[0].error).toEqual([]);
    expect(readableOn("#ffffff")).toBe("#000000");
  });
});

// Single-hunk Re-apply puts the typed text back at the region's start: the server's place for it
// only for a pure insertion, placed by the server's changes, with nothing inserted at that point
// since. Each guard below refuses one case where M1 put the wrong text in, and each has a
// planted fault (that guard out) showing the old result.
describe("Re-apply's guards (single hunk)", () => {
  const withoutGuard = withoutReapplyGuard;
  const note = (h: LiveEditorHandle) => (h.view.dom.closest(".mpc-editor")!.querySelector(".mpc-note") as HTMLElement).textContent;

  /** A change lost with its socket (`MPC-E000`, found absent): `change` on "abc\n". */
  async function lost(change: { from: number; to?: number; insert?: string }, n = 1) {
    const s = setup("abc\n", n);
    await settle(s.net);
    s.eds[0].view.dispatch({ changes: change, userEvent: change.insert ? "input.type" : "delete" });
    s.ts[0].drop(false);
    await settle(s.net);
    const rej = s.logs[0].rejected.at(-1)!;
    expect(rej.error?.id).toBe("MPC-E000");
    expect(s.eds[0].rejected.map((r) => r.id)).toEqual([rej.id]);
    return { ...s, rej };
  }

  it("a pure insertion still applies, once", async () => {
    const { net, server, eds, rej } = await lost({ from: 1, insert: "Q" });
    expect(eds[0].reapply(rej.id)).toEqual({ status: "applied" });
    await settle(net);
    expect(server.text).toBe("aQbc\n");
    expect(doc(eds[0])).toBe(server.text);
    expect(eds[0].rejected).toEqual([]);
  });

  it("a deletion: refused (M1 said applied and changed nothing)", async () => {
    const { net, server, eds, rej } = await lost({ from: 1, to: 2 });
    expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "not-insertion" });
    expect(note(eds[0])).toBe(DEFAULT_STRINGS.notInsertion);
    expect(eds[0].rejected.map((r) => r.id)).toEqual([rej.id]); // kept
    await settle(net);
    expect(server.text).toBe("abc\n");
  });

  it("planted fault (not-insertion out): a deletion says applied and changes nothing", async () => {
    await withoutGuard("not-insertion", async () => {
      const { net, server, eds, rej } = await lost({ from: 1, to: 2 });
      expect(eds[0].reapply(rej.id), "the fault went unnoticed").toEqual({ status: "applied" });
      await settle(net);
      expect(server.text, "the fault went unnoticed").toBe("abc\n"); // "b" never deleted again
    });
  });

  it("a replacement: refused (M1 put the new text beside the old)", async () => {
    const { net, server, eds, rej } = await lost({ from: 1, to: 2, insert: "B" });
    expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "not-insertion" });
    await settle(net);
    expect(server.text).toBe("abc\n");
  });

  it("planted fault (not-insertion out): a replacement's text goes in beside the old", async () => {
    await withoutGuard("not-insertion", async () => {
      const { net, server, eds, rej } = await lost({ from: 1, to: 2, insert: "B" });
      expect(eds[0].reapply(rej.id)).toEqual({ status: "applied" });
      await settle(net);
      expect(server.text, "the fault went unnoticed").toBe("aBbc\n");
    });
  });

  /** A lost "X" at 3, then p2 types "Y" at that very place. */
  async function tie() {
    const s = await lost({ from: 3, insert: "X" }, 2);
    type(s.eds[1], 3, "Y");
    await settle(s.net);
    expect(s.server.text).toBe("abcY\n");
    return s;
  }

  it("a tie (something inserted at its place since): refused (M1 put it before; the server would put it after)", async () => {
    const { net, server, eds, rej } = await tie();
    expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "tie" });
    expect(note(eds[0])).toBe(DEFAULT_STRINGS.tie);
    expect(eds[0].rejected.map((r) => r.id)).toEqual([rej.id]);
    await settle(net);
    expect(server.text).toBe("abcY\n");
  });

  it("text around its place deleted since: refused (M1 put it at the deletion's start)", async () => {
    const { net, server, eds, rej } = await deletedAround();
    expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "deleted-around" });
    expect(note(eds[0])).toBe(DEFAULT_STRINGS.deletedAround);
    await settle(net);
    expect(server.text).toBe("ad\n");
  });

  it("planted fault (deleted-around out): the text goes in where the deletion began", async () => {
    await withoutGuard("deleted-around", async () => {
      const { net, server, eds, rej } = await deletedAround();
      expect(eds[0].reapply(rej.id)).toEqual({ status: "applied" });
      await settle(net);
      // the server's rule refuses it: an insertion strictly inside a range deleted since
      expect("conflict" in rebaseOurs([{ start: 2, end: 2, insert: "X" }], [[{ start: 1, end: 3, insert: "" }]])).toBe(true);
      expect(server.text, "the fault went unnoticed").toBe("aXd\n");
    });
  });

  it("planted fault (tie out): the text goes in before the other insertion", async () => {
    await withoutGuard("tie", async () => {
      const { net, server, eds, rej } = await tie();
      expect(eds[0].reapply(rej.id)).toEqual({ status: "applied" });
      await settle(net);
      // the server's rule puts the earlier change after the one inserted at its place since
      const sent = rebaseOurs([{ start: 3, end: 3, insert: "X" }], [[{ start: 3, end: 3, insert: "Y" }]]);
      expect(sent).toEqual({ ok: [{ start: 4, end: 4, insert: "X" }] });
      expect(server.text, "the fault went unnoticed").toBe("abcXY\n");
    });
  });

  /** A lost "X" at 2 in "abcd\n", then p2 deletes "bc" around it. */
  async function deletedAround() {
    const s = setup("abcd\n", 2);
    await settle(s.net);
    type(s.eds[0], 2, "X");
    s.ts[0].drop(false);
    await settle(s.net);
    const rej = s.logs[0].rejected.at(-1)!;
    expect(rej.error?.id).toBe("MPC-E000");
    s.eds[1].view.dispatch({ changes: { from: 1, to: 3 }, userEvent: "delete" });
    await settle(s.net);
    expect(s.server.text).toBe("ad\n");
    return { ...s, rej };
  }

  /** A pending "Y" at 3 refused by an MPC-E112 (its base left the window): the editor reloads. */
  async function reloaded() {
    const s = setup("abc\n", 1);
    await settle(s.net);
    s.server.window = 0;
    type(s.eds[0], 3, "Y");
    s.server.serverEdit([{ start: 0, end: 0, insert: "0" }]); // before the edit arrives
    await settle(s.net);
    expect(s.logs[0].reload.length).toBeGreaterThanOrEqual(1);
    expect(s.server.text).toBe("0abc\n");
    expect(doc(s.eds[0])).toBe(s.server.text);
    const rej = s.logs[0].rejected.find((r) => r.text === "Y")!;
    expect(rej.error?.id).toBe("MPC-E112");
    return { ...s, rej };
  }

  it("a reload since: refused (M1 put the text at the start of the document)", async () => {
    const { net, server, eds, rej } = await reloaded();
    expect(eds[0].reapply(rej.id)).toEqual({ status: "unknown", reason: "reloaded" });
    expect(note(eds[0])).toBe(DEFAULT_STRINGS.reloaded);
    await settle(net);
    expect(server.text).toBe("0abc\n");
  });

  it("planted fault (reloaded out): the text goes in at the start of the document", async () => {
    await withoutGuard("reloaded", async () => {
      const { net, server, eds, rej } = await reloaded();
      expect(eds[0].reapply(rej.id)).toEqual({ status: "applied" });
      await settle(net);
      expect(server.text, "the fault went unnoticed").toBe("Y0abc\n");
    });
  });

  it("a deletion someone already made: still already-present, by presence", async () => {
    const { net, server, eds, ts, logs } = setup("abc\n", 1);
    server.leaveOldOnReconnect = false;
    server.noRuleLeave = "keepalive";
    await settle(net);
    eds[0].view.dispatch({ changes: { from: 1, to: 2 }, userEvent: "delete" });
    ts[0].drop(true, 1, 40); // half-open: the deletion lands late, recognised by its client ID
    await settle(net);
    const rej = logs[0].rejected.find((r) => r.error?.id === "MPC-E000")!;
    expect(server.text).toBe("ac\n");
    expect(eds[0].reapply(rej.id)).toEqual({ status: "already-present" });
  });
});
