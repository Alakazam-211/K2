// Copied from vendor/k2-mpc-base/client/src/editor/binding.ts (K2 owns this copy; see ./index.ts).
// The live editor's CodeMirror binding: one `LiveText` shown in one `EditorView`.
//
// - Local transactions become hunks through the byte index (`bytes.ts`): no `doc.toString()`
//   per keystroke.
// - Remote changes are first **held** as one CodeMirror change set on the editor's document,
//   then dispatched together, outside any update, and never during an IME composition (a
//   dispatch under a composition can break it or double the composed text). While changes are
//   held, the editor's document plus `held` is `live.text`; a local transaction meanwhile goes
//   to `live` mapped over `held`, and `held` is mapped over it (CodeMirror's mapping gives the
//   same document either way round).
// - The editor's own history is kept out: remote changes never enter it, and undo and redo
//   (keys, the browser's Edit menu) go to the server, ahead of any keymap the host adds.

import {
  ChangeSet,
  Prec,
  StateEffect,
  StateField,
  Transaction,
  type ChangeSpec,
  type Extension,
  type Range,
  type Text,
} from "@codemirror/state";
import {
  Decoration,
  EditorView,
  WidgetType,
  keymap,
  type DecorationSet,
  type ViewUpdate,
} from "@codemirror/view";
import { historyField } from "@codemirror/commands";
import { remoteChange } from "./codemirror";
import type { Hunk } from "@k2/mpc-client";
import type { LiveText, Rejection } from "@k2/mpc-client";
import { ByteIndex } from "./bytes";
import { readableOn } from "./colour";

/** The byte index of the editor's document, kept up to date by every transaction. */
export const byteIndex = StateField.define<ByteIndex>({
  create: (s) => ByteIndex.of(s.doc),
  update: (idx, tr) => (tr.docChanged ? idx.update(tr.startState.doc, tr.state.doc, tr.changes) : idx),
});

/** Hunks (UTF-8 bytes) for `changes` on `doc`. */
export function hunksOf(doc: Text, idx: ByteIndex, changes: ChangeSet): Hunk[] {
  const out: Hunk[] = [];
  changes.iterChanges((fromA, toA, _fromB, _toB, inserted) => {
    const start = idx.byteAt(doc, fromA);
    const end = fromA === toA ? start : idx.byteAt(doc, toA);
    const insert = inserted.toString();
    if (start !== end || insert) out.push({ start, end, insert });
  });
  return out;
}

/** CodeMirror changes on `doc` for `hunks` (UTF-8 bytes of `doc`), or null when one is off a
 * character boundary. */
export function changesOf(doc: Text, idx: ByteIndex, hunks: readonly Hunk[]): ChangeSet | null {
  const specs: ChangeSpec[] = [];
  for (const h of hunks) {
    const from = idx.posAt(doc, h.start);
    const to = h.end === h.start ? from : idx.posAt(doc, h.end);
    if (from < 0 || to < 0) return null;
    specs.push({ from, to, insert: h.insert });
  }
  return ChangeSet.of(specs, doc.length);
}

/** A caret to draw: positions in UTF-8 bytes of `live.text`. */
export interface CaretView {
  handle: string;
  name: string;
  colour: string;
  anchor: number;
  head: number;
  /** Show its name now (labels `active`: it moved lately; `always`: always). */
  label: boolean;
  /** The connection it is on (one person can have several: tabs, devices); absent from an
   * older host (one caret a person). Carets are keyed on (handle, client). */
  client?: number;
  /** Another connection of this person's own (the same person in another tab or on another
   * device): drawn dashed, its name "You · other tab" (or the host's label). */
  sameUser?: boolean;
}

/** A caret's key: one person's connections each get their own (K2, 2026-10-31). */
export function caretKey(c: { handle: string; client?: number }): string {
  return c.client === undefined ? c.handle : `${c.handle}#${c.client}`;
}

