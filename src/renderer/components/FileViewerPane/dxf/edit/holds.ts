// Agents' holds on a selection. A hold's keys are the CAD pack's index
// keys, always `@h:<HANDLE>` (upper-case hex, no leading zeros): an
// entity's, or a layer record's, a block record's or a layout's, which
// covers everything on or in it (model space's covers all of model space);
// `@doc:dxf` is the whole drawing. Layer and block names come to handles
// through the drawing's header. A person isn't refused by a hold, so before
// an edit that touches one the viewer asks once, naming who and what for.

import { namesOf } from '../cad-change-words'
import type { ModelHeader, ModelRow } from '../model/types'
import type { Hold } from './edit-source'

const norm = (k: string): string => (/^@h:/i.test(k) ? `@h:${k.slice(3).replace(/^0+(?=.)/, '').toUpperCase()}` : k)

type HeaderKeys = Pick<ModelHeader, 'layers' | 'layouts'>

function keyMaps(header: HeaderKeys) {
  return {
    layerKey: new Map(header.layers.map((l) => [l.name, norm(l.handle)])),
    modelKeys: header.layouts.filter((l) => l.kind === 'model').flatMap((l) => [norm(l.handle), l.space]),
  }
}

/** The whole drawing's key. */
export const DOC_KEY = '@doc:dxf'

/** Every key whose hold covers row `r`: its own, its layer's, its space's, the drawing's. */
function rowKeys(r: ModelRow, m: ReturnType<typeof keyMaps>): string[] {
  const out = [norm(r.h), norm(r.owner), DOC_KEY]
  const lk = m.layerKey.get(r.layer)
  if (lk) out.push(lk)
  if (r.owner === 'model') out.push(...m.modelKeys)
  return out
}

/** The holds that cover any of `handles`, in the order given. */
export function holdsOn(handles: readonly string[], holds: readonly Hold[], rows: readonly ModelRow[], header: HeaderKeys): Hold[] {
  if (!holds.length || !handles.length) return []
  const m = keyMaps(header)
  const want = new Set(handles.map(norm))
  const covered = new Set([...want, ...rows.filter((r) => want.has(norm(r.h))).flatMap((r) => rowKeys(r, m))])
  return holds.filter((hd) => hd.keys.some((k) => covered.has(norm(k))))
}

/** Each row a hold covers, with its first holder's pid: what the drawing marks as held. */
export function heldHandles(holds: readonly Hold[], rows: readonly ModelRow[], header: HeaderKeys): Map<string, string> {
  const out = new Map<string, string>()
  if (!holds.length) return out
  const m = keyMaps(header)
  const holder = new Map<string, string>()
  for (const hd of holds) for (const k of hd.keys) if (!holder.has(norm(k))) holder.set(norm(k), hd.pid)
  for (const r of rows) {
    const pid = rowKeys(r, m)
      .map((k) => holder.get(k))
      .find((x) => x !== undefined)
    if (pid) out.set(r.h, pid)
  }
  return out
}

/** The one question before an edit over holds: who, what for, then go ahead or not. */
export function holdPrompt(holds: readonly Hold[], count: number, verb: 'move' | 'relabel'): string {
  return `${holdLine(holds, count)} ${verb === 'move' ? 'Move anyway?' : 'Change it anyway?'}`
}

/** "Atlas is working on these: moving the doors." for each holder. */
export function holdLine(holds: readonly Hold[], count: number): string {
  const these = count === 1 ? 'this' : 'these'
  const by = new Map<string, { name: string; notes: string[] }>()
  for (const h of holds) {
    const e = by.get(h.pid) ?? { name: h.name, notes: [] }
    if (h.note && !e.notes.includes(h.note)) e.notes.push(h.note)
    by.set(h.pid, e)
  }
  return [...by.values()].map((e) => `${e.name} is working on ${these}${e.notes.length ? `: ${namesOf(e.notes)}` : ''}.`).join(' ')
}
