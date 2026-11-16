// Who's on a live file, as the shell draws them: the avatar stack, Watching,
// and the names on changes. Pure, so the counts are tested without a browser.

import type { TimePositionJson } from '@k2/mpc-client'

export interface LivePerson {
  /** The kit's handle (`atlas`, `sam`). */
  id: string
  name: string
  kind: 'person' | 'agent'
  /** Their colour, from the data (the kit's actor colour). */
  colour: string
  away: boolean
  /** The viewer themselves. */
  self?: boolean
  /** What they're doing, short ("holding 2 clips", "in the file, not editing"). */
  doing?: string | null
  /** The full reason behind `doing`, for its tooltip (a hold's quoted note). */
  doingTip?: string | null
  /** Where they are, in words ("Table, framed", "Interview B on V1"). */
  place?: string | null
  /** The kit's presence: their view address and playhead, once the host lists
   *  `view_address` and `time_position` in `welcome.features`. */
  viewAddress?: string | null
  timePosition?: TimePositionJson | null
  /** An agent working right now (its avatar may pulse). */
  editing?: boolean
  /** The view's presence slot for them (`--color-presence-1..4`): the colour their marks take on the file, so their avatar matches. None: the data colour. */
  presence?: 1 | 2 | 3 | 4
}

export interface Watching {
  /** Here now: you first, then people, then agents. */
  here: LivePerson[]
  away: LivePerson[]
  /** "Watching N": only people who are here, you included. */
  count: number
}

const rank = (p: LivePerson): number => (p.self ? 0 : p.kind === 'person' ? 1 : 2)

export function watching(people: readonly LivePerson[]): Watching {
  const here = people.filter((p) => !p.away).sort((a, b) => rank(a) - rank(b))
  const away = people.filter((p) => p.away && !p.self).sort((a, b) => rank(a) - rank(b))
  return { here, away, count: here.length }
}

/** Everyone but you, for the header's stack: `max` places, the last one a
 *  `+N` when there are more. Away people stay in it (the view fades them).
 *  The editors' stack (`editor`): you first, then who's here; no away people. */
export function avatarStack(people: readonly LivePerson[], max = 4, editor = false): { shown: LivePerson[]; more: LivePerson[] } {
  const others = editor ? [...people.filter((p) => p.self), ...watching(people).here.filter((p) => !p.self)] : people.filter((p) => !p.self)
  if (others.length <= max) return { shown: others, more: [] }
  return { shown: others.slice(0, max - 1), more: others.slice(max - 1) }
}

export function personById(people: readonly LivePerson[], id: string | null | undefined): LivePerson | undefined {
  return id ? people.find((p) => p.id === id) : undefined
}

/** "just now", "2 min ago", else the clock (HH:MM, in `timeZone`). */
export function agoText(at: number, now: number, timeZone?: string): string {
  const d = Math.max(0, now - at) / 1000
  if (d < 60) return 'just now'
  if (d < 3600) return `${Math.floor(d / 60)} min ago`
  return new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit', hour12: false, timeZone }).format(new Date(at))
}

export const plural = (n: number, word: string): string => `${n} ${word}${n === 1 ? '' : 's'}`