export interface BindingHooks {
  /** The person's own transaction went to `live` (after it is applied). */
  local?(): void;
  /** The editor and `live` disagreed and the editor was reset to `live.text`. */
  repaired?(reason: string): void;
  /** A reconfiguration added CodeMirror's history (once). Its commands then apply as the
   * person's own edits, sent like typing. */
  historyAdded?(): void;
  /** Undo or redo requested from the editor. */
  undo(): void;
  redo(): void;
  /** Carets to draw now. */
  carets(): CaretView[];
  /** The text "Insert my text here" would insert, and where (bytes of `live.text`), while the
   * person looks at it before inserting; `null`: none. */
  preview?(): { byte: number; text: string } | null;
}

const setCarets = StateEffect.define<DecorationSet>();
const setRejected = StateEffect.define<DecorationSet>();
const setReveal = StateEffect.define<DecorationSet>();
const setPreview = StateEffect.define<DecorationSet>();

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
    readonly label: boolean,
    readonly below: boolean,
    readonly key: string = "",
    readonly sameUser: boolean = false,
  ) {
    super();
  }
  eq(o: CaretWidget) {
    return (
      o.name === this.name &&
      o.colour === this.colour &&
      o.label === this.label &&
      o.below === this.below &&
      o.key === this.key &&
      o.sameUser === this.sameUser
    );
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = "cm-mpc-caret" + (this.sameUser ? " cm-mpc-caret-same" : "");
    el.setAttribute("aria-hidden", "true");
    if (this.key) el.dataset.mpcCaret = this.key;
    el.style.setProperty("--mpc-user", this.colour);
    el.style.setProperty("--mpc-user-fg", readableOn(this.colour));
    const label = document.createElement("span");
    label.className = "cm-mpc-caret-name" + (this.label ? " cm-mpc-caret-name-shown" : "") + (this.below ? " cm-mpc-caret-name-below" : "");
    label.textContent = this.name;
    el.appendChild(label);
    return el;
  }
  ignoreEvent() {
    return true;
  }
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
    el.className = "cm-mpc-rejected cm-mpc-rejected-text";
    el.textContent = this.text;
    return el;
  }
}

/** The text "Insert my text here" would insert, shown where it would go before it is inserted. */
class PreviewWidget extends WidgetType {
  constructor(readonly text: string) {
    super();
  }
  eq(o: PreviewWidget) {
    return o.text === this.text;
  }
  toDOM() {
    const el = document.createElement("span");
    el.className = "cm-mpc-insert-preview";
    el.textContent = this.text;
    return el;
  }
}

/** Whether a change is the person's own, made in this editor (not remote, not ours). */
const isLocal = (tr: Transaction) => tr.docChanged && !tr.annotation(remoteChange);

export class Binding {
  /** Remote changes not yet in the editor, on its document; `null` when none. */
  private held: ChangeSet | null = null;
  /** While changes are held: the editor's document with them applied (`live.text`, as a
   * CodeMirror `Text`) and its byte index, both kept up to date change by change, so a
   * keystroke during a composition never re-indexes the whole document. */
  private shadow: { doc: Text; idx: ByteIndex } | null = null;
  private flushQueued = false;
  private composing = false;
  private compositionTimer: ReturnType<typeof setTimeout> | null = null;
  private kind = "";
  private group = 0;
  private offs: Array<() => void> = [];
  private view: EditorView | null = null;
  private dead = false;
  private historySeen = false;
  /** This connection's caret in bytes of `live.text`, as last told to `live`. */
  caret: { anchor: number; head: number } | null = null;
  /** Times the editor had to be reset to `live.text` (a bug if ever non-zero). */
  repairs = 0;
  readonly carets = decoField(setCarets);
  readonly rejectedField = decoField(setRejected);
  readonly revealField = decoField(setReveal);
  readonly previewField = decoField(setPreview);

  constructor(
    readonly live: LiveText,
    private readonly hooks: BindingHooks,
  ) {}

