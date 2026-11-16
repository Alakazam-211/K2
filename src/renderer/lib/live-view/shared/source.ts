// Where a live tab's shell gets its data: one small interface every live
// view's source implements. The shell's pieces are presentational (props
// in, callbacks out); the tab reads a LiveSource and hands them what it
// says. The sample sources implement it now (sample-shell.ts); the
// kit-backed ones (a keyed watch plus the kit's PresenceChannel on a doc
// socket) implement it later, and the shell doesn't change.
//
// Shaped for what K2 is adding on the host:
//   - `welcome.features` listing `view_address` and `time_position`:
//     `features.viewAddress` / `features.timePosition`, and each person's
//     `viewAddress` / `timePosition` (people.ts) once listed;
//   - `outside_save.copy.doc` (Host::document_at): `OutsideSave` is the kit's
//     frame, so `copy.doc` arrives as is (Open the copy uses it, else the path);
//   - the "say something" route and its reply frame: `say()` here, and
//     `messages` for the replies; `features.say` false hides nothing but
//     makes Send fail honestly;
//   - an agent's `viewAddress` (the still it last asked for): just another
//     person's `viewAddress`.

import type { LivePerson } from './people'
import type { ConnectionState, OutsideSave } from './notices'
import type { LiveTier } from './state'

export interface LiveChange {
  id: string
  /** Who made it (a person's id; `file` for a save made outside K2). */
  who: string
  /** Its one verb ("trimmed", "moved"), the same word in the sentence. */
  verb: string
  /** The sentence after the name: "trimmed the start of Interview B by 1s". */
  text: string
  at_ms: number
  /** Where it is (an address), for Go there; null when there's nowhere to go. */
  target: string | null
  /** Undo can revert it (the editors' Changes; the watch views don't offer it). */
  undoable?: boolean
  /** A refused verb or an outside save, drawn quieter. */
  tone?: 'refused' | 'outside'
  /** You undid it (the editors): the row turns quiet and offers Redo. */
  undone?: boolean
  /** A second action asking someone (a refusal's holder): "Ask Sam". */
  ask?: { id: string; name: string }
}

export interface LiveMessage {
  id: string
  from: string
  to: string
  text: string
  at_ms: number
  /** What it pointed at: a label for people, the address for the agent. */
  address?: { label: string; address: string } | null
}

export interface LiveFeatures {
  /** The host lists `view_address` in `welcome.features`. */
  viewAddress: boolean
  /** The host lists `time_position` in `welcome.features`. */
  timePosition: boolean
  /** The host has the say route. */
  say: boolean
}

export interface LiveShellSnapshot {
  tier: LiveTier
  connection: ConnectionState | null
  /** For "Back.": how many changes came in while away. */
  caughtUp?: number
  /** For "can't reconnect": a retry just failed. */
  retried?: boolean
  /** The viewer's own id among `people`. */
  self: string
  people: LivePerson[]
  /** The agent at work (the default Follow target). */
  agent: string | null
  agentDone: boolean
  /** The header's one sentence: who, and the rest of it. */
  activity: { who: string | null; text: string } | null
  /** Newest last. */
  changes: LiveChange[]
  messages: LiveMessage[]
  /** The latest outside save, with when it came. */
  outsideSave: (OutsideSave & { at_ms: number }) | null
  /** The latest refused verb, by name. */
  refusal?: { actor: string; verb: string; target: string; holder: string | null; at_ms: number } | null
  /** The picture lags the live file ("Live · picture behind"). */
  behind?: boolean
  features: LiveFeatures
  /** The time this snapshot's times read against ("2 min ago"); absent: now. */
  clock?: number
}

export type SayResult = { ok: true } | { ok: false }

export interface LiveSource {
  /** `sample` draws the "Sample data" banner; `live` is a real doc. */
  readonly kind: 'sample' | 'live'
  snapshot(): LiveShellSnapshot
  /** Every new snapshot. Returns the unsubscribe. */
  subscribe(cb: (s: LiveShellSnapshot) => void): () => void
  /** Reconnect now (Retry, Retry now). */
  retry(): void
  /** Send a message to a participant, pointing at an address. */
  say(msg: { to: string; text: string; address: string | null }): Promise<SayResult>
  /** Tell the others what you're looking at (when `features.viewAddress`). */
  setViewAddress?(address: string | null): void
  close(): void
}
