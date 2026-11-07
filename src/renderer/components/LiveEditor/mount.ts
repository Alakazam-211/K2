// K2's own editor code (see ./index.ts); started from k2-mpc-client 0.11.0's editor/mount.ts (deleted from the kit in v0.10.2).
// `mountLiveEditor`: a live CodeMirror editor for one text root, in plain DOM. It owns the
// connection's life (connect, welcome, live; on a drop, backoff and `connect({resume})`, then
// a resync; past the server's window, a snapshot), the binding, the carets, a roster strip,
// the rejections with their actions, and a banner. See EDITOR.md.

import { defaultKeymap, historyField } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import type { FrameScheduler, Hunk, InsertRefusal, Mark, Participant } from "@k2/mpc-client";
import { LiveText, type Rejection } from "@k2/mpc-client";
import { canRetry } from "@k2/mpc-client";
import type {
  ClientFrame,
  CloseEvent,
  CloseReason,
  ErrorReport,
  Handle,
  LiveTransport,
  Rights,
  RosterEntry,
  RosterEvent,
  ServerFrame,
  Transport,
  Welcome,
} from "@k2/mpc-client";
import { Binding, caretKey, type CaretView } from "./binding";
import { readableOn } from "./colour";
import type { ClaimEvent, RemovedTextNote } from "./notes";
import { editorTheme, injectFrameStyles } from "./theme";

export type ConnectionState = "connecting" | "live" | "reconnecting" | "closed";
export type LabelMode = "always" | "active" | "hover";

export interface EditorStrings {
  connecting: string;
  reconnecting: string;
  retryNow: string;
  readOnly: string;
  closed: Record<CloseReason, string>;
  notApplied: string;
  reapply: string;
  dismiss: string;
  you: string;
  /** The name on this person's own caret in another tab or on another device. */
  youElsewhere: string;
  /** The same, when the host labelled that connection (`{label}`). */
  youOn: string;
  roster: string;
  rejections: string;
  offline: string;
  /** On a rejection whose reply was lost: it may have landed after all. */
  mayHaveApplied: string;
  /** After Re-apply found the change already in the text (nothing re-applied). */
  alreadyPresent: string;
  /** A change whose reply was lost went through after all. */
  landed: string;
  /** A change the person re-applied had also gone through: it may be in twice. */
  duplicateRisk: string;
  /** After Re-apply couldn't tell (nothing re-applied). */
  cantTell: string;
  /** After Re-apply refused a change that met someone else's change since (nothing re-applied). */
  conflictReapply: string;
  /** After Re-apply couldn't work out where a change goes now (nothing re-applied). */
  cantPlace: string;
  /** The single-hunk action that inserts the typed text where the change was (after a preview). */
  insertHere: string;
  /** Confirm the insertion being previewed. */
  insertConfirm: string;
  /** Cancel the insertion being previewed. */
  insertCancel: string;
  /** While previewing: where the text will go (`{line}` is the 1-based line). */
  insertPreview: string;
  /** Added to the preview's note when the rejected edit also removed text (a replacement or a
   * deletion): Insert only puts the typed text back. */
  insertRemovedText: string;
  /** "Insert my text here" on a change with more than one hunk (nothing inserted). */
  insertMultiHunk: string;
  /** Copy the rejected text. */
  copy: string;
  /** After a copy. */
  copied: string;
  /** When the clipboard refused: the text is selected for the person to copy. */
  copyFailed: string;
  /** @deprecated v0.9.8.2's single-hunk guards; no longer shown. */
  multiHunk: string;
  /** @deprecated v0.9.8.2's single-hunk guards; no longer shown. */
  notInsertion: string;
  /** @deprecated v0.9.8.2's single-hunk guards; no longer shown. */
  reloaded: string;
  /** @deprecated v0.9.8.2's single-hunk guards; no longer shown. */
  tie: string;
  /** @deprecated v0.9.8.2's single-hunk guards; no longer shown. */
  deletedAround: string;
}

export const DEFAULT_STRINGS: EditorStrings = {
  connecting: "Connecting…",
  reconnecting: "Reconnecting…",
  retryNow: "Retry now",
  readOnly: "Read-only",
  closed: {
    left: "You left this document.",
    idle: "Disconnected after a while without activity.",
    kicked: "You were removed from this document.",
    rights_lost: "You no longer have access to this document.",
    deleted: "This document was deleted.",
    moved: "This document was moved.",
    released: "This document was handed over.",
    cut_off: "The connection fell too far behind.",
    declined: "This document can't be edited live.",
    not_found: "This document wasn't found.",
    auth: "Sign in again to keep editing.",
    network: "The connection was lost.",
    closed: "Live editing of this document was closed.",
  },
  notApplied: "Not applied:",
  reapply: "Re-apply",
  dismiss: "Dismiss",
  you: "(you)",
  youElsewhere: "You · other tab",
  youOn: "You · {label}",
  roster: "People in this document",
  rejections: "Changes that weren't applied",
  offline: "Not connected: nothing was sent.",
  mayHaveApplied: "This change may already be in the document.",
  alreadyPresent: "This change is already in the document: nothing was re-applied.",
  landed: "A change that seemed lost went through after all.",
  duplicateRisk: "A change you re-applied had also gone through: it may now be in the document twice. Check it.",
  cantTell: "Can't tell whether this change is already in the document: nothing was re-applied. Check it, then type what's missing.",
  conflictReapply: "Someone else changed this text since: nothing was re-applied. Check it, then type what's missing.",
  cantPlace: "Can't tell where this change goes now (the document reloaded, or a lot changed since): nothing was re-applied. Check it, then type what's missing.",
  insertHere: "Insert my text here…",
  insertConfirm: "Insert",
  insertCancel: "Cancel",
  insertPreview: "Your text will go in at line {line}, shown in the editor. Insert it, or Cancel.",
  insertRemovedText: "Your edit also removed text; that part isn't redone here.",
  insertMultiHunk: "This change touched more than one place: copy its text and retype it where you want it.",
  copy: "Copy",
  copied: "Copied.",
  copyFailed: "Couldn't copy: the text is selected, copy it with your keyboard.",
  multiHunk: "This change touched more than one place: nothing was re-applied. Check it, then type what's missing.",
  notInsertion: "This change deleted or replaced text: nothing was re-applied. Check it, then type what's missing.",
  reloaded: "The document reloaded since this change: nothing was re-applied. Check it, then type what's missing.",
  tie: "Something else was typed at the same place since: nothing was re-applied. Check it, then type what's missing.",
  deletedAround: "The text around this change was deleted or replaced since: nothing was re-applied. Check it, then type what's missing.",
};

