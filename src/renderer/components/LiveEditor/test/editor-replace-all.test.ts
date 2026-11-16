// @vitest-environment jsdom
// Replace all in a live doc (build-editors.md E3): it's ONE edit of yours, so
// one Ctrl+Z (the server's per-person undo) takes back every replacement and
// nothing typed before it. Replace all marks its transaction with CodeMirror's
// `isolateHistory`; the binding starts a fresh undo group for it, so it never
// composes with the typing queued ahead of it, and the typing after it starts
// its own group again.
//
// The fake server keeps one undo step per edit frame (see
// editor-undo-order.test.ts), so a replace composed into the typing's frame
// would be undone together with the typing.

import { afterEach, describe, expect, it } from "vitest";
import "../../../../../vendor/k2-mpc-base/client/test/jsdom-shims";
import { mountLiveEditor, type LiveEditorHandle } from "../mount";
import { FakeLiveHost, netTimers, settle, type FakeLiveTransport } from "../../../../../vendor/k2-mpc-base/client/test/fake-live";
import { FakeServer, Net, rng } from "../../../../../vendor/k2-mpc-base/client/test/fake-server";
import { frames } from "./editor-harness";
import { replaceAllSpec } from "@/lib/editor/find/find";

function setup(text: string) {
  const net = new Net();
  const server = new FakeServer(net, text);
  const host = new FakeLiveHost(net, server, () => 1);
  const el = document.createElement("div");
  document.body.append(el);
  const t: FakeLiveTransport = host.transport("p1");
  const timers = netTimers(net, rng(11).next);
  const h = mountLiveEditor(el, { transport: t, timers, schedule: frames(timers), reconnect: { minMs: 5, maxMs: 40 } });
  return { net, server, h, t };
}

const doc = (h: LiveEditorHandle) => h.view.state.doc.toString();
const type = (h: LiveEditorHandle, pos: number, s: string) =>
  h.view.dispatch({ changes: { from: pos, insert: s }, selection: { anchor: pos + s.length }, userEvent: "input.type" });
const ctrlZ = (h: LiveEditorHandle) =>
  h.view.contentDOM.dispatchEvent(new KeyboardEvent("keydown", { key: "z", keyCode: 90, ctrlKey: true, bubbles: true, cancelable: true }));
const spec = { search: "draft", caseSensitive: false, wholeWord: false, regexp: false, replace: "sketch" };
const groups = (t: FakeLiveTransport) => t.sent.flatMap((f) => (f.type === "edit" ? [(f as { group?: string }).group ?? ""] : []));

afterEach(() => {
  document.body.replaceChildren();
});

describe("Replace all in a live doc", () => {
  it("is one edit: one Ctrl+Z takes back every replacement, and the typing queued before it stays", async () => {
    const { net, server, h, t } = setup("a draft, a draft, a draft.");
    await settle(net);
    expect(h.state).toBe("live");
    const end = doc(h).length;
    // typed fast: the first key goes out, the second waits behind it
    type(h, end, " x");
    type(h, end + 2, "y");
    // Replace all at once, while "y" still waits in the queue
    h.view.dispatch(replaceAllSpec(h.view.state, spec)!);
    expect(doc(h)).toBe("a sketch, a sketch, a sketch. xy");
    await settle(net);
    expect(server.text).toBe("a sketch, a sketch, a sketch. xy");
    const g = groups(t);
    expect(new Set(g).size, `the replace has a group of its own: ${JSON.stringify(g)}`).toBeGreaterThanOrEqual(2);
    ctrlZ(h);
    await settle(net);
    expect(server.text, "one Ctrl+Z: every replacement back, the typing kept").toBe("a draft, a draft, a draft. xy");
    expect(doc(h)).toBe(server.text);
  });

  it("typing after Replace all starts its own undo step again", async () => {
    const { net, server, h } = setup("draft draft");
    await settle(net);
    h.view.dispatch(replaceAllSpec(h.view.state, spec)!);
    type(h, doc(h).length, "!");
    await settle(net);
    expect(server.text).toBe("sketch sketch!");
    ctrlZ(h);
    await settle(net);
    expect(server.text, "the typing is undone alone").toBe("sketch sketch");
    ctrlZ(h);
    await settle(net);
    expect(server.text, "then the replace, whole").toBe("draft draft");
  });
});
