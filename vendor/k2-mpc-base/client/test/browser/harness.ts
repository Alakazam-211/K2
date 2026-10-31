// The page the browser tests drive: live editors mounted against the fake server, on a fake
// network the test steps (`settle`), so a remote change arrives exactly when the test says.

import { mountLiveEditor, type LiveEditorHandle } from "../../src/editor";
import type { Hunk } from "../../src/hunk";
import { FakeServer, Net } from "../fake-server";
import { FakeLiveHost, settle, type FakeLiveTransport } from "../fake-live";

export interface Scenario {
  net: Net;
  server: FakeServer;
  editors: LiveEditorHandle[];
  transports: FakeLiveTransport[];
  /** Errors, rejections and reloads every editor reported (a test expects none unless it says). */
  errors: string[];
  rejected: Array<{ editor: number; id: number; text: string; error: string | null }>;
  reloads: string[];
  settle(): Promise<void>;
  /** An outside change on the server (an agent's edit): broadcast to every editor. */
  serverEdit(hunks: Hunk[]): void;
  /** Editor `i` types `text` at UTF-16 position `pos` (a transaction, as typing makes). */
  type(i: number, pos: number, text: string): void;
  state(): { server: string; docs: string[]; texts: string[]; versions: number[]; serverVersion: number; states: string[] };
}

function scenario(text: string, count = 2, opts: { labels?: "always" | "active" | "hover" } = {}): Scenario {
  const net = new Net();
  const server = new FakeServer(net, text);
  const host = new FakeLiveHost(net, server, () => 1);
  const editors: LiveEditorHandle[] = [];
  const transports: FakeLiveTransport[] = [];
  const errors: string[] = [];
  const rejected: Scenario["rejected"] = [];
  const reloads: string[] = [];
  for (let i = 0; i < count; i++) {
    const el = document.createElement("div");
    el.id = `ed${i}`;
    el.style.height = "160px";
    el.style.display = "flex";
    document.body.append(el);
    const t = host.transport(`p${i + 1}`);
    transports.push(t);
    const h = mountLiveEditor(el, { transport: t, labels: opts.labels ?? "always", reconnect: { minMs: 5, maxMs: 20 } });
    h.on("error", (e) => errors.push(`editor ${i}: ${e.id} ${e.message}`));
    h.on("rejected", (r) => rejected.push({ editor: i, id: r.id, text: r.text, error: r.error?.id ?? null }));
    h.on("reload", (r) => reloads.push(`editor ${i}: ${r.reason}`));
    editors.push(h);
  }
  return {
    net,
    server,
    editors,
    transports,
    errors,
    rejected,
    reloads,
    settle: () => settle(net),
    serverEdit: (hunks) => server.serverEdit(hunks),
    type(i, pos, s) {
      const v = editors[i].view;
      v.dispatch({ changes: { from: pos, insert: s }, selection: { anchor: pos + s.length }, userEvent: "input.type" });
    },
    state: () => ({
      server: server.text,
      docs: editors.map((e) => e.view.state.doc.toString()),
      texts: editors.map((e) => e.text),
      versions: editors.map((e) => e.version),
      serverVersion: server.version,
      states: editors.map((e) => e.state),
    }),
  };
}

/**
 * A composition made the way an IME makes one, without an IME: composition events, and the
 * composing text written into the editor's DOM text node at the caret (what the browser does),
 * which CodeMirror reads as it reads a real composition.
 */
function syntheticComposition(view: LiveEditorHandle["view"]) {
  const dom = view.contentDOM;
  const sel = view.state.selection.main.head;
  const at = view.domAtPos(sel);
  let node = at.node;
  let offset = at.offset;
  if (node.nodeType !== Node.TEXT_NODE) {
    const before = node.childNodes[offset - 1];
    const after = node.childNodes[offset];
    if (before && before.nodeType === Node.TEXT_NODE) {
      node = before;
      offset = (before as Text).length;
    } else if (after && after.nodeType === Node.TEXT_NODE) {
      node = after;
      offset = 0;
    } else throw new Error(`the caret isn't in a text node (${node.nodeName})`);
  }
  const textNode = node as Text;
  const start = offset;
  let len = 0;
  const place = () => {
    const s = dom.ownerDocument.getSelection()!;
    s.collapse(textNode, start + len);
  };
  const fire = (type: string, data: string) => dom.dispatchEvent(new CompositionEvent(type, { data, bubbles: true }));
  const input = (data: string, composing: boolean) =>
    dom.dispatchEvent(new InputEvent("input", { data, inputType: "insertCompositionText", isComposing: composing, bubbles: true }));
  return {
    start() {
      fire("compositionstart", "");
    },
    update(text: string) {
      textNode.replaceData(start, len, text);
      len = text.length;
      place();
      fire("compositionupdate", text);
      input(text, true);
    },
    end(text: string) {
      if (text !== textNode.data.slice(start, start + len)) {
        textNode.replaceData(start, len, text);
        len = text.length;
        place();
      }
      fire("compositionend", text);
      input(text, false);
    },
    /** Whether the composed text is still in the DOM node (not replaced under the composition). */
    intact(text: string) {
      return textNode.isConnected && textNode.data.slice(start, start + len) === text;
    },
  };
}

(window as unknown as { MPC: unknown }).MPC = { scenario, syntheticComposition };