/** What the banner shows (for `render.banner`). */
export interface BannerInfo {
  state: ConnectionState;
  rights: Rights;
  /** Set once closed. */
  close: CloseEvent | null;
  /** The default text for this state. */
  text: string;
  /** Try to reconnect now (while reconnecting). */
  retry(): void;
}

/**
 * A rejected change (`LiveText`'s `Rejection`), plus `mayHaveApplied`: its reply was lost
 * with a connection (`MPC-E000`), so it may have landed after all; Re-apply checks first.
 */
export type EditorRejection = Rejection & { mayHaveApplied: boolean };

/**
 * What `reapply` did:
 * - `applied`: the change went back in where the server would put it, as a new edit (its own undo
 *   step);
 * - `already-present`: it is in the text already (a `mayHaveApplied` change that landed, or a
 *   change with nothing left to apply, such as a deletion someone else made); nothing applied,
 *   the rejection dismissed;
 * - `unknown`: Re-apply can't put it back where the server would: nothing applied, the
 *   rejection kept, a note asks the person to check and retype (`reason` says why);
 * - `unavailable`: not now (no such rejection, read-only, or an IME composition under way).
 */
export type ReapplyResult = {
  status: "applied" | "already-present" | "unknown" | "unavailable";
  /**
   * Why `unknown`:
   * - `cant-tell`: (a `mayHaveApplied` change) the record can't tell whether it landed (its
   *   version left the history, or an undo was unanswered on the lost connection);
   * - `conflict`: the change met someone else's change since (the server refused it, or would);
   * - `cant-place`: the editor can't work out where the server would put it now (it reloaded
   *   since, or more changed since than its history keeps).
   *
   * `multi-hunk`, `not-insertion`, `reloaded`, `tie` and `deleted-around` were v0.9.8.2's
   * single-hunk guards; Re-apply places every change by the server's rule now and never returns
   * them (kept in the type for one release).
   */
  reason?: "cant-tell" | "conflict" | "cant-place" | "multi-hunk" | "not-insertion" | "reloaded" | "tie" | "deleted-around";
};

/**
 * What "Insert my text here" did (`insertHere`, `confirmInsert`):
 * - `previewing`: the typed text is shown in the editor where it would go (`line`); nothing is
 *   inserted until `confirmInsert` (or `cancelInsert`);
 * - `applied`: inserted there, as a new edit (its own undo step), the rejection dismissed;
 * - `already-present`: (a `mayHaveApplied` change) it landed after all; nothing inserted;
 * - `unknown`: `cant-tell` (it may have landed: the record can't tell) or `multi-hunk` (only a
 *   single-hunk change has one place to insert at); nothing inserted;
 * - `unavailable`: not now (read-only, no such rejection or no text, a composition, nothing
 *   previewed).
 */
export type InsertResult = {
  status: "previewing" | "applied" | "already-present" | "unknown" | "unavailable";
  reason?: "cant-tell" | "multi-hunk";
  /** While `previewing`: the line (1-based) the text would go in at. */
  line?: number;
};

export interface RejectionActions {
  reapply(): ReapplyResult;
  dismiss(): void;
  reveal(): void;
  /** Preview inserting the typed text where the change was (single-hunk changes). */
  insertHere(): InsertResult;
  /** Insert the previewed text. */
  confirmInsert(): InsertResult;
  cancelInsert(): void;
  /** Copy the rejected text to the clipboard (or select it, when the clipboard refuses). */
  copy(): Promise<boolean>;
}

export interface LiveEditorOptions {
  transport: LiveTransport;
  /** The text root (default `source`). */
  root?: string;
  /** `markdown` (default): Markdown highlighting and list keys. `plain`: neither. */
  mode?: "markdown" | "plain";
  /** More CodeMirror extensions from the host (they share its CodeMirror). */
  extensions?: Extension;
  /** Never edit, whatever the rights. */
  readOnly?: boolean;
  /** When remote carets show their names (default `active`: for a while after they move). */
  labels?: LabelMode;
  /** Retry after a drop (default 1–15 s, jittered); `false`: a drop closes the editor. */
  reconnect?: { minMs: number; maxMs: number } | false;
  strings?: Partial<Omit<EditorStrings, "closed">> & { closed?: Partial<Record<CloseReason, string>> };
  render?: {
    /** One rejection's row content; `false` hides the list (the host shows `rejected`). */
    rejection?: ((r: EditorRejection, actions: RejectionActions) => HTMLElement) | false;
    /** The banner's content (`null`: no banner now); `false`: never a banner. */
    banner?: ((b: BannerInfo) => HTMLElement | null) | false;
    /** The roster strip; `false`: none (the host draws its own from `roster`). */
    roster?: ((entries: readonly RosterEntry[], click: (handle: Handle) => void) => HTMLElement) | false;
  };
  /** Timers and randomness (tests drive them); also the replica's clock (caret re-sends, when
   * a remote caret was last active). */
  timers?: Timers;
  /** When other people's carets and the rejected changes redraw after they change
   * (`LiveOptions.schedule`): default at most once an animation frame. Tests drive it. */
  schedule?: FrameScheduler;
  /** How long a snapshot request may take before a fresh connect replaces it (default 10 s). */
  snapshotTimeoutMs?: number;
}

export interface Timers {
  set(f: () => void, ms: number): unknown;
  clear(h: unknown): void;
  random(): number;
  now(): number;
}

export interface LiveEditorEvents {
  state: ConnectionState;
  roster: readonly RosterEntry[];
  rights: Rights;
  change: { hunks: Hunk[]; by: Handle | null; local: boolean };
  pending: number;
  ack: { ops: number[]; version: number };
  rejected: EditorRejection;
  /** A rejection the server refused as a write conflict (`MPC-E111`); `details.conflicts`. */
  conflict: { rejection: EditorRejection; error: ErrorReport };
  note: RemovedTextNote;
  /** A change whose reply was lost (`mayHaveApplied`) landed after all: it left `rejected`. */
  landed: { rejection: EditorRejection; version: number };
  /** A change the person re-applied had also landed (only on a host that breaks WIRE.md's
   * reconnect rule): its text may be in the document twice. */
  duplicateRisk: { rejection: EditorRejection; landed: number; reapplied: number };
  claim: ClaimEvent;
  /** The editor started again from a snapshot (its pending changes went to `rejected`). */
  reload: { reason: string; error: ErrorReport | null };
  closed: CloseEvent;
  error: ErrorReport;
  participantClick: { handle: Handle; entry: RosterEntry | null };
}

