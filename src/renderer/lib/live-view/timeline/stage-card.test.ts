// The monitor when there's no edit to show (T6): opening the file (each part
// as it arrives, step N of M), a timeline with no clips yet, and a file that
// can't be watched live, each with the mock's words and its next step.

import { describe, expect, it } from 'vitest'
import { modelOf } from './fold'
import { EMPTY, OPEN } from './scenarios'
import { simulate } from './sim'
import { stageCard } from './stage-card'
import { CTX } from './world'

const empty = EMPTY.find((s) => s.id === 'empty')!
const errors = EMPTY.find((s) => s.id === 'errors')!
const card = (sc: typeof OPEN, i: number) => {
  const S = simulate(sc, i, CTX)
  return stageCard(S, modelOf(S.B), { agent: 'Atlas', file: 'rough-cut' })
}

describe('the monitor with no edit to show', () => {
  it('opening: the parts as they arrive, step N of M, the lanes first', () => {
    const c0 = card(OPEN, 0)
    expect(c0).toMatchObject({ kind: 'opening', title: 'Opening rough-cut' })
    if (c0?.kind !== 'opening') throw new Error('not opening')
    expect(c0.line).toMatch(/^The lanes fill in as each part arrives; the picture comes once the clips are in\. Step 1 of \d+\.$/)
    expect(c0.rows[0]).toEqual({ text: 'Connecting', state: 'now', n: null })
    expect(c0.rows[1]).toEqual({ text: 'Who you are and the lanes', state: 'wait', n: null })
    const c3 = card(OPEN, 3)
    if (c3?.kind !== 'opening') throw new Error('not opening')
    // the page this step brought has landed; the next waits
    expect(c3.rows.slice(0, 4).map((r) => r.state)).toEqual(['done', 'done', 'done', 'done'])
    expect(c3.rows[4].state).toBe('wait')
    expect(c3.rows[3].n).toBeGreaterThan(0)
    expect(c3.rows.map((r) => r.text)).toContain('Clips')
    expect(c3.progress).toBeGreaterThan(c0.progress)
    expect(card(OPEN, OPEN.steps.length - 1)).toBeNull()
  })

  it('no clips yet: the words and Ask Atlas; the first clip takes it away', () => {
    expect(card(empty, 0)).toEqual({ kind: 'empty', title: 'No clips on this timeline yet', line: 'When Atlas adds clips, the picture shows here.', actions: [{ id: 'ask', label: 'Ask Atlas' }] })
    expect(card(empty, 1)).toBeNull()
  })

  it('a file that can’t be watched live: why, with Open saved versions and Ask Atlas', () => {
    const c = card(errors, 0)
    expect(c).toEqual({
      kind: 'declined',
      title: 'This file holds 3 timelines',
      line: 'K2 watches one timeline per file live. You can still open the file’s saved versions.',
      actions: [
        { id: 'open-saved', label: 'Open saved versions', primary: true },
        { id: 'ask', label: 'Ask Atlas' },
      ],
    })
    expect(errors.steps.map((_, i) => card(errors, i)?.title)).toEqual(['This file holds 3 timelines', 'Two clips overlap on V2', 'Some times fall between frames', 'Too large to watch live, for now'])
  })
})
