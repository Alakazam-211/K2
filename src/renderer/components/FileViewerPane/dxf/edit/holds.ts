// Agents' holds on a selection. A hold's keys are index keys: an entity's
// `@h:<HANDLE>`, or a layer's, a block's or a layout's own handle, which
// covers everything on or in it (model space's covers all of model space);
// `@layer:<name>` is read too. A person isn't refused by a hold, so before
// an edit that touches one the viewer asks once, naming who and what for.

import { namesOf } from '../cad-change-words'
import type { ModelHeader, ModelRow } from '../model/types'
import type { Hold } from './edit-source'

const norm = (k: string): string => (/^@h:/i.test(k) ? `@h:${k.slice(3).replace(/^0+(?=.)/, '').toUpperCase()}` : k)

/** The holds that cover any of `handles`, in the order given. */
export function holdsOn(handles: readonly string[], holds: readonly Hold[], rows: readonly ModelRow[], header: Pick<ModelHeader, 'layers' | 'layouts'>): Hold[] {
  if (!holds.length || !handles.length) return []
  const byH = new Map(rows.map((r) => [norm(r.h), r]))
  const layerKey = new Map(header.layers.map((l) => [l.name, norm(l.handle)]))
  const modelKeys = new Set(header.layouts.filter((l) => l.kind === 'model').flatMap((l) => [norm(l.handle), l.space]))
  const keysOf = (h: string): Set<string> => {
    const r = byH.get(norm(h))
    const out = new Set([norm(h)])
    if (!r) return out
    const lk = layerKey.get(r.layer)
    if (lk) out.add(lk)
    out.add(`@layer:${r.layer}`)
    out.add(norm(r.owner))
    if (r.owner === 'model') for (const k of modelKeys) out.add(k)
    return out
  }
  const covered = handles.map(keysOf)
  return holds.filter((hd) => hd.keys.some((k) => covered.some((c) => c.has(norm(k)))))
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