  /** The extensions to put in the editor's state (with `live.text` as its document). */
  extension(): Extension {
    return [
      byteIndex,
      this.carets,
      this.rejectedField,
      this.revealField,
      this.previewField,
      // undo and redo are the server's, ahead of anything a host adds (a history keymap)
      Prec.highest(
        keymap.of([
          { key: "Mod-z", run: () => (this.hooks.undo(), true), preventDefault: true },
          { key: "Mod-Shift-z", run: () => (this.hooks.redo(), true), preventDefault: true },
          { key: "Mod-y", run: () => (this.hooks.redo(), true), preventDefault: true },
        ]),
      ),
      Prec.highest(
        EditorView.domEventHandlers({
          beforeinput: (e) => {
            if (e.inputType === "historyUndo" || e.inputType === "historyRedo") {
              e.preventDefault();
              if (e.inputType === "historyUndo") this.hooks.undo();
              else this.hooks.redo();
              return true;
            }
            return false;
          },
          compositionstart: () => {
            this.composing = true;
            if (this.compositionTimer) clearTimeout(this.compositionTimer);
            return false;
          },
          compositionend: () => {
            this.composing = false;
            this.flushAfterComposition(0);
            return false;
          },
        }),
      ),
      // A host's history command called directly (`undo(view)`) dispatches with `filter: false`,
      // so nothing here can stop it: it applies as the person's own edit and goes to the server
      // like typing. Remote changes never enter a history (`addToHistory: false`), so it can't
      // take anyone else's text out.
      EditorView.updateListener.of((u) => this.update(u)),
    ];
  }

  attach(view: EditorView): void {
    this.view = view;
    this.offs.push(
      this.live.onChange((e) => {
        if (!e.local) this.remote(e.hunks);
      }),
    );
    this.schedule();
  }

  private unhold(): void {
    this.held = null;
    this.shadow = null;
  }

  /** Whether remote changes are being held (an IME composition is under way). */
  get holding(): boolean {
    return this.held !== null;
  }

  /** Composing now, by the DOM events or CodeMirror's own reading. */
  get isComposing(): boolean {
    return this.composing || (this.view?.composing ?? false);
  }

  destroy(): void {
    this.dead = true;
    for (const off of this.offs) off();
    this.offs = [];
    if (this.compositionTimer) clearTimeout(this.compositionTimer);
  }

  // ---- remote

  private remote(hunks: Hunk[]): void {
    const view = this.view;
    if (!view || this.dead) return;
    const doc = view.state.doc;
    const base = this.shadow && this.held ? this.shadow.doc : doc;
    const idx = this.shadow && this.held ? this.shadow.idx : view.state.field(byteIndex);
    const cs = changesOf(base, idx, hunks);
    if (!cs) {
      this.unhold();
      return this.repair("a remote change is off a character boundary of the editor's text");
    }
    this.held = this.held ? this.held.compose(cs) : cs;
    const next = cs.apply(base);
    this.shadow = { doc: next, idx: idx.update(base, next, cs) };
    this.schedule();
  }

  /** Dispatch the held changes and redraw, soon and outside any update. */
  schedule(): void {
    if (this.flushQueued || this.dead) return;
    this.flushQueued = true;
    queueMicrotask(() => {
      this.flushQueued = false;
      this.flush();
    });
  }

  private flushAfterComposition(tries: number): void {
    if (this.compositionTimer) clearTimeout(this.compositionTimer);
    // CodeMirror reads the composition's last DOM change just after compositionend
    this.compositionTimer = setTimeout(() => {
      this.compositionTimer = null;
      if (this.dead) return;
      if (this.view?.composing && tries < 50) return this.flushAfterComposition(tries + 1);
      this.flush();
    }, tries === 0 ? 0 : 20);
  }

  /** Put the held changes in the editor (unless composing) and redraw the decorations. */
  flush(): void {
    const view = this.view;
    if (!view || this.dead) return;
    // nothing touches the editor under a composition, not even decorations: compositionend flushes
    if (this.isComposing) return;
    if (this.held) {
      const changes = this.held;
      this.unhold();
      view.dispatch({
        changes,
        annotations: [remoteChange.of(true), Transaction.addToHistory.of(false)],
        filter: false,
      });
    }
    this.redraw();
    this.sendCaret();
  }

