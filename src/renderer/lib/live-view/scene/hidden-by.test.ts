// Hidden by: K2 Cores' rule (k2-mpc-blend docs/WIRE.md, "Hidden by"), on
// their shared vectors (tests/fixtures/blend-hidden-by/hidden-by.json,
// copied unchanged from k2-mpc-blend feat/mesh-read-3 56aa72f6,
// crates/blend/tests/data/hidden-by.json): its own flag first; any path
// through shown collections shows it; else the outermost hidden collection
// on the first depth-first path; absent when it isn't in the scene drawn.
// The tab's '(hidden)' reads the same rule (hiddenOf).

import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'
import { hiddenBy, type HiddenByScene } from './hidden-by'
import { hiddenOf } from './overlay'
import { simulateRaw, TURN } from './turn'

const V = JSON.parse(readFileSync(join(__dirname, '..', '..', '..', '..', '..', 'tests', 'fixtures', 'blend-hidden-by', 'hidden-by.json'), 'utf8')) as { cases: (HiddenByScene & { name: string; expect: Record<string, string | null> })[] }

describe('hidden by, on K2 Cores’ vectors', () => {
  it('has all eleven cases', () => {
    expect(V.cases).toHaveLength(11)
  })
  for (const c of V.cases)
    it(c.name, () => {
      expect(hiddenBy(c)).toEqual(c.expect)
    })
})

describe('the tab’s hidden flag reads the rule', () => {
  it('the sample’s hidden left wall is hidden by its own flag; a shown object is shown', () => {
    const S = simulateRaw(TURN, 0)
    expect(hiddenOf(S.D!, '@obj:6')).toBe('self')
    expect(hiddenOf(S.D!, '@obj:4')).toBe('none')
  })
})
