// @vitest-environment jsdom
import { EditorState } from "@codemirror/state";
import { EditorView } from "@codemirror/view";
import { describe, expect, it } from "vitest";
import { changesToHunks, codemirrorLive, hunksToChanges } from "../src/codemirror";
import { LiveText } from "../src/live";
import type { ClientFrame, ServerFrame, Transport } from "../src/wire";

function wire() {
  const sent: ClientFrame[] = [];
  const ls = new Set<(f: ServerFrame) => void>();
  const t: Transport = { send: (f) => sent.push(f), onFrame: (cb) => (ls.add(cb), () => ls.delete(cb)) };
  return { t, sent, deliver: (f: ServerFrame) => ls.forEach((l) => l(f)) };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

describe("UTF-16 <-> UTF-8 at the editor edge", () => {
  it("turns CodeMirror changes into byte hunks and back", () => {
    const s = EditorState.create({ doc: "aé日🎉b" });
    const tr = s.update({ changes: [{ from: 1, to: 2, insert: "E" }, { from: 5, insert: "!" }] });
    const hs = changesToHunks(s.doc.toString(), tr.changes);
    expect(hs).toEqual([
      { start: 1, end: 3, insert: "E" },
      { start: 10, end: 10, insert: "!" },
    ]);
    expect(hunksToChanges(s.doc.toString(), hs)).toEqual([
      { from: 1, to: 2, insert: "E" },
      { from: 5, to: 5, insert: "!" },
    ]);
  });
});

describe("codemirrorLive", () => {
  it("sends typing as edits, applies remote changes, and shows remote carets", async () => {
    const w = wire();
    const live = new LiveText({ text: "hello", version: 1 }, w.t);
    const view = new EditorView({
      state: EditorState.create({ doc: live.text, extensions: [codemirrorLive(live)] }),
      parent: document.body,
    });
    view.dispatch({ changes: { from: 5, insert: " 日" }, userEvent: "input.type" });
    expect(live.text).toBe("hello 日");
    expect(w.sent[0]).toMatchObject({ type: "edit", base: 1, hunks: [{ start: 5, end: 5, insert: " 日" }] });
    w.deliver({ type: "ack", op: 1, version: 2, hunks: [{ start: 5, end: 5, insert: " 日" }] });
    w.deliver({
      type: "change",
      event: { version: 3, pid: "p2", client: 2, via: "tab", root: "source", hunks: [{ start: 0, end: 0, insert: ">> " }], at_ms: 0 },
    });
    await tick();
    expect(view.state.doc.toString()).toBe(">> hello 日");
    w.deliver({
      type: "roster",
      entries: [
        { handle: "p2", name: "Ada", colour: "#e11", kind: "person", role: "editor", away: false, cursor: { root: "source", anchor: 3, head: 8, version: 3 } },
      ],
    });
    await tick();
    expect(view.dom.querySelector(".cm-mpc-caret-name")?.textContent).toBe("Ada");
    expect(view.dom.querySelector(".cm-mpc-selection")?.textContent).toBe("hello");
    // the editor's undo key goes to the server
    view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "z", keyCode: 90, ctrlKey: true, bubbles: true, cancelable: true }));
    expect(w.sent.some((f) => f.type === "undo")).toBe(true);
    view.destroy();
  });
});