export interface LiveEditorHandle {
  readonly view: EditorView;
  readonly text: string;
  readonly version: number;
  readonly state: ConnectionState;
  readonly rights: Rights;
  readonly roster: readonly RosterEntry[];
  readonly rejected: readonly EditorRejection[];
  /** Local changes not yet acknowledged. */
  readonly pending: number;
  on<K extends keyof LiveEditorEvents>(event: K, cb: (v: LiveEditorEvents[K]) => void): () => void;
  undo(): void;
  redo(): void;
  /** Re-insert a rejection's text where it was, as a new edit. False when it can't now. */
  reapply(id: number): ReapplyResult;
  dismiss(id: number): void;
  /**
   * "Insert my text here", the person's own choice for a single-hunk rejection: shows its typed
   * text in the editor where it would go (`previewing`, with the line); nothing is inserted until
   * `confirmInsert()`. Unlike Re-apply it doesn't follow the server's rule: it inserts the typed
   * text at the change's place as the editor tracked it, and never deletes.
   */
  insertHere(id: number): InsertResult;
  /** Insert the text being previewed, as a new edit (its own undo step). */
  confirmInsert(): InsertResult;
  /** Stop previewing; nothing inserted. */
  cancelInsert(): void;
  /** Copy a rejection's text to the clipboard; when the clipboard refuses, select it. */
  copyRejected(id: number): Promise<boolean>;
  /** Scroll to lines `first..last` (1-based) and mark them for a moment. */
  reveal(lines: { first: number; last: number }): void;
  /** Keep a participant's caret in view (`null` stops). */
  follow(handle: Handle | null): void;
  /** The person can (or can't) see the editor: tells the host; showing it again resyncs. */
  setVisible(visible: boolean): void;
  /** Reconnect now instead of waiting for the backoff (or resync when live). */
  reconnectNow(): void;
  destroy(): void;
}

/** The error code for a host CodeMirror history (the server owns undo). */
export const HISTORY_ERROR = "MPC-EDITOR-HISTORY";
/** A transport reported a close whose `retry` disagrees with the close table (WIRE.md,
 * Closing): the editor goes by the table and reports it once per reason. */
export const CLOSE_RETRY_ERROR = "MPC-EDITOR-CLOSE-RETRY";

/** Thrown by `mountLiveEditor` when the host's set-up can't work (`code` is stable). */
export class LiveEditorError extends Error {
  constructor(
    readonly code: string,
    message: string,
  ) {
    super(message);
    this.name = "LiveEditorError";
  }
}

const HISTORY_MESSAGE =
  "the host's extensions include CodeMirror's history; in a live editor the server owns undo " +
  "(Mod-z goes to it), so leave history() out";

const SNAPSHOT_OP_BASE = 0x40000000;
const ACTIVE_LABEL_MS = 2500;

const systemTimers: Timers = {
  set: (f, ms) => setTimeout(f, ms),
  clear: (h) => clearTimeout(h as ReturnType<typeof setTimeout>),
  random: Math.random,
  now: () => Date.now(),
};

// a close event by its reason; its `retry` is the transport's claim, which the table overrides
const isCloseEvent = (e: unknown): e is CloseEvent =>
  typeof e === "object" && e !== null && typeof (e as CloseEvent).reason === "string";

/** Mount a live editor in `parent`. */
export function mountLiveEditor(parent: HTMLElement, opts: LiveEditorOptions): LiveEditorHandle {
  return new LiveEditor(parent, opts);
}

class LiveEditor implements LiveEditorHandle {
  readonly view: EditorView;
  private readonly t: LiveTransport;
  private readonly root: string;
  private readonly strings: EditorStrings;
  private readonly timers: Timers;
  private readonly labels: LabelMode;
  private readonly dom: {
    frame: HTMLElement;
    banner: HTMLElement;
    roster: HTMLElement;
    body: HTMLElement;
    rejections: HTMLElement;
    note: HTMLElement;
  };
  private readonly readOnlyConf = new Compartment();
  private live: LiveText | null = null;
  private binding: Binding | null = null;
  private _state: ConnectionState = "connecting";
  private _rights: Rights = "read";
  private self: Handle | null = null;
  private closeInfo: CloseEvent | null = null;
  private closedFrame: CloseEvent | null = null;
  private abort: AbortController | null = null;
  private connecting = false;
  private attempts = 0;
  private retryTimer: unknown = null;
  private labelTimer: unknown = null;
  private snapshot: { op: number; timer: unknown } | null = null;
  private nextSnapshotOp = SNAPSHOT_OP_BASE;
  private lastPending = 0;
  private following: Handle | null = null;
  private visible = true;
  private destroyed = false;
  private seenNotes = new Set<string>();
  /** Rejections that landed after all (Re-apply on them says so). */
  private landedIds = new Set<number>();
  /** The rejection whose "Insert my text here" is being previewed. */
  private previewId: number | null = null;
  /**
   * The carets already in the doc when it opened (the first roster), by `caretKey`, with their
   * `lastActiveAt` then: with labels `active` their names stay hidden until they move (a caret
   * first seen later shows its name, as one that moves does). `null` until the first roster.
   */
  private openingCarets: Map<string, number> | null = null;
  private readonly ls = new Map<keyof LiveEditorEvents, Set<(v: never) => void>>();
  private readonly offs: Array<() => void> = [];
  private readonly frameListeners = new Set<(f: ServerFrame) => void>();

  constructor(parent: HTMLElement, private readonly opts: LiveEditorOptions) {
    // the server owns undo: a host history would undo locally, out of step with everyone
    if (EditorState.create({ extensions: opts.extensions ?? [] }).field(historyField, false) !== undefined) {
      throw new LiveEditorError(HISTORY_ERROR, HISTORY_MESSAGE);
    }
    this.t = opts.transport;
    this.root = opts.root ?? "source";
    this.timers = opts.timers ?? systemTimers;
    this.labels = opts.labels ?? "active";
    this.strings = {
      ...DEFAULT_STRINGS,
      ...opts.strings,
      closed: { ...DEFAULT_STRINGS.closed, ...opts.strings?.closed },
    } as EditorStrings;
    const doc = parent.ownerDocument;
    injectFrameStyles(doc);
    const el = (tag: string, cls: string) => {
      const e = doc.createElement(tag);
      e.className = cls;
      return e;
    };
    const frame = el("div", `mpc-editor mpc-labels-${this.labels}`);
    const banner = el("div", "mpc-banner");
    banner.setAttribute("role", "status");
    banner.hidden = true;
    const roster = el("div", "mpc-roster");
    roster.setAttribute("role", "group");
    roster.setAttribute("aria-label", this.strings.roster);
    roster.hidden = true;
    const body = el("div", "mpc-body");
    const rejections = el("ul", "mpc-rejections");
    rejections.setAttribute("aria-label", this.strings.rejections);
    rejections.setAttribute("aria-live", "polite");
    rejections.hidden = true;
    const note = el("div", "mpc-note");
    note.setAttribute("role", "status");
    note.hidden = true;
    frame.append(banner, roster, body, rejections, note);
    parent.appendChild(frame);
    this.dom = { frame, banner, roster, body, rejections, note };
    this.view = new EditorView({ parent: body, state: this.createState("") });

    this.offs.push(
      this.t.onFrame((f) => this.onHostFrame(f)),
      this.t.onClose((e) => this.onClose(e)),
    );
    const win = doc.defaultView;
    if (win) {
      const online = () => this.reconnectNow();
      const vis = () => this.setVisible(doc.visibilityState !== "hidden");
      win.addEventListener("online", online);
      doc.addEventListener("visibilitychange", vis);
      this.offs.push(
        () => win.removeEventListener("online", online),
        () => doc.removeEventListener("visibilitychange", vis),
      );
    }
    this.renderBanner();
    // the first attempt after mount returns, so every event (a refusal included) reaches the
    // host's listeners
    queueMicrotask(() => this.attempt(false));
  }

