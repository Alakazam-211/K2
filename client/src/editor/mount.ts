// `mountLiveEditor`: a live CodeMirror editor for one text root, in plain DOM. It owns the
// connection's life (connect, welcome, live; on a drop, backoff and `connect({resume})`, then
// a resync; past the server's window, a snapshot), the binding, the carets, a roster strip,
// the rejections with their actions, and a banner. See EDITOR.md.

import { defaultKeymap, historyField } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { defaultHighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { Compartment, EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import type { Hunk } from "../hunk";
import { LiveText, type Rejection } from "../live";
import { canRetry } from "../wire";
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
} from "../wire";
import { Binding, type CaretView } from "./binding";
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
  /** After Re-apply refused a change with more than one hunk (nothing re-applied). */
  multiHunk: string;
  /** After Re-apply refused a change that deleted or replaced text (nothing re-applied). */
  notInsertion: string;
  /** After Re-apply refused a change from before a reload (nothing re-applied). */
  reloaded: string;
  /** After Re-apply refused a change something else was inserted beside since (nothing
   * re-applied). */
  tie: string;
  /** After Re-apply refused a change whose surrounding text was deleted or replaced since
   * (nothing re-applied). */
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
  roster: "People in this document",
  rejections: "Changes that weren't applied",
  offline: "Not connected: nothing was sent.",
  mayHaveApplied: "This change may already be in the document.",
  alreadyPresent: "This change is already in the document: nothing was re-applied.",
  landed: "A change that seemed lost went through after all.",
  duplicateRisk: "A change you re-applied had also gone through: it may now be in the document twice. Check it.",
  cantTell: "Can't tell whether this change is already in the document: nothing was re-applied. Check it, then type what's missing.",
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
 * - `applied`: the text went back in, as a new edit;
 * - `already-present`: (a `mayHaveApplied` change) it is in the text already; nothing applied,
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
   * - `multi-hunk`: more than one hunk (its text would go back in one place, not each hunk's);
   * - `not-insertion`: it deleted or replaced text (Re-apply only re-inserts typed text);
   * - `reloaded`: the editor reloaded since (its place moved by a diff, not the server's changes);
   * - `tie`: something else was inserted at its place since (the server would put it after
   *   that, Re-apply before);
   * - `deleted-around`: text around its place was deleted or replaced since (its place snapped
   *   to the deletion's start; the server refuses it, or never saw the deletion).
   */
  reason?: "cant-tell" | "multi-hunk" | "not-insertion" | "reloaded" | "tie" | "deleted-around";
};

export interface RejectionActions {
  reapply(): ReapplyResult;
  dismiss(): void;
  reveal(): void;
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
  /** Timers and randomness (tests drive them). */
  timers?: Timers;
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
  private caretsSeen = new Map<Handle, { key: string; until: number }>();
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
    if (!this.live) return;
    if (this._state !== "live") return this.emit("error", { id: "MPC-E000", message: this.strings.offline });
    this.live.undo();
  }

  redo(): void {
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
    const r = this.rejected.find((x) => x.id === id);
    if (!r) return unavailable;
    if (b.holding) return unavailable; // during a composition
    if (r.hunks.length > 1) {
      // refused rather than put in the wrong place (a later version restores each hunk in place)
      this.showNote(this.strings.multiHunk);
      return { status: "unknown", reason: "multi-hunk" };
    }
    if (r.mayHaveApplied) {
      // its reply was lost: Re-apply only when the server's record shows it never landed
      const p = live.presence(id);
      if (p === "present") {
        this.dismiss(id);
        this.showNote(this.strings.alreadyPresent);
        return { status: "already-present" };
      }
      if (p !== "absent") {
        this.showNote(this.strings.cantTell);
        return { status: "unknown", reason: "cant-tell" };
      }
    }
    // only a pure insertion, placed by the server's changes, with no tie, goes back where it was
    const g = live.reapplyGuard(id);
    if (g === null) return unavailable;
    if (g !== "ok") {
      const notes = {
        "multi-hunk": this.strings.multiHunk,
        "not-insertion": this.strings.notInsertion,
        reloaded: this.strings.reloaded,
        tie: this.strings.tie,
        "deleted-around": this.strings.deletedAround,
      } as const;
      const note = notes[g];
      this.showNote(note);
      return { status: "unknown", reason: g };
    }
    const pos = b.posOf(r.region.start);
    if (pos < 0) return unavailable;
    this.view.dispatch({
      changes: { from: pos, insert: r.text },
      selection: { anchor: pos + r.text.length },
      userEvent: "input.reapply",
    });
    if (r.mayHaveApplied) live.noteReapplied(id);
    this.dismiss(id);
    this.showNote("");
    return { status: "applied" };
  }

  /** A short status line under the rejections (empty: none). */
  private showNote(text: string): void {
    const n = this.dom.note;
    n.textContent = text;
    n.hidden = !text;
  }

  dismiss(id: number): void {
    this.live?.dismiss(id);
    this.renderRejections();
    this.binding?.schedule();
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
    const live = new LiveText({ text: w.text!, version: w.version }, this.proxy(), { root: this.root, self: w.self, client: w.client });
    this.live = live;
    const binding = new Binding(live, {
      undo: () => this.undo(),
      redo: () => this.redo(),
      carets: () => this.caretViews(),
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
        r.mayHaveApplied = r.error?.id === "MPC-E000" && r.asOf !== undefined;
        this.emit("rejected", r);
        if (r.error?.id === "MPC-E111") this.emit("conflict", { rejection: r, error: r.error });
        this.renderRejections();
        this.binding?.schedule();
        this.pendingChanged();
      }),
      live.onRoster((entries) => {
        this.noteCarets(entries);
        this.emit("roster", entries);
        this.renderRoster();
        this.binding?.schedule();
        queueMicrotask(() => this.scrollToFollowed());
      }),
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

  private noteCarets(entries: readonly RosterEntry[]): void {
    const now = this.timers.now();
    for (const e of entries) {
      const key = e.cursor ? `${e.cursor.root}:${e.cursor.anchor}:${e.cursor.head}:${e.cursor.version ?? ""}` : "";
      const was = this.caretsSeen.get(e.handle);
      if (!was || was.key !== key) this.caretsSeen.set(e.handle, { key, until: was ? now + ACTIVE_LABEL_MS : 0 });
    }
    if (this.labels === "active") this.scheduleLabelExpiry();
  }

  private scheduleLabelExpiry(): void {
    if (this.labelTimer !== null) this.timers.clear(this.labelTimer);
    this.labelTimer = null;
    const now = this.timers.now();
    let next = Infinity;
    for (const s of this.caretsSeen.values()) if (s.until > now) next = Math.min(next, s.until);
    if (next === Infinity) return;
    this.labelTimer = this.timers.set(() => {
      this.labelTimer = null;
      this.binding?.schedule();
      this.scheduleLabelExpiry();
    }, next - now + 1);
  }

  private caretViews(): CaretView[] {
    const live = this.live;
    if (!live) return [];
    const now = this.timers.now();
    return live.carets().map((c) => ({
      ...c,
      label: this.labels === "always" || (this.labels === "active" && (this.caretsSeen.get(c.handle)?.until ?? 0) > now),
    }));
  }

  private scrollToFollowed(): void {
    const live = this.live;
    const b = this.binding;
    if (!this.following || !live || !b || this.destroyed) return;
    const c = live.carets().find((x) => x.handle === this.following);
    if (!c) return;
    const pos = b.posOf(c.head);
    if (pos >= 0) this.view.dispatch({ effects: EditorView.scrollIntoView(pos, { y: "nearest" }) });
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
    for (const rej of rejected) {
      const actions: RejectionActions = {
        reapply: () => this.reapply(rej.id),
        dismiss: () => this.dismiss(rej.id),
        reveal: () => {
          const pos = this.binding?.posOf(rej.region.start) ?? -1;
          if (pos >= 0) {
            const line = this.view.state.doc.lineAt(pos).number;
            this.reveal({ first: line, last: line });
          }
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
        li.append(again, dismiss);
      }
      list.append(li);
    }
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