  /** Redraw carets and rejections (in `live.text`, which is the editor's document now). */
  redraw(): void {
    const view = this.view;
    if (!view || this.dead || this.held) return;
    const doc = view.state.doc;
    const idx = view.state.field(byteIndex);
    view.dispatch({
      effects: [
        setCarets.of(caretDecorations(doc, idx, this.hooks.carets())),
        setRejected.of(rejectedDecorations(doc, idx, this.live.rejected)),
        setPreview.of(previewDecorations(doc, idx, this.hooks.preview?.() ?? null)),
      ],
    });
  }

  // ---- local

  private update(u: ViewUpdate): void {
    if (this.dead) return;
    if (!this.historySeen && u.transactions.some((tr) => tr.reconfigured) && u.state.field(historyField, false) !== undefined) {
      this.historySeen = true;
      this.hooks.historyAdded?.();
    }
    for (const tr of u.transactions) {
      if (!isLocal(tr)) continue;
      // a Re-apply is an undo step of its own: never composed with the typing around it
      const kind = tr.isUserEvent("input.reapply") ? "reapply" : tr.isUserEvent("input") ? "input" : tr.isUserEvent("delete") ? "delete" : (tr.annotation(Transaction.userEvent) ?? "other");
      if (kind !== this.kind || kind === "reapply") {
        this.kind = kind;
        this.group++;
      }
      let hunks: Hunk[];
      if (this.held && this.shadow) {
        // live.text is the editor's text plus the held changes (the shadow): map this change
        // over them, and keep the shadow and its index up to date (no re-index of the document)
        const { doc: shadow, idx } = this.shadow;
        const mine = tr.changes.map(this.held);
        hunks = hunksOf(shadow, idx, mine);
        const next = mine.apply(shadow);
        this.shadow = { doc: next, idx: idx.update(shadow, next, mine) };
        this.held = this.held.map(tr.changes, true);
      } else {
        hunks = hunksOf(tr.startState.doc, tr.startState.field(byteIndex), tr.changes);
      }
      if (hunks.length === 0) continue; // nothing left of it (the held changes did the same)
      try {
        this.live.applyLocal(hunks, { group: `${kind}-${this.group}` });
      } catch (e) {
        return this.repair(`a local change didn't fit the live text (${(e as Error).message})`);
      }
      this.hooks.local?.();
    }
    if ((u.selectionSet || u.docChanged) && !this.held) this.trackCaret(u.state.doc, u.state.field(byteIndex));
  }

  private trackCaret(doc: Text, idx: ByteIndex): void {
    const m = this.view!.state.selection.main;
    const anchor = idx.byteAt(doc, m.anchor);
    const head = idx.byteAt(doc, m.head);
    if (this.caret && this.caret.anchor === anchor && this.caret.head === head) return;
    this.caret = { anchor, head };
    this.live.setCursor(anchor, head);
  }

  private sendCaret(): void {
    const view = this.view;
    if (!view || this.held || !view.hasFocus) return;
    this.trackCaret(view.state.doc, view.state.field(byteIndex));
  }

  /** The editor no longer matches `live`: show `live.text` (the person's pending text is in it). */
  repair(reason: string): void {
    const view = this.view;
    if (!view || this.dead) return;
    this.repairs++;
    this.unhold();
    const doc = view.state.doc;
    const want = this.live.text;
    const have = doc.toString();
    if (have !== want) {
      let p = 0;
      const n = Math.min(have.length, want.length);
      while (p < n && have.charCodeAt(p) === want.charCodeAt(p)) p++;
      let s = 0;
      while (s < n - p && have.charCodeAt(have.length - 1 - s) === want.charCodeAt(want.length - 1 - s)) s++;
      queueMicrotask(() =>
        view.dispatch({
          changes: { from: p, to: have.length - s, insert: want.slice(p, want.length - s) },
          annotations: [remoteChange.of(true), Transaction.addToHistory.of(false)],
          filter: false,
        }),
      );
    }
    this.hooks.repaired?.(reason);
  }