  // ---- the handle

  get text(): string {
    return this.live?.text ?? "";
  }
  get version(): number {
    return this.live?.version ?? 0;
  }
  get state(): ConnectionState {
    return this._state;
  }
  get rights(): Rights {
    return this._rights;
  }
  get roster(): readonly RosterEntry[] {
    return this.live?.roster ?? [];
  }
  get rejected(): readonly EditorRejection[] {
    return (this.live?.rejected ?? []) as readonly EditorRejection[];
  }
  get pending(): number {
    return this.live?.pending ?? 0;
  }

  on<K extends keyof LiveEditorEvents>(event: K, cb: (v: LiveEditorEvents[K]) => void): () => void {
    let set = this.ls.get(event);
    if (!set) this.ls.set(event, (set = new Set()));
    const s = set;
    s.add(cb as (v: never) => void);
    return () => {
      s.delete(cb as (v: never) => void);
    };
  }

  undo(): void {
    if (this.destroyed) return;
    if (!this.live) return;
    if (this._state !== "live") return this.emit("error", { id: "MPC-E000", message: this.strings.offline });
    this.live.undo();
  }

  redo(): void {
    if (this.destroyed) return;
    if (!this.live) return;
    if (this._state !== "live") return this.emit("error", { id: "MPC-E000", message: this.strings.offline });
    this.live.redo();
  }

  reapply(id: number): ReapplyResult {
    const live = this.live;
    const b = this.binding;
    const unavailable: ReapplyResult = { status: "unavailable" };
    if (!live || !b || this.isReadOnly()) return unavailable;
    if (this.landedIds.has(id)) {
      this.showNote(this.strings.alreadyPresent);
      return { status: "already-present" };
    }
    const m = this.markOf(id);
    if (!m) return unavailable;
    if (b.holding) return unavailable; // during a composition
    // its reply was lost: Re-apply only when the server's record shows it never landed (the
    // mark's `presence`; `absent` for a change the server refused)
    if (m.presence === "present") return this.alreadyPresent(id);
    if (m.presence !== "absent") {
      this.showNote(this.strings.cantTell);
      return { status: "unknown", reason: "cant-tell" };
    }
    // where the server would put it: its change as sent, rebased from its base (never by region)
    switch (m.placement) {
      case "conflict":
      case "cant-place":
        this.showNote(m.placement === "conflict" ? this.strings.conflictReapply : this.strings.cantPlace);
        return { status: "unknown", reason: m.placement };
      case "in-text":
        return this.alreadyPresent(id);
    }
    b.editThroughLive(() => live.reapply(id, { group: b.newGroup("reapply") }));
    if (m.mayHaveApplied) live.noteReapplied(id);
    this.dismiss(id);
    this.showNote("");
    return { status: "applied" };
  }

  /** The rejection `id` as the library's mark (`live.marks()`), if it is open. */
  private markOf(id: number): Mark | undefined {
    return this.live?.marks().find((m) => m.id === id);
  }

  /** It is in the text already: dismissed, and the note says so. */
  private alreadyPresent(id: number): { status: "already-present" } {
    this.dismiss(id);
    this.showNote(this.strings.alreadyPresent);
    return { status: "already-present" };
  }

  /** What "Insert my text here" says when the library refuses it (`insertHerePreview`,
   * `insertHere`). */
  private insertRefused(id: number, refused: InsertRefusal): InsertResult {
    switch (refused) {
      case "already-present":
        this.alreadyPresent(id);
        return { status: "already-present" };
      case "cant-tell":
        this.showNote(this.strings.cantTell);
        return { status: "unknown", reason: "cant-tell" };
      case "multi-hunk":
        this.showNote(this.strings.insertMultiHunk);
        return { status: "unknown", reason: "multi-hunk" };
      case "unavailable":
        return { status: "unavailable" };
    }
  }

  /** A short status line under the rejections (empty: none). */
  private showNote(text: string): void {
    const n = this.dom.note;
    n.textContent = text;
    n.hidden = !text;
  }

  dismiss(id: number): void {
    if (this.previewId === id) this.previewId = null;
    this.live?.dismiss(id);
    this.renderRejections();
    this.binding?.schedule();
  }

  insertHere(id: number): InsertResult {
    const live = this.live;
    const b = this.binding;
    const unavailable: InsertResult = { status: "unavailable" };
    if (!live || !b || this.isReadOnly()) return unavailable;
    if (this.landedIds.has(id)) {
      this.showNote(this.strings.alreadyPresent);
      return { status: "already-present" };
    }
    if (b.holding) return unavailable;
    // the library's checks: typed text, one hunk (one place), then a lost reply's record
    const p = live.insertHerePreview(id);
    if ("refused" in p) return this.insertRefused(id, p.refused);
    const doc = this.view.state.doc;
    if (p.at > doc.length) return unavailable;
    const line = doc.lineAt(p.at).number;
    this.previewId = id;
    this.renderRejections();
    b.redraw();
    this.reveal({ first: line, last: line });
    // a replacement or deletion: say that only the typed text goes back, never the removal
    this.showNote(this.strings.insertPreview.replace("{line}", String(line)) + (p.removed ? ` ${this.strings.insertRemovedText}` : ""));
    (this.dom.rejections.querySelector(`li[data-rejection="${id}"] .mpc-insert-confirm`) as HTMLElement | null)?.focus();
    return { status: "previewing", line };
  }

