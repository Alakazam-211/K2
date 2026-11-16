// The one adapter: the kit's KeyedWatch state, the pack's pinned goldens
// and the sample's pages all give the same book, so the real timeline
// pack's frames drop in where the sample's are.

import type { KeyedRootState, KeyedState } from '@k2/mpc-client'
import { describe, expect, it } from 'vitest'
import { applyFrame, bookFromKeyedState, bookFromPages, bookFromPinned } from './adapter'
import { lanesOf, modelOf } from './fold'
import { GOLD } from './samples'
import { GB } from './scenarios-monitor'
import { BOOKS, FRAMES } from './world'

/** The sample's pages as the kit's KeyedWatch holds them after the snapshot. */
function keyedStateOf(roots: typeof GOLD.state.roots): KeyedState {
  const m = new Map<string, KeyedRootState>()
  for (const r of roots) {
    if (r.kind === 'list') m.set(r.name, { kind: 'list', live: (r.live || []).map((x) => [x[0], x[1]]), hidden: new Map((r.hidden || []).map((x) => [x[0], x[1]])) })
    else m.set(r.name, { kind: 'map', entries: new Map((r.entries || []).map(([k, f, v]) => [f == null ? k : `${k}\u001f${f}`, v])) })
  }
  return { schema: { name: GOLD.schema.keyed.name, roots: GOLD.schema.keyed.roots, composite: GOLD.schema.keyed.composite }, roots: m }
}

describe('the adapter', () => {
  const derived = (name: string, c: string): string | null => {
    const d = GOLD.index.containers.find((x) => x.key === c)?.derived as Record<string, string> | undefined
    return d?.[name] ?? null
  }

  it('the kit’s keyed state gives the same book as the sample’s pages, labels from containerDerived', () => {
    const kit = bookFromKeyedState(keyedStateOf(GOLD.state.roots), GOLD.state.upto, derived)
    const pages = bookFromPages(GOLD.state, GOLD.index.containers)
    expect(kit).toEqual(pages)
    expect(lanesOf(modelOf(kit)).map((t) => `${t.label} ${t.title}`)).toEqual(['V2 B-roll and titles', 'V1 Interviews', 'A1 Dialogue', 'A2 Music'])
  })

  it('the turn’s ops_change frames fold on the kit’s book exactly as on the sample’s', () => {
    let B = bookFromKeyedState(keyedStateOf(GOLD.state.roots), GOLD.state.upto, derived)
    const misfits: string[] = []
    for (const f of FRAMES) {
      B = applyFrame(B, f, misfits)
      expect(B).toEqual(BOOKS[f.rev])
    }
    expect(misfits).toEqual([])
  })

  it('an op that doesn’t fit is a misfit, never guessed at', () => {
    const misfits: string[] = []
    applyFrame(BOOKS[30], { rev: 31, derived: null, ops: [{ root: 'items', kind: 'list', op: 'delete', key: '@item:999', index_before: 0, index_after: null }] }, misfits)
    expect(misfits).toEqual(['rev 31: delete @item:999: not at 0'])
  })

  it('the pinned goldens read as pinned: their own labels, a frame’s derived containers taken in', () => {
    expect(lanesOf(modelOf(GB.cut)).map((t) => t.label)).toEqual(['V2', 'V1', 'A1'])
    const relabelled = applyFrame(GB.cut, { rev: 2, ops: [], derived: { containers: { '@trk:3': { kind_label: 'V9', kind: 'video' } } } })
    expect(modelOf(relabelled).byT['@trk:3'].label).toBe('V9')
    expect(bookFromPinned(GOLD.golden.cut)).toEqual(GB.cut)
  })
})