  // ---- positions for the host

  /** CodeMirror changes for `hunks` (UTF-8 bytes of `live.text`), or null (a composition is
   * holding remote changes, or a hunk is off a character boundary). */
  changesFor(hunks: readonly Hunk[]): ChangeSet | null {
    const view = this.view;
    if (!view || this.held) return null;
    return changesOf(view.state.doc, view.state.field(byteIndex), hunks);
  }

  /** The UTF-16 position in the editor of a byte offset of `live.text` (or -1). */
  posOf(byte: number): number {
    const view = this.view;
    if (!view || this.held) return -1;
    return view.state.field(byteIndex).posAt(view.state.doc, byte);
  }

  /** Mark lines `first..last` (1-based) for a moment, and scroll them into view. */
  reveal(first: number, last: number, ms = 1600): void {
    const view = this.view;
    if (!view) return;
    const doc = view.state.doc;
    const a = Math.max(1, Math.min(first, doc.lines));
    const b = Math.max(a, Math.min(last, doc.lines));
    const ranges: Range<Decoration>[] = [];
    for (let l = a; l <= b; l++) ranges.push(Decoration.line({ class: "cm-mpc-reveal" }).range(doc.line(l).from));
    view.dispatch({
      effects: [setReveal.of(Decoration.set(ranges)), EditorView.scrollIntoView(doc.line(a).from, { y: "center" })],
    });
    setTimeout(() => {
      if (!this.dead && this.view) this.view.dispatch({ effects: setReveal.of(Decoration.none) });
    }, ms);
  }
}

/** Decorations for remote carets and selections. */
export function caretDecorations(doc: Text, idx: ByteIndex, carets: readonly CaretView[]): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  // A stable order (by key) so two carets at one spot (one person's two tabs) keep theirs from
  // update to update: the widgets are reused, not swapped (no flicker).
  const ordered = [...carets].sort((x, y) => (caretKey(x) < caretKey(y) ? -1 : caretKey(x) > caretKey(y) ? 1 : 0));
  for (const c of ordered) {
    const a = idx.posAt(doc, c.anchor);
    const h = idx.posAt(doc, c.head);
    if (a < 0 || h < 0) continue;
    const [from, to] = a <= h ? [a, h] : [h, a];
    if (from < to) {
      ranges.push(
        Decoration.mark({
          class: "cm-mpc-selection" + (c.sameUser ? " cm-mpc-selection-same" : ""),
          attributes: { style: `--mpc-user: ${c.colour}`, "data-mpc-caret": caretKey(c) },
        }).range(from, to),
      );
    }
    // on the first line a label above the caret would be cut off: put it below
    const below = doc.lineAt(h).number === 1;
    ranges.push(Decoration.widget({ widget: new CaretWidget(c.name, c.colour, c.label, below, caretKey(c), c.sameUser === true), side: 1 }).range(h));
  }
  return Decoration.set(ranges, true);
}

function previewDecorations(doc: Text, idx: ByteIndex, p: { byte: number; text: string } | null): DecorationSet {
  if (!p) return Decoration.none;
  const at = idx.posAt(doc, p.byte);
  if (at < 0) return Decoration.none;
  return Decoration.set([Decoration.widget({ widget: new PreviewWidget(p.text), side: -1 }).range(at)]);
}

function rejectedDecorations(doc: Text, idx: ByteIndex, rejected: readonly Rejection[]): DecorationSet {
  const ranges: Range<Decoration>[] = [];
  for (const r of rejected) {
    const from = idx.posAt(doc, r.region.start);
    const to = idx.posAt(doc, r.region.end);
    if (from < 0 || to < 0) continue;
    if (from < to) ranges.push(Decoration.mark({ class: "cm-mpc-rejected", attributes: { "data-mpc-rejection": String(r.id) } }).range(from, to));
    else if (r.text) ranges.push(Decoration.widget({ widget: new RejectedWidget(r.text), side: -1 }).range(from));
  }
  return Decoration.set(ranges, true);
}
