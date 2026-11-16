// Who acts on a timeline: the frames' actors and the roster's people, by
// handle. K2's choice the wire leaves to it: an outside save (`kind: file`,
// colour null) shows as "Saved outside K2" in K2's grey.

import type { Actor } from './schema'

export const EXT_ID = 'kit:external'
export const OUTSIDE_NAME = 'Saved outside K2'
export const OUTSIDE_GREY = '#8a8f98'

const ACTORS = new Map<string, Actor>()

function shaped(a: Actor): Actor {
  const o = { id: a.id, name: a.name, kind: a.kind, colour: a.colour }
  if (o.kind === 'file') {
    o.name = OUTSIDE_NAME
    if (!o.colour) o.colour = OUTSIDE_GREY
  }
  return o
}

/** Remember an actor; the first seen under a handle stays. */
export function addActor(a: Actor | null | undefined): void {
  if (a && a.id && !ACTORS.has(a.id)) ACTORS.set(a.id, shaped(a))
}

/** An actor by handle or as a frame names it; one never seen keeps its own fields. */
export function actorOf(a: string | Actor | null | undefined): Actor {
  const id = typeof a === 'string' ? a : a?.id
  const known = id ? ACTORS.get(id) : undefined
  if (known) return known
  if (a && typeof a === 'object') return shaped(a)
  return { id: String(a), name: String(a), kind: 'person', colour: '#888' }
}