  confirmInsert(): InsertResult {
    const live = this.live;
    const b = this.binding;
    const id = this.previewId;
    if (!live || !b || id === null || this.isReadOnly() || b.holding) return { status: "unavailable" };
    if (this.landedIds.has(id)) {
      this.previewId = null;
      this.showNote(this.strings.alreadyPresent);
      return { status: "already-present" };
    }
    this.previewId = null;
    // the library inserts it where the preview showed, as its own undo step, notes a lost
    // reply's change as re-applied (`onDuplicateRisk`), and dismisses it
    const done = b.editThroughLive(() => live.insertHere(id, { group: b.newGroup("insert-here") }));
    if ("refused" in done) {
      const out = this.insertRefused(id, done.refused);
      this.renderRejections();
      b.redraw();
      return out;
    }
    this.dismiss(id);
    this.showNote("");
    return { status: "applied" };
  }

  cancelInsert(): void {
    const id = this.previewId;
    if (id === null) return;
    this.previewId = null;
    this.showNote("");
    this.renderRejections();
    this.binding?.redraw(); // the preview goes at once
    (this.dom.rejections.querySelector(`li[data-rejection="${id}"] .mpc-insert-here`) as HTMLElement | null)?.focus();
  }

  async copyRejected(id: number): Promise<boolean> {
    const r = this.rejected.find((x) => x.id === id);
    if (!r) return false;
    const win = this.dom.frame.ownerDocument.defaultView;
    try {
      if (!win?.navigator.clipboard) throw new Error("no clipboard");
      await win.navigator.clipboard.writeText(r.text);
      this.showNote(this.strings.copied);
      return true;
    } catch {
      // the clipboard refused: select the text, so the person can copy it with their keyboard
      const snippet = this.dom.rejections.querySelector(`li[data-rejection="${id}"] .mpc-rejected-snippet`);
      const sel = win?.getSelection();
      if (snippet && sel) sel.selectAllChildren(snippet);
      this.showNote(this.strings.copyFailed);
      return false;
    }
  }

  reveal(lines: { first: number; last: number }): void {
    this.binding?.reveal(lines.first, lines.last);
  }

  follow(handle: Handle | null): void {
    this.following = handle;
    this.scrollToFollowed();
  }

  setVisible(visible: boolean): void {
    if (this.destroyed) return;
    const was = this.visible;
    this.visible = visible;
    // LiveText remembers it and sends it again after a reconnect
    this.live?.setVisible(visible);
    if (this._state === "live") {
      if (visible && !was) this.live?.resync();
    } else if (visible && this._state === "reconnecting") this.reconnectNow();
  }

  reconnectNow(): void {
    if (this.destroyed) return;
    if (this._state === "live") return this.live?.resync();
    if (this._state !== "reconnecting" || this.connecting) return;
    this.clearRetry();
    this.attempt(true);
  }

  destroy(): void {
    if (this.destroyed) return;
    this.destroyed = true;
    this.abort?.abort();
    this.clearRetry();
    if (this.labelTimer !== null) this.timers.clear(this.labelTimer);
    if (this.snapshot) this.timers.clear(this.snapshot.timer);
    for (const off of this.offs) off();
    this.binding?.destroy();
    this.live?.close();
    this.view.destroy();
    this.dom.frame.remove();
    this.t.close();
  }

  // ---- the connection

  private attempt(resume: boolean): void {
    if (this.destroyed || this._state === "closed") return;
    this.abort?.abort();
    const ac = new AbortController();
    this.abort = ac;
    this.connecting = true;
    this.closedFrame = null;
    const from = resume && this.live ? this.live.version : undefined;
    // the previous connection's client ID, so the host leaves it first (WIRE.md, Reconnecting)
    const prev = this.live?.client;
    // no connection to send on until the welcome: edits wait in LiveText's queue
    this.live?.disconnected();
    this.setState(this.live ? "reconnecting" : "connecting");
    let p: Promise<Welcome>;
    try {
      p = this.t.connect({ resume: from, ...(prev !== undefined ? { prev } : {}), signal: ac.signal });
    } catch (e) {
      p = Promise.reject(e);
    }
    p.then(
      (w) => {
        if (ac.signal.aborted || this.destroyed) return;
        this.connecting = false;
        this.onWelcome(w);
      },
      (e) => {
        if (ac.signal.aborted || this.destroyed) return;
        this.connecting = false;
        if (isCloseEvent(e)) {
          const c = this.retryOf(e);
          if (!c.retry || this.opts.reconnect === false) return this.close(c);
        }
        if (this.opts.reconnect === false) return this.close({ reason: "network", retry: false });
        if (!isCloseEvent(e)) this.emit("error", { id: "MPC-E000", message: `connect failed: ${String((e as Error)?.message ?? e)}` });
        this.retryLater();
      },
    );
  }

  private onWelcome(w: Welcome): void {
    if (w.root !== this.root) {
      this.emit("error", { id: "MPC-E000", message: `the host welcomed root ${w.root}, not ${this.root}` });
      return this.close({ reason: "declined", retry: false });
    }
    this.self = w.self;
    this.attempts = 0;
    this.setRights(w.rights);
    if (!this.live) {
      if (w.text === undefined) {
        this.emit("error", { id: "MPC-E000", message: "the host's first welcome carried no text" });
        return this.close({ reason: "declined", retry: false });
      }
      this.start(w);
      this.setState("live");
      if (!this.visible) this.live!.setVisible(false);
      return;
    }
    const live = this.live;
    this.setState("live");
    // LiveText takes the new client ID, sends caret and visibility again, and catches up: with
    // `text`, from it (pending changes go to `rejected`); without, with `since` (whose reply
    // settles the edit in flight), then sends the edits made while disconnected
    if (w.text !== undefined) {
      this.cancelSnapshot();
      live.reconnected(w);
      this.emit("reload", { reason: "the host couldn't resume", error: null });
      this.renderRejections();
    } else live.reconnected(w);
  }

  private onClose(e: CloseEvent): void {
    if (this.destroyed || this._state === "closed") return;
    const info = this.closedFrame ?? this.retryOf(e);
    this.closedFrame = null;
    this.cancelSnapshot();
    this.abort?.abort();
    this.connecting = false;
    this.live?.disconnected();
    if (!info.retry || this.opts.reconnect === false) return this.close(info);
    this.setState(this.live ? "reconnecting" : "connecting");
    this.retryLater();
  }

  /** Whether a close allows a reconnect is the close table's (`canRetry`, WIRE.md, Closing),
   * never the transport's own `retry`: a transport that reports, say, `auth` with `retry: true`
   * must not make the editor retry. A disagreement is reported once per reason. */
  private retryOf(e: CloseEvent): CloseEvent {
    const retry = canRetry(e.reason);
    if (typeof e.retry === "boolean" && e.retry !== retry && !this.retryMismatches.has(e.reason)) {
      this.retryMismatches.add(e.reason);
      this.emit("error", {
        id: CLOSE_RETRY_ERROR,
        message: `the transport reported "${e.reason}" with retry ${e.retry}; the close table says ${retry}`,
      });
    }
    return { ...e, retry };
  }
  private readonly retryMismatches = new Set<string>();

