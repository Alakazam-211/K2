// Presence and marks as data (`LiveText.participants()`, `LiveText.marks()`): what a host needs
// to draw other people's carets and the person's rejected changes in its own editor UI. No
// DOM: the library holds the data and says when it changed; the host draws. See PRESENCE.md.
//
// Offsets here are UTF-16 code units of the replica's current text (what CodeMirror and a JS
// string index use), not the UTF-8 bytes the wire and `carets()` use.

import type { OpId, Presence } from "./live";
import type { ErrorReport, Handle } from "./wire";

/**
 * A participant's kind, stable for a host to switch on. The roster's `kind` (the kit's
 * `ParticipantKind` in lower case) maps: `person` to `person`, `agent` to `agent`, and every
 * other value (`app`, `external`, `restore`, `import`, and any added later) to `app`: a program
 * acting on the doc, not a person or an agent typing. `Participant.rawKind` keeps the roster's
 * own value.
 */
export type ParticipantKind = "person" | "agent" | "app";

/** A caret or selection in UTF-16 code units of the current text (`anchor` stays put while
 * `head` moves; equal for a bare caret). */
export interface Range16 {
  anchor: number;
  head: number;
}

/** One connection in the doc (`LiveText.participants()`). */
export interface Participant {
  /** The participant's handle (the roster's `handle`, a change event's `pid`). */
  handle: Handle;
  /** The connection (`Device.client`), from a host that sends per-connection carets; absent
   * from an older host (one entry a participant) and on a participant's caret-less entry. */
  client?: number;
  name: string;
  /** The roster's `colour`, exactly as the host sent it (one source of truth, no
   * conversion): a stable identity value for the host to map to its own palette, not a style
   * instruction. */
  colour: string;
  kind: ParticipantKind;
  /** The roster's own `kind` string (`person`, `agent`, `app`, `external`, ...). */
  rawKind: string;
  /** This connection itself: listed once, with `caret: null` and no selections (the host
   * draws its own caret), so the host can show "you" in a list. */
  isSelf: boolean;
  /** Another connection of this connection's own participant (the same person in another tab
   * or on another device). */
  sameUser: boolean;
  /** The host's label for the connection (`Device.label`), when it gave one. Never doc text. */
  label?: string;
  /** The connection's caret in the current text; `null` when it has none to show. */
  caret: Range16 | null;
  /**
   * Every selection range of the connection. One element (the caret) today, since the wire
   * carries one range a connection; empty with no caret. Kept an array so a host that draws
   * from it keeps working when the wire carries several.
   */
  selections: Range16[];
  /**
   * When this connection's caret last moved or it last changed the text, on the replica's
   * clock (`LiveOptions.timers.now()`, default `Date.now()`); when it was first seen until then.
   * A caret the host re-sends unmoved (after other people's changes) doesn't count. For a host
   * to fade idle carets.
   */
  lastActiveAt: number;
  /** The roster's `away`. */
  away: boolean;
}

/** Where a rejected change stands for Re-apply (`placeRejection`): `ok` (it would go in),
 * `in-text` (all of it is in the text already: Re-apply only dismisses it), `conflict`,
 * `cant-place`. */
export type MarkPlacement = "ok" | "in-text" | "conflict" | "cant-place";

/** One rejected change, as data (`LiveText.marks()`). */
export interface Mark {
  /** The rejection's id (`Rejection.id`): what `reapply`, `dismiss` and `insertHere` take. */
  id: number;
  /** Where the change was, in UTF-16 code units of the current text (`Rejection.region`; it
   * moves with every change until dismissed). Empty (`from === to`) for a deletion, or for a
   * change the replica can't place (a reload since). */
  range: { from: number; to: number };
  /** The text it would have inserted (what the person typed); may be empty (a deletion). */
  text: string;
  /**
   * Why it was refused: the server's error id and message when it sent one (e.g. `MPC-E111`, a
   * write conflict; `MPC-E000`, the reply was lost). Without one: `MPC-E111` when the client saw
   * the change meet competing text, `MPC-E000` when the replica started again from the server's
   * text (a reload) before the change was answered.
   */
  reason: { code: string; message: string };
  /** The server's full refusal, when it sent one (`details` included). */
  error: ErrorReport | null;
  /** Its reply was lost (`MPC-E000` with a record to check): it may have landed after all. */
  mayHaveApplied: boolean;
  /** For a change whose reply was lost, `LiveText.presence(id)`; else `absent` (the server
   * refused it, or the client saw it would). */
  presence: Presence;
  /** `LiveText.placeRejection(id)`, summarized. */
  placement: MarkPlacement;
  /** `reapply(id)` would put the change back now: `presence` is `absent` and `placement` is
   * `ok`. */
  canReapply: boolean;
  /** `insertHere(id)` would insert the typed text now (one hunk, some text, `absent`). */
  canInsertHere: boolean;
  /** The change also removed text: "Insert my text here" puts back only the typed text, never
   * the removal. */
  removed: boolean;
}

/** Why "Insert my text here" can't go ahead: `unavailable` (no such rejection, no typed text,
 * or its place isn't a character boundary), `multi-hunk` (only a one-hunk change has one place
 * to insert at), `already-present` (its reply was lost and it landed), `cant-tell` (its reply
 * was lost and the record can't tell whether it landed). */
export type InsertRefusal = "unavailable" | "multi-hunk" | "already-present" | "cant-tell";

/** Where "Insert my text here" would put the typed text (`insertHerePreview`): `at` in UTF-16
 * code units of the current text. */
export type InsertPreview = { at: number; text: string; removed: boolean } | { refused: InsertRefusal };

/** What `insertHere` did: inserted `text` at `at` (UTF-16) as local edit `op`, or why not. */
export type InsertHereResult = { op: OpId; at: number; text: string } | { refused: InsertRefusal };

/**
 * Runs `deliver` later, once: the presence and marks events' batching (`LiveOptions.schedule`).
 * A browser host can leave the default; a host with no frames (an agent, a Node tool) can pass
 * `(deliver) => deliver()` for immediate delivery, or its own timer.
 */
export type FrameScheduler = (deliver: () => void) => void;

/** The default batch interval when there's no `requestAnimationFrame`, in ms. */
export const FRAME_MS = 16;

/** The roster's `kind` as a stable `ParticipantKind` (see there). */
export function participantKind(raw: string): ParticipantKind {
  const k = raw.toLowerCase();
  return k === "person" ? "person" : k === "agent" ? "agent" : "app";
}

/** The default scheduler: `requestAnimationFrame` when the global has one, else
 * `set(deliver, FRAME_MS)` (the replica's timers). */
export function defaultScheduler(set: (f: () => void, ms: number) => unknown): FrameScheduler {
  const g = globalThis as { requestAnimationFrame?: (cb: () => void) => unknown };
  if (typeof g.requestAnimationFrame === "function") return (deliver) => void g.requestAnimationFrame!(() => deliver());
  return (deliver) => void set(deliver, FRAME_MS);
}
