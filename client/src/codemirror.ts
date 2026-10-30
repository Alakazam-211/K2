// The CodeMirror 6 binding: an editor showing one `LiveText`. Local transactions become hunks
// (UTF-16 to UTF-8 bytes here, and only here); remote changes become CodeMirror changes;
// remote carets and selections show as decorations with each participant's name and colour;
// rejected changes are marked where they were. The editor's own history should be left out:
// undo and redo go to the server (`Mod-z`, `Mod-Shift-z`, `Mod-y`).

import {
  Annotation,
  StateEffect,
  StateField,
  Transaction,
  type ChangeSet,
  type ChangeSpec,
  type Extension,
  type Range,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  ViewPlugin,
  WidgetType,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { byteToIndex, coalesce, indexToByte, type Hunk } from "./hunk";
import type { LiveText, RemoteCaret } from "./live";

/** Marks transactions the binding makes from remote changes (not sent back). */
export const remoteChange = Annotation.define<boolean>();

/** A transaction's changes as hunks on `old` (the document before it), in UTF-8 bytes. */
export function changesToHunks(old: string, changes: ChangeSet): Hunk[] {
  const out: Hunk[] = [];
  let at16 = 0;
  let at8 = 0;
  const byteAt = (i: number) => {
    at8 += indexToByte(old.slice(at16, i), i - at16);
    at16 = i;
    return at8;
  };
  changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    const start = byteAt(fromA);
    const end = byteAt(toA);
    out.push({ start, end, insert: inserted.toString() });
  });
  return coalesce(out.filter((h) => h.start !== h.end || h.insert.length > 0));
}

/** Hunks (UTF-8 bytes of `text`) as CodeMirror changes (UTF-16 offsets of `text`). */
export function hunksToChanges(text: string, hunks: readonly Hunk[]): ChangeSpec[] {
  return hunks.map((h) => {
    const from = byteToIndex(text, h.start);
    const to = byteToIndex(text, h.end);
    if (from < 0 || to < 0) throw new Error("a hunk is off a character boundary");
    return { from, to, insert: h.insert };
  });
}

const setCarets = StateEffect.define<DecorationSet>();
const setRejected = StateEffect.define<DecorationSet>();

function decoField(effect: typeof setCarets): StateField<DecorationSet> {
  return StateField.define<DecorationSet>({
    create: () => Decoration.none,
    update(deco, tr) {
      for (const e of tr.effects) if (e.is(effect)) return e.value;
      return tr.docChanged ? deco.map(tr.changes) : deco;
    },
    provide: (f) => EditorView.decorations.from(f),
  });
}

class CaretWidget extends WidgetType {
  constructor(
    readonly name: string,
    readonly colour: string,
  ) {
    super();
  }
  eq(o: CaretWidget) {
    return o.name === this.name && o.colour === this.colour;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = "cm-mpc-caret";
    el.style.borderLeftColor = this.colour;
    const label = document.createElement("span");
    label.className = "cm-mpc-caret-name";
    label.textContent = this.name;
    label.style.backgroundColor = this.colour;
    el.appendChild(label);
    return el;
  }
  ignoreEvent() {
    return true;
  }
}

/** Decorations for remote carets and selections, in UTF-16 offsets of `text`. */
export function caretDecorations(text: string, carets: readonly RemoteCaret[]): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const c of carets) {
    const a = byteToIndex(text, c.anchor);
    const h = byteToIndex(text, c.head);
    if (a < 0 || h < 0) continue;
    const [from, to] = a <= h ? [a, h] : [h, a];
    if (from < to) {
      ranges.push(
        Decoration.mark({
          class: "cm-mpc-selection",
          attributes: { style: `background-color: ${c.colour}33`, title: c.name },
        }).range(from, to),
      );
    }
    ranges.push(Decoration.widget({ widget: new CaretWidget(c.name, c.colour), side: 1 }).range(h));
  }
  return Decoration.set(ranges, true);
}

function rejectedDecorations(text: string, live: LiveText): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const r of live.rejected) {
    const from = byteToIndex(text, r.region.start);
    const to = byteToIndex(text, r.region.end);
    if (from < 0 || to < 0) continue;
    const attrs = { class: "cm-mpc-rejected", attributes: { title: `not applied: ${r.text}` } };
    if (from < to) ranges.push(Decoration.mark(attrs).range(from, to));
    else ranges.push(Decoration.widget({ widget: new RejectedWidget(r.text), side: -1 }).range(from));
  }
  return Decoration.set(ranges, true);
}

class RejectedWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  eq(o: RejectedWidget) {
    return o.text === this.text;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = "cm-mpc-rejected-text";
    el.textContent = this.text;
    el.title = "not applied";
    return el;
  }
}

const theme = EditorView.baseTheme({
  ".cm-mpc-caret": {
    position: "relative",
    borderLeft: "2px solid",
    marginLeft: "-1px",
    marginRight: "-1px",
  },
  ".cm-mpc-caret-name": {
    position: "absolute",
    top: "-1.2em",
    left: "-2px",
    fontSize: "0.75em",
    lineHeight: "1.2em",
    padding: "0 3px",
    color: "white",
    whiteSpace: "nowrap",
    borderRadius: "2px",
    pointerEvents: "none",
  },
  ".cm-mpc-rejected": { textDecoration: "underline wavy #d33" },
  ".cm-mpc-rejected-text": { opacity: "0.55", textDecoration: "line-through", color: "#d33" },
});

export interface CodemirrorLiveOptions {
  /** Bind Mod-z / Mod-Shift-z / Mod-y to the server's undo and redo (default true). */
  undoKeys?: boolean;
}

/**
 * The CodeMirror 6 extension for `live`. The editor must start with `live.text` as its
 * document (`\n` line breaks only), and every change to it must go through the editor.
 */
export function codemirrorLive(live: LiveText, opts: CodemirrorLiveOptions = {}): Extension {
  const carets = decoField(setCarets);
  const rejected = decoField(setRejected);
  const plugin = ViewPlugin.fromClass(
    class {
      private offs: Array<() => void> = [];
      private pending: Array<() => void> = [];
      private kind = "";
      private group = 0;
      constructor(readonly view: EditorView) {
        const later = (f: () => void) => {
          this.pending.push(f);
          if (this.pending.length === 1) queueMicrotask(() => this.drain());
        };
        this.offs.push(
          live.onChange((e) => {
            if (e.local) return;
            // relative to live's text before this change, which is the editor's document
            // once the changes before it are in: dispatch in order, outside any update
            const hunks = e.hunks;
            later(() => {
              const text = this.view.state.doc.toString();
              this.view.dispatch({
                changes: hunksToChanges(text, hunks),
                annotations: remoteChange.of(true),
              });
              this.refresh();
            });
          }),
          live.onRoster(() => later(() => this.refresh())),
          live.onRejected(() => later(() => this.refresh())),
        );
        later(() => this.refresh());
      }
      drain() {
        const fs = this.pending;
        this.pending = [];
        for (const f of fs) f();
      }
      refresh() {
        const text = this.view.state.doc.toString();
        this.view.dispatch({
          effects: [
            setCarets.of(caretDecorations(text, live.carets())),
            setRejected.of(rejectedDecorations(text, live)),
          ],
        });
      }
      update(u: ViewUpdate) {
        for (const tr of u.transactions) {
          if (!tr.docChanged || tr.annotation(remoteChange)) continue;
          const kind = tr.isUserEvent("input")
            ? "input"
            : tr.isUserEvent("delete")
              ? "delete"
              : (tr.annotation(Transaction.userEvent) ?? "other");
          if (kind !== this.kind) {
            this.kind = kind;
            this.group++;
          }
          live.applyLocal(changesToHunks(tr.startState.doc.toString(), tr.changes), {
            group: `${kind}-${this.group}`,
          });
        }
        if (u.selectionSet) {
          const text = u.state.doc.toString();
          const m = u.state.selection.main;
          live.setCursor(indexToByte(text, m.anchor), indexToByte(text, m.head));
        }
      }
      destroy() {
        for (const off of this.offs) off();
      }
    },
  );
  const keys =
    opts.undoKeys === false
      ? []
      : [
          keymap.of([
            { key: "Mod-z", run: () => (live.undo(), true), preventDefault: true },
            { key: "Mod-Shift-z", run: () => (live.redo(), true), preventDefault: true },
            { key: "Mod-y", run: () => (live.redo(), true), preventDefault: true },
          ]),
        ];
  return [carets, rejected, plugin, theme, ...keys];
}