  private retryLater(): void {
    if (this.destroyed || this._state === "closed") return;
    this.clearRetry();
    const r = this.opts.reconnect || { minMs: 1000, maxMs: 15000 };
    const cap = Math.min(r.maxMs, r.minMs * 2 ** Math.min(this.attempts, 20));
    const delay = Math.max(r.minMs, cap / 2 + (this.timers.random() * cap) / 2);
    this.attempts++;
    if (this._state === "connecting" && !this.live) this.setState("connecting");
    else this.setState("reconnecting");
    this.retryTimer = this.timers.set(() => {
      this.retryTimer = null;
      this.attempt(true);
    }, delay);
  }

  private clearRetry(): void {
    if (this.retryTimer !== null) this.timers.clear(this.retryTimer);
    this.retryTimer = null;
  }

  private close(info: CloseEvent): void {
    if (this._state === "closed") return;
    this.closeInfo = info;
    this.abort?.abort();
    this.clearRetry();
    this.cancelSnapshot();
    this.live?.disconnected();
    this.setState("closed");
    this.applyReadOnly();
    this.emit("closed", info);
  }

  // ---- frames

  /** What `LiveText` sees: the transport, minus the editor's own snapshot frames. While no
   * connection is open LiveText sends no edits (`disconnected`); the transport drops the rest
   * (`since`, carets: the next welcome resyncs and sends the caret again). */
  private proxy(): Transport {
    return {
      send: (f: ClientFrame) => this.t.send(f),
      onFrame: (cb: (f: ServerFrame) => void) => {
        this.frameListeners.add(cb);
        return () => this.frameListeners.delete(cb);
      },
    };
  }

  private onHostFrame(f: ServerFrame): void {
    if (this.destroyed) return;
    switch (f.type) {
      case "snapshot":
        // the editor's own request (LiveText's `reload` isn't used: the editor falls back to a
        // fresh connect when the host can't answer)
        if (this.snapshot && f.op === this.snapshot.op) return this.onSnapshot(f.op, f.text, f.version);
        if (f.op >= SNAPSHOT_OP_BASE) return;
        break;
      case "closed":
        // the reason for the close that follows; LiveText reports it too (`error` frames are
        // LiveText's to report)
        this.closedFrame = {
          reason: f.reason,
          retry: canRetry(f.reason),
          ...(f.code !== undefined ? { code: f.code } : {}),
          ...(f.path !== undefined ? { path: f.path } : {}),
          ...(f.error ? { error: f.error } : {}),
        };
        break;
      case "refused":
        if (this.snapshot && f.op === this.snapshot.op) return this.snapshotFailed();
        if (f.op >= SNAPSHOT_OP_BASE) return; // a late reply to an abandoned snapshot
        break;
    }
    for (const cb of [...this.frameListeners]) cb(f as ServerFrame);
  }

  // ---- the replica

  private start(w: Welcome): void {
    const live = new LiveText({ text: w.text!, version: w.version }, this.proxy(), {
      root: this.root,
      self: w.self,
      client: w.client,
      // one clock for the editor and the replica: `lastActiveAt` is compared with it
      timers: this.timers,
      ...(this.opts.schedule ? { schedule: this.opts.schedule } : {}),
    });
    this.live = live;
    const binding = new Binding(live, {
      undo: () => this.undo(),
      redo: () => this.redo(),
      carets: () => this.caretViews(),
      preview: () => {
        if (this.previewId === null) return null;
        const p = live.insertHerePreview(this.previewId);
        return "refused" in p ? null : { at: p.at, text: p.text };
      },
      local: () => this.pendingChanged(),
      historyAdded: () => this.emit("error", { id: HISTORY_ERROR, message: HISTORY_MESSAGE }),
      repaired: (reason) => this.emit("error", { id: "MPC-E000", message: `the editor was reset to the live text: ${reason}` }),
    });
    this.binding = binding;
    this.view.setState(this.createState(live.text, binding.extension()));
    binding.attach(this.view);
    this.offs.push(
      live.onChange((e) => {
        this.emit("change", { hunks: e.hunks, by: e.by, local: e.local });
        this.pendingChanged();
      }),
      live.onAck((e) => {
        this.emit("ack", e);
        this.pendingChanged();
      }),
      live.onRejected((rej) => {
        const r = rej as EditorRejection;
        r.mayHaveApplied = this.markOf(r.id)?.mayHaveApplied === true;
        this.emit("rejected", r);
        if (r.error?.id === "MPC-E111") this.emit("conflict", { rejection: r, error: r.error });
        this.renderRejections();
        this.binding?.schedule();
        this.pendingChanged();
      }),
      live.onRoster((entries) => {
        if (!this.openingCarets) {
          this.openingCarets = new Map(live.participants().filter((p) => p.caret && !p.isSelf).map((p) => [caretKey(p), p.lastActiveAt]));
        }
        this.emit("roster", entries);
        this.renderRoster();
      }),
      // carets and marks redraw from the library's data, at most once a delivery (a frame)
      live.onParticipants(() => {
        this.binding?.schedule();
        // after the redraw's flush: the positions are in the editor's document then
        queueMicrotask(() => this.scrollToFollowed());
      }),
      live.onMarks(() => this.binding?.schedule()),
      live.onRosterEvent((ev) => this.onRosterEvent(ev)),
      live.onLanded((l) => {
        this.landedIds.add(l.rejection.id);
        this.emit("landed", { rejection: l.rejection as EditorRejection, version: l.version });
        this.renderRejections();
        this.showNote(this.strings.landed);
        this.binding?.schedule();
      }),
      live.onDuplicateRisk((d) => {
        this.emit("duplicateRisk", { rejection: d.rejection as EditorRejection, landed: d.landed, reapplied: d.reapplied });
        this.showNote(this.strings.duplicateRisk);
      }),
      live.onReload((e) => {
        this.emit("reload", e);
        this.renderRejections();
        this.requestSnapshot();
      }),
      live.onError((e) => this.emit("error", e)),
    );
    this.applyReadOnly();
  }

