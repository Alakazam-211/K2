// Holds on a live text (build-editors.md E4b; editors-notes.md §2 Holds,
// v3.1, v3.2): the kit's claims on lines (`claim_started {lines, note, hold,
// actor}` / `claim_ended`, which the Live editor passes on as `claim`) as the
// editor draws them: a soft tint in the holder's presence colour, a bar in
// the gutter, a label ("Atlas is rewriting Setup") with "Message Atlas", and,
// while your caret is inside, one line under it saying you can still type.
//
// A hold never blocks a person (K2 Cores, v3.1): your words go in at once and
// the holder keeps them. Your own claims aren't drawn. Pure: the reducer and
// the words; lib/editor/live/hold-field.ts puts them on the text.

import type { Slot } from './presence-slots'

export interface Hold {
  claim: string
  who: string
  name: string
  agent: boolean
  slot: Slot
  /** Lines `first..last` (1-based) when the claim was taken. */
  first: number
  last: number
  /** The holder's own words for what they're doing ("rewriting Setup"). */
  note: string | null
  /** A hold (true) or a plain claim: both are drawn; neither blocks you. */
  hold: boolean
}

export interface HoldsContext {
  self: string
  people: readonly { id: string; name: string; kind: string; self?: boolean }[]
  slot: (id: string) => Slot
}

type ClaimLike = { kind: string; who?: string; claim?: string; lines?: { first: number; last: number }; note?: unknown; hold?: unknown; actor?: { id?: string; name?: string; kind?: string }; [k: string]: unknown }

export function holdsReducer(holds: Hold[], ev: ClaimLike, ctx: HoldsContext): Hold[] {
  if (ev.kind === 'claim_ended') {
    const next = holds.filter((h) => h.claim !== ev.claim)
    return next.length === holds.length ? holds : next
  }
  if (ev.kind !== 'claim_started' || !ev.who || !ev.claim || !ev.lines) return holds
  if (ev.who === ctx.self || ctx.people.some((p) => p.id === ev.who && p.self)) return holds
  const person = ctx.people.find((p) => p.id === ev.who)
  const first = Math.max(1, Math.min(ev.lines.first, ev.lines.last))
  const last = Math.max(first, ev.lines.last)
  const hold: Hold = {
    claim: ev.claim,
    who: ev.who,
    name: ev.actor?.name?.trim() || person?.name || ev.who,
    agent: (ev.actor?.kind ?? person?.kind) === 'agent',
    slot: ctx.slot(ev.who),
    first,
    last,
    note: typeof ev.note === 'string' && ev.note.trim() ? ev.note.trim() : null,
    hold: ev.hold !== false,
  }
  return [...holds.filter((h) => h.claim !== ev.claim), hold]
}

/** The holds of people still in the file. */
export function pruneHolds(holds: Hold[], present: readonly string[]): Hold[] {
  const next = holds.filter((h) => present.includes(h.who))
  return next.length === holds.length ? holds : next
}

/** "Atlas is rewriting Setup" (the holder's own -ing words), else "Atlas is working on Setup". */
export function holdLabel(h: Hold, section: string | null): string {
  const first = h.note?.split(/\s+/)[0] ?? ''
  if (h.note && /^[a-z]+ing$/i.test(first)) return `${h.name} is ${first.toLowerCase()}${h.note.slice(first.length)}`
  return `${h.name} is working on ${section ?? (h.first === h.last ? `line ${h.first}` : `lines ${h.first}–${h.last}`)}`
}

/** The line under your caret inside a hold. */
export const holdHint = (h: Hold, section: string | null): string => `${holdLabel(h, section)}. You can still type here; your words go in now, and ${h.name} sees them and keeps them.`
