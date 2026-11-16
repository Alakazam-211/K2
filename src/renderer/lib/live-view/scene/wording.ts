// Who acts, and what they did in words. K2 writes the words from the
// actor's name, the verb and the ops, by name at the change's rev: "moved
// Table", "turned and resized Rug", "renamed Vase to Tall vase". A refusal
// says who couldn't do what, and why, in plain words; its code goes only in
// the details. People's colours are data (the kit's actor colours).

import { GOLD } from './samples'
import { same, xformParts } from './math'
import { PROPS, UNITS, nameOf } from './fold'
import type { Actor, Doc, Frame, Refusal } from './types'

export const ATLAS = 'agent:atlas'
export const BROOK = 'agent:brook'
export const ADA = 'person:ada'
export const SAM = 'person:sam'

export const ACTORS: Record<string, Actor> = {}
const addActor = (a: Partial<Actor> | null | undefined): void => {
  if (a && a.id && !ACTORS[a.id]) ACTORS[a.id] = { id: a.id, name: a.name ?? a.id, kind: a.kind ?? 'person', colour: a.colour ?? '' }
}
Object.values(GOLD.turn.actors).forEach(addActor)
GOLD.pc.start.forEach((q) => addActor({ id: q.handle, name: q.name, kind: q.kind, colour: q.colour }))
// K2's choice: kit:external has no name or colour.
for (const a of Object.values(ACTORS))
  if (a.kind === 'file') {
    a.name = 'Saved outside K2'
    if (!a.colour) a.colour = '#8a8f98'
  }
addActor({ id: BROOK, name: 'Brook', kind: 'agent', colour: '#3fa37a' })

export function actorOf(a: string | Actor | null | undefined): Actor {
  const id = typeof a === 'string' ? a : a?.id
  return (id && ACTORS[id]) || (typeof a === 'object' && a) || { id: String(a), name: String(a), kind: 'person', colour: '#888' }
}

export interface Touched {
  addr: string
  fields: Record<string, { before: string | null | undefined; after: string | null | undefined }>
  added: boolean
  removed: boolean
}

export function changesOf(f: Frame): Touched[] {
  const by: Record<string, Touched> = {}
  const order: string[] = []
  const touch = (a: string): Touched => {
    if (!by[a]) {
      by[a] = { addr: a, fields: {}, added: false, removed: false }
      order.push(a)
    }
    return by[a]
  }
  for (const op of f.ops) {
    if (op.kind === 'list' && op.root === UNITS) {
      const t = touch(op.key)
      if (op.op === 'insert' || op.op === 'restore') t.added = true
      if (op.op === 'delete') t.removed = true
    } else if (op.kind === 'map' && op.root === PROPS[UNITS] && op.field) {
      const t = touch(op.key)
      if (!t.added && !t.removed) t.fields[op.field] = { before: op.before, after: op.after }
    }
  }
  return order.map((a) => by[a])
}

export function phraseOne(c: Touched, D0: Doc, D1: Doc): { text: string; kind: 'remove' | 'add' | 'place' | 'rename' | 'vis' } {
  const nm0 = nameOf(D0, c.addr)
  const nm1 = nameOf(D1, c.addr)
  const F = c.fields
  const out: string[] = []
  if (c.removed) return { text: `removed ${nm0}`, kind: 'remove' }
  if (c.added) return { text: `added ${nm1}`, kind: 'add' }
  if (F.name) out.push(`renamed ${F.name.before} to ${F.name.after}`)
  if (F.xform) {
    const a = xformParts(F.xform.before)
    const b = xformParts(F.xform.after)
    const diff = (k: 'loc' | 'rot' | 'scale'): boolean => !!a && !!b && !same(a[k], b[k])
    const v = [diff('loc') && 'moved', diff('rot') && 'turned', diff('scale') && 'resized'].filter((x): x is string => !!x)
    out.push(`${v.length > 1 ? v.slice(0, -1).join(', ') + ' and ' + v[v.length - 1] : v[0] || 'placed'} ${nm1}`)
  }
  const bits = ([['hidden', 'the viewport'], ['norender', 'renders']] as const).filter(([k]) => F[k])
  if (bits.length) {
    const off = bits.filter(([k]) => F[k].after === '1').map((b) => b[1])
    const on = bits.filter(([k]) => F[k].after !== '1').map((b) => b[1])
    if (off.length) out.push(`hid ${nm1} in ${off.join(' and ')}`)
    if (on.length) out.push(`showed ${nm1} in ${on.join(' and ')}`)
  }
  if (F.noselect) out.push(F.noselect.after === '1' ? `made ${nm1} unselectable` : `made ${nm1} selectable`)
  return { text: out.join(', ') || `changed ${nm1}`, kind: F.xform ? 'place' : F.name ? 'rename' : 'vis' }
}

export function framePhrase(f: Frame, D0: Doc, D1: Doc): { who: string; sep: string; text: string; target: string | null; addrs: string[] } {
  const a = actorOf(f.actor)
  const file = a.kind === 'file'
  const cs = changesOf(f)
  const parts = cs.map((c) => phraseOne(c, D0, D1).text)
  const text = parts.length ? (parts.length > 3 ? parts.slice(0, 2).join(', ') + `, +${parts.length - 2} more` : parts.join(', ')) : 'changed the scene'
  const target = cs.find((c) => !c.removed)
  return { who: a.name, sep: file ? ': ' : ' ', text, target: target ? target.addr : null, addrs: cs.map((c) => c.addr) }
}

export const VERB_WORD: Record<string, string> = { 'object place': 'move', 'object rename': 'rename', 'object visibility': 'hide or show' }
const WHY: Record<string, string> = { blend_driven: 'animation drives it', blend_rotation_mode: 'it turns by quaternion', blend_not_editable: 'it’s linked from another file' }

/** Plain words for a refusal: who couldn't do what, and why. The code goes only in the detail. */
export function refusalText(r: Refusal, D: Doc): { who: string; text: string; line: string; detail: string } {
  const who = actorOf(r.actor).name
  const what = VERB_WORD[r.verb] || 'change'
  const name = nameOf(D, r.unit)
  const why = r.holder ? `${actorOf(r.holder).name} is holding it` : WHY[r.code] || 'it can’t change right now'
  return { who, text: `couldn’t ${what} ${name}: ${why}`, line: `${who} couldn’t ${what} ${name}: ${why}`, detail: [r.code, r.verb, r.unit, r.claim].filter(Boolean).join(' · ') }
}