  private onRosterEvent(ev: RosterEvent): void {
    const kind = (ev as { kind: string }).kind;
    if (kind === "overwritten") return this.onNote(ev as unknown as RemovedTextNote);
    if (kind === "snapshot") {
      const notes = (ev as { overwritten?: RemovedTextNote[] }).overwritten ?? [];
      for (const n of notes) this.onNote({ ...n, kind: "overwritten" });
      return;
    }
    if (kind === "claim_started" || kind === "claim_ended") return this.emit("claim", ev as unknown as ClaimEvent);
    if (kind === "rights" && "who" in ev && ev.who === this.self) {
      const r = (ev as { rights: string }).rights;
      return this.setRights(r === "edit" ? "edit" : r === "none" ? "none" : "read");
    }
    if (kind === "closed") {
      const reason = (ev as unknown as { reason?: string }).reason;
      this.closedFrame = { reason: "closed", retry: false };
      if (reason) this.closedFrame.error = { id: "MPC-E000", message: reason };
    }
  }

  private onNote(n: RemovedTextNote): void {
    if (!this.self || n.to !== this.self) return;
    const key = `${n.change}\u0000${n.row ?? ""}`;
    if (this.seenNotes.has(key)) return;
    this.seenNotes.add(key);
    this.emit("note", n);
  }

  private requestSnapshot(): void {
    if (this.snapshot || this._state !== "live") return; // not live: the next welcome resyncs
    const op = this.nextSnapshotOp++;
    const timer = this.timers.set(() => this.snapshotFailed(), this.opts.snapshotTimeoutMs ?? 10000);
    this.snapshot = { op, timer };
    this.t.send({ type: "snapshot", op });
  }

  private onSnapshot(op: number, text: string, version: number): void {
    if (!this.snapshot || this.snapshot.op !== op || !this.live) return;
    this.cancelSnapshot();
    this.live.reset({ text, version });
    this.renderRejections();
  }

  private snapshotFailed(): void {
    this.cancelSnapshot();
    if (this.destroyed || this._state === "closed") return;
    // the host can't send one: a fresh connect brings the text
    this.attempt(false);
  }

  private cancelSnapshot(): void {
    if (this.snapshot) this.timers.clear(this.snapshot.timer);
    this.snapshot = null;
  }

  // ---- read-only and rights

  private setRights(r: Rights): void {
    if (r === this._rights) return this.applyReadOnly();
    this._rights = r;
    this.emit("rights", r);
    this.applyReadOnly();
  }

  private isReadOnly(): boolean {
    return !!this.opts.readOnly || this._rights !== "edit" || this._state === "closed" || !this.live;
  }

  private applyReadOnly(): void {
    if (this.destroyed) return;
    const ro = this.isReadOnly();
    this.view.dispatch({ effects: this.readOnlyConf.reconfigure(EditorState.readOnly.of(ro)) });
    this.view.contentDOM.setAttribute("aria-readonly", String(ro));
    this.renderBanner();
  }

  private createState(doc: string, binding: Extension = []): EditorState {
    const mode = this.opts.mode ?? "markdown";
    return EditorState.create({
      doc,
      extensions: [
        binding, // first: its undo keys and filters come before the host's
        this.readOnlyConf.of(EditorState.readOnly.of(this.isReadOnlyInitially())),
        mode === "markdown" ? [markdown(), syntaxHighlighting(defaultHighlightStyle, { fallback: true })] : [],
        keymap.of(defaultKeymap),
        EditorView.lineWrapping,
        EditorView.contentAttributes.of({ "aria-multiline": "true", role: "textbox" }),
        EditorView.editorAttributes.of({ class: `mpc-labels-${this.labels}` }),
        editorTheme,
        this.opts.extensions ?? [],
      ],
    });
  }

  private isReadOnlyInitially(): boolean {
    // before the first welcome nothing may be typed
    return this.live === undefined || this.live === null ? true : this.isReadOnly();
  }

  // ---- carets and labels

  /** The carets to draw: every other connection with a caret (`live.participants()`, never
   * this one), its name shown per `labels`. */
  private caretViews(): CaretView[] {
    const live = this.live;
    if (!live) return [];
    const now = this.timers.now();
    let hideAt = Infinity;
    const out: CaretView[] = [];
    for (const p of live.participants()) {
      if (p.isSelf || !p.caret) continue;
      const active = this.labels === "active" && this.recentlyActive(p, now);
      if (active) hideAt = Math.min(hideAt, p.lastActiveAt + ACTIVE_LABEL_MS);
      out.push({
        handle: p.handle,
        ...(p.client !== undefined ? { client: p.client } : {}),
        // this person's own other tab or device: "You · other tab", or the host's label for it
        name: p.sameUser ? (p.label ? this.strings.youOn.replace("{label}", p.label) : this.strings.youElsewhere) : p.name,
        colour: p.colour,
        head: p.caret.head,
        selections: p.selections,
        sameUser: p.sameUser,
        away: p.away,
        label: this.labels === "always" || active,
      });
    }
    if (this.labels === "active") this.hideLabelsAt(hideAt);
    return out;
  }

  /** A caret moved (or its connection typed) in the last `ACTIVE_LABEL_MS` (`lastActiveAt`, on
   * this editor's clock); a caret that was there when the doc opened only once it has moved. */
  private recentlyActive(p: Participant, now: number): boolean {
    const key = caretKey(p);
    const opening = this.openingCarets?.get(key);
    if (opening !== undefined) {
      if (p.lastActiveAt === opening) return false;
      this.openingCarets!.delete(key);
    }
    return now < p.lastActiveAt + ACTIVE_LABEL_MS;
  }

  /** Redraw when the first shown name is due to hide (`Infinity`: none shown). */
  private hideLabelsAt(at: number): void {
    if (this.labelTimer !== null) this.timers.clear(this.labelTimer);
    this.labelTimer = null;
    if (at === Infinity) return;
    this.labelTimer = this.timers.set(() => {
      this.labelTimer = null;
      this.binding?.schedule();
    }, at - this.timers.now() + 1);
  }

  private scrollToFollowed(): void {
    const live = this.live;
    const b = this.binding;
    // positions are in `live.text`: the editor's document unless changes are held
    if (!this.following || !live || !b || b.holding || this.destroyed) return;
    const p = live.participants().find((x) => x.handle === this.following && x.caret !== null);
    if (!p?.caret || p.caret.head > this.view.state.doc.length) return;
    this.view.dispatch({ effects: EditorView.scrollIntoView(p.caret.head, { y: "nearest" }) });
  }

  // ---- the frame's parts

