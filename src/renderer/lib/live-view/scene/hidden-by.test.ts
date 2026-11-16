// Hidden by: K2 Cores' rule (k2-mpc-blend docs/WIRE.md, "Hidden by"), on
// their shared vectors (tests/fixtures/blend-hidden-by/hidden-by.json,
// copied unchanged from k2-mpc-blend 3a0c5cee, 2026-11-16,
// crates/blend/tests/data/hidden-by.json; 13 cases, two with `moves`: a
// collection moved live, taken out from under every parent and put last
// under `to`, before the walk): its own flag first; any path
// through shown collections shows it; else the outermost hidden collection
// on the first depth-first path; absent when it isn't in the scene drawn.
// The tab's '(hidden)' reads the same rule (hiddenOf).

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { hiddenBy, type HiddenByScene } from './hidden-by'
import { hiddenOf } from './overlay'
import { simulateRaw, TURN } from './turn'

type Move = { collection: string; to: string }
const V = JSON.parse(readFileSync(join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'blend-hidden-by', 'hidden-by.json'), 'utf8')) as { cases: (HiddenByScene & { name: string; moves?: Move[]; expect: Record<string, string | null> })[] }

/** A collection moved live: out from under every parent, then last under `to` (in order). */
function moved(sc: HiddenByScene, moves: Move[] = []): HiddenByScene {
  const collections = sc.collections.map((c) => ({ ...c, children: [...c.children] }))
  for (const m of moves) {
    for (const c of collections) c.children = c.children.filter((k) => k !== m.collection)
    collections.find((c) => c.key === m.to)?.children.push(m.collection)
  }
  return { ...sc, collections }
}

describe('hidden by, on K2 Cores’ vectors', () => {
  it('has all thirteen cases, two with a collection moved live', () => {
    expect(V.cases).toHaveLength(13)
    expect(V.cases.filter((c) => c.moves?.length)).toHaveLength(2)
  })
  for (const c of V.cases)
    it(c.name, () => {
      expect(hiddenBy(moved(c, c.moves))).toEqual(c.expect)
    })
})

describe('the tab’s hidden flag reads the rule', () => {
  it('the sample’s hidden left wall is hidden by its own flag; a shown object is shown', () => {
    const S = simulateRaw(TURN, 0)
    expect(hiddenOf(S.D!, '@obj:6')).toBe('self')
    expect(hiddenOf(S.D!, '@obj:4')).toBe('none')
  })
})