  private renderBanner(): void {
    const r = this.opts.render?.banner;
    const banner = this.dom.banner;
    if (r === false) {
      banner.hidden = true;
      return;
    }
    let text = "";
    if (this._state === "closed") text = this.closeInfo ? this.strings.closed[this.closeInfo.reason] ?? this.strings.closed.closed : this.strings.closed.closed;
    else if (this._state === "reconnecting") text = this.strings.reconnecting;
    else if (this._state === "connecting") text = this.strings.connecting;
    else if (this._rights !== "edit" && !this.opts.readOnly) text = this.strings.readOnly;
    const info: BannerInfo = { state: this._state, rights: this._rights, close: this.closeInfo, text, retry: () => this.reconnectNow() };
    banner.replaceChildren();
    banner.dataset.state = this._state;
    if (r) {
      const el = r(info);
      banner.hidden = !el;
      if (el) banner.append(el);
      return;
    }
    banner.hidden = !text;
    if (!text) return;
    const doc = banner.ownerDocument;
    const span = doc.createElement("span");
    span.textContent = text;
    banner.append(span);
    if (this._state === "reconnecting") {
      const b = doc.createElement("button");
      b.type = "button";
      b.textContent = this.strings.retryNow;
      b.addEventListener("click", () => this.reconnectNow());
      banner.append(b);
    }
  }

  private renderRoster(): void {
    const r = this.opts.render?.roster;
    const el = this.dom.roster;
    if (r === false) {
      el.hidden = true;
      return;
    }
    const entries = this.roster;
    const click = (handle: Handle) =>
      this.emit("participantClick", { handle, entry: entries.find((e) => e.handle === handle) ?? null });
    el.replaceChildren();
    el.hidden = entries.length === 0;
    if (r) {
      el.append(r(entries, click));
      return;
    }
    const doc = el.ownerDocument;
    for (const e of entries) {
      const b = doc.createElement("button");
      b.type = "button";
      const self = e.self || e.handle === this.self;
      b.textContent = self ? `${e.name} ${this.strings.you}` : e.name;
      b.title = `${e.name} (${e.kind})`;
      b.dataset.handle = e.handle;
      b.dataset.away = String(e.away);
      b.style.setProperty("--mpc-user", e.colour);
      b.style.setProperty("--mpc-user-fg", readableOn(e.colour));
      b.addEventListener("click", () => click(e.handle));
      el.append(b);
    }
  }

  private renderRejections(): void {
    const r = this.opts.render?.rejection;
    const list = this.dom.rejections;
    const rejected = this.rejected;
    list.replaceChildren();
    list.hidden = r === false || rejected.length === 0;
    if (r === false) return;
    const doc = list.ownerDocument;
    if (this.previewId !== null && !rejected.some((x) => x.id === this.previewId)) {
      this.previewId = null; // it left (landed, or dismissed elsewhere)
      this.showNote("");
    }
    for (const rej of rejected) {
      const actions: RejectionActions = {
        reapply: () => this.reapply(rej.id),
        dismiss: () => this.dismiss(rej.id),
        insertHere: () => this.insertHere(rej.id),
        confirmInsert: () => (this.previewId === rej.id ? this.confirmInsert() : { status: "unavailable" }),
        cancelInsert: () => {
          if (this.previewId === rej.id) this.cancelInsert();
        },
        copy: () => this.copyRejected(rej.id),
        reveal: () => {
          const m = this.markOf(rej.id);
          const doc = this.view.state.doc;
          if (!m || !this.binding || this.binding.holding || m.range.from > doc.length) return;
          const line = doc.lineAt(m.range.from).number;
          this.reveal({ first: line, last: line });
        },
      };
      const li = doc.createElement("li");
      li.dataset.rejection = String(rej.id);
      if (r) {
        li.append(r(rej, actions));
      } else {
        const label = doc.createElement("span");
        label.textContent = this.strings.notApplied;
        const snippet = doc.createElement("span");
        snippet.className = "mpc-rejected-snippet";
        snippet.textContent = rej.text || "—";
        snippet.title = rej.text;
        const again = doc.createElement("button");
        again.type = "button";
        again.className = "mpc-reapply";
        again.textContent = this.strings.reapply;
        again.disabled = !rej.text;
        again.addEventListener("click", () => actions.reapply());
        const dismiss = doc.createElement("button");
        dismiss.type = "button";
        dismiss.className = "mpc-dismiss";
        dismiss.textContent = this.strings.dismiss;
        dismiss.addEventListener("click", () => actions.dismiss());
        snippet.addEventListener("click", () => actions.reveal());
        li.append(label, snippet);
        if (rej.mayHaveApplied) {
          const maybe = doc.createElement("span");
          maybe.className = "mpc-may-have-applied";
          maybe.textContent = this.strings.mayHaveApplied;
          li.append(maybe);
        }
        const button = (cls: string, text: string, run: () => void) => {
          const btn = doc.createElement("button");
          btn.type = "button";
          btn.className = cls;
          btn.textContent = text;
          btn.addEventListener("click", run);
          return btn;
        };
        if (this.previewId === rej.id) {
          li.dataset.previewing = "true";
          li.append(
            button("mpc-insert-confirm", this.strings.insertConfirm, () => actions.confirmInsert()),
            button("mpc-insert-cancel", this.strings.insertCancel, () => actions.cancelInsert()),
          );
          li.addEventListener("keydown", (e) => {
            if (e.key === "Escape") {
              e.preventDefault();
              actions.cancelInsert();
            }
          });
        } else {
          li.append(again);
          if (this.offersInsertHere(rej.id)) li.append(button("mpc-insert-here", this.strings.insertHere, () => actions.insertHere()));
        }
        if (rej.text) li.append(button("mpc-copy", this.strings.copy, () => void actions.copy()));
        li.append(dismiss);
      }
      list.append(li);
    }
  }

  /** A row offers "Insert my text here" when the library would place it: typed text, one hunk.
   * A lost reply's record (`already-present`, `cant-tell`) is checked when it's used, and says
   * why then. */
  private offersInsertHere(id: number): boolean {
    const p = this.live?.insertHerePreview(id);
    return !!p && (!("refused" in p) || p.refused === "already-present" || p.refused === "cant-tell");
  }

  // ---- events

  private setState(s: ConnectionState): void {
    if (s === this._state) return;
    this._state = s;
    this.dom.frame.dataset.state = s;
    this.emit("state", s);
    if (this.live) this.applyReadOnly();
    else this.renderBanner();
  }

  private pendingChanged(): void {
    const n = this.pending;
    if (n === this.lastPending) return;
    this.lastPending = n;
    this.emit("pending", n);
  }

  private emit<K extends keyof LiveEditorEvents>(k: K, v: LiveEditorEvents[K]): void {
    const set = this.ls.get(k) as Set<(v: LiveEditorEvents[K]) => void> | undefined;
    if (!set) return;
    for (const cb of [...set]) {
      try {
        cb(v);
      } catch (e) {
        if (k !== "error") this.emit("error", { id: "MPC-E000", message: `a ${k} listener threw: ${String(e)}` });
      }
    }
  }
}

