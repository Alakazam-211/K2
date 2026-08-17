import { describe, expect, it } from 'vitest'

import {
  applyFrameBatch,
  isGridEmpty,
  mergeDelta,
  type CellRun,
  type CursorSnapshot,
  type TermGridDelta,
  type TermGridSnapshot,
} from './gridState'

function run(text: string, over: Partial<CellRun> = {}): CellRun {
  return {
    text,
    fg: null,
    bg: null,
    bold: false,
    italic: false,
    underline: false,
    inverse: false,
    dim: false,
    strikeout: false,
    ...over,
  }
}

function cursor(row = 0, col = 0): CursorSnapshot {
  return { row, col, visible: true }
}

function snap(over: Partial<TermGridSnapshot> = {}): TermGridSnapshot {
  const grid = over.grid ?? [[run('prompt')]]
  return {
    paneId: 'pane',
    cols: 80,
    rows: grid.length,
    grid,
    scrollback: [],
    cursor: cursor(),
    version: 1,
    displayOffset: 0,
    ...over,
  }
}

function delta(over: Partial<TermGridDelta> = {}): TermGridDelta {
  return {
    paneId: 'other',
    cols: 80,
    rows: 1,
    damagedRows: [],
    scrollbackAppended: [],
    cursor: cursor(0, 1),
    version: 2,
    displayOffset: 0,
    ...over,
  }
}

describe('isGridEmpty', () => {
  it('true for an all-blank visible grid', () => {
    expect(isGridEmpty(snap({ grid: [[], [run('   ')]] }))).toBe(true)
  })

  it('false when any visible run has ink', () => {
    expect(isGridEmpty(snap({ grid: [[run(' $')]] }))).toBe(false)
  })
})

describe('mergeDelta', () => {
  it('returns prev unchanged when there is no snapshot yet', () => {
    const d = delta()
    expect(mergeDelta(null, d)).toBeNull()
  })

  it('patches damaged rows and keeps untouched row identity', () => {
    const row0 = [run('keep')]
    const row1 = [run('old')]
    const prev = snap({ grid: [row0, row1], rows: 2 })
    const next = mergeDelta(
      prev,
      delta({
        rows: 2,
        damagedRows: [{ row: 1, runs: [run('new')] }],
      }),
    )
    expect(next).not.toBeNull()
    expect(next!.grid[0]).toBe(row0)
    expect(next!.grid[1]).toEqual([run('new')])
    expect(next!.grid[1]).not.toBe(row1)
  })

  it('grows and shrinks the live grid to delta.rows', () => {
    const prev = snap({ grid: [[run('a')], [run('b')]], rows: 2 })
    const grown = mergeDelta(prev, delta({ rows: 4, damagedRows: [] }))
    expect(grown!.grid).toHaveLength(4)
    expect(grown!.grid[2]).toEqual([])
    expect(grown!.grid[3]).toEqual([])

    const shrunk = mergeDelta(prev, delta({ rows: 1, damagedRows: [] }))
    expect(shrunk!.grid).toHaveLength(1)
    expect(shrunk!.grid[0]).toBe(prev.grid[0])
  })

  it('skips damaged rows outside [0, delta.rows)', () => {
    const prev = snap({ grid: [[run('a')]], rows: 1 })
    const next = mergeDelta(
      prev,
      delta({
        rows: 1,
        damagedRows: [
          { row: -1, runs: [run('no')] },
          { row: 1, runs: [run('no')] },
        ],
      }),
    )
    expect(next!.grid[0]).toBe(prev.grid[0])
  })

  it('appends scrollback only when the delta carries rows', () => {
    const sb = [[run('old')]]
    const prev = snap({ scrollback: sb })
    const same = mergeDelta(prev, delta({ scrollbackAppended: [] }))
    expect(same!.scrollback).toBe(sb)

    const added = [[run('more')]]
    const next = mergeDelta(prev, delta({ scrollbackAppended: added }))
    expect(next!.scrollback).toEqual([[run('old')], [run('more')]])
    expect(next!.scrollback).not.toBe(sb)
  })

  it('takes dims/cursor/version from the delta and sticky mode from prev', () => {
    const prev = snap({
      paneId: 'keep-me',
      mouseReport: true,
      sgrMouse: true,
      altScreen: true,
    })
    const next = mergeDelta(
      prev,
      delta({
        paneId: 'ignored',
        cols: 40,
        rows: 2,
        cursor: cursor(1, 3),
        version: 9,
        displayOffset: 4,
      }),
    )
    expect(next!.paneId).toBe('keep-me')
    expect(next!.cols).toBe(40)
    expect(next!.rows).toBe(2)
    expect(next!.cursor).toEqual(cursor(1, 3))
    expect(next!.version).toBe(9)
    expect(next!.displayOffset).toBe(4)
    expect(next!.mouseReport).toBe(true)
    expect(next!.sgrMouse).toBe(true)
    expect(next!.altScreen).toBe(true)
  })
})

describe('applyFrameBatch', () => {
  const base = {
    rendered: null as TermGridSnapshot | null,
    scrollPx: 0,
    cellHeight: 20,
    resizeHoldActive: false,
  }

  it('replaces the live grid with a snapshot', () => {
    const live = snap({ version: 1, grid: [[run('old')]] })
    const payload = snap({ version: 5, grid: [[run('fresh')]] })
    const result = applyFrameBatch({
      ...base,
      live,
      pending: [{ kind: 'snapshot', payload }],
    })
    expect(result.live).toBe(payload)
    expect(result.suppressRender).toBe(false)
    expect(result.ackVersion).toBe(5)
    expect(result.scrollPx).toBe(0)
  })

  it('merges deltas in order onto the live grid', () => {
    const live = snap({
      grid: [[run('a')], [run('b')]],
      rows: 2,
      version: 1,
    })
    const result = applyFrameBatch({
      ...base,
      live,
      pending: [
        {
          kind: 'delta',
          payload: delta({
            rows: 2,
            version: 2,
            damagedRows: [{ row: 0, runs: [run('A')] }],
          }),
        },
        {
          kind: 'delta',
          payload: delta({
            rows: 2,
            version: 4,
            damagedRows: [{ row: 1, runs: [run('B')] }],
          }),
        },
      ],
    })
    expect(result.live!.grid[0]).toEqual([run('A')])
    expect(result.live!.grid[1]).toEqual([run('B')])
    expect(result.ackVersion).toBe(4)
  })

  it('applies deltas after a snapshot in the same batch', () => {
    const payload = snap({
      grid: [[run('s0')], [run('s1')]],
      rows: 2,
      version: 3,
    })
    const result = applyFrameBatch({
      ...base,
      live: snap({ version: 1 }),
      pending: [
        { kind: 'snapshot', payload },
        {
          kind: 'delta',
          payload: delta({
            rows: 2,
            version: 6,
            damagedRows: [{ row: 1, runs: [run('d1')] }],
          }),
        },
      ],
    })
    expect(result.live!.grid[0]).toBe(payload.grid[0])
    expect(result.live!.grid[1]).toEqual([run('d1')])
    expect(result.ackVersion).toBe(6)
  })

  it('acks the highest version even when render is suppressed', () => {
    const rendered = snap({ grid: [[run('keep')]] })
    const blank = snap({ version: 11, grid: [[run('   ')]] })
    const result = applyFrameBatch({
      ...base,
      live: rendered,
      rendered,
      resizeHoldActive: true,
      pending: [{ kind: 'snapshot', payload: blank }],
    })
    expect(result.suppressRender).toBe(true)
    expect(result.live).toBe(blank)
    expect(result.ackVersion).toBe(11)
  })

  it('does not suppress a blank when the hold is inactive', () => {
    const rendered = snap({ grid: [[run('keep')]] })
    const blank = snap({ version: 2, grid: [[]] })
    const result = applyFrameBatch({
      ...base,
      live: rendered,
      rendered,
      resizeHoldActive: false,
      pending: [{ kind: 'snapshot', payload: blank }],
    })
    expect(result.suppressRender).toBe(false)
    expect(result.live).toBe(blank)
  })

  it('does not suppress when the next frame has ink', () => {
    const rendered = snap({ grid: [[run('keep')]] })
    const next = snap({ version: 2, grid: [[run('ok')]] })
    const result = applyFrameBatch({
      ...base,
      live: rendered,
      rendered,
      resizeHoldActive: true,
      pending: [{ kind: 'snapshot', payload: next }],
    })
    expect(result.suppressRender).toBe(false)
  })

  it('anchors scroll when rows append below a scrolled-up view', () => {
    const live = snap({
      grid: [[run('g')]],
      scrollback: [[run('old')]],
    })
    const result = applyFrameBatch({
      ...base,
      live,
      scrollPx: 20,
      pending: [
        {
          kind: 'delta',
          payload: delta({
            rows: 1,
            version: 3,
            scrollbackAppended: [[run('n0')], [run('n1')], [run('n2')]],
          }),
        },
      ],
    })
    expect(result.scrollPx).toBe(80)
    expect(result.live!.scrollback).toHaveLength(4)
  })

  it('leaves scrollPx at 0 so the view follows live output', () => {
    const live = snap({ scrollback: [[run('old')]] })
    const result = applyFrameBatch({
      ...base,
      live,
      scrollPx: 0,
      pending: [
        {
          kind: 'delta',
          payload: delta({
            rows: 1,
            version: 3,
            scrollbackAppended: [[run('n0')]],
          }),
        },
      ],
    })
    expect(result.scrollPx).toBe(0)
  })

  it('re-anchors a cap-shift snapshot by content seam-match', () => {
    const lines = Array.from({ length: 20 }, (_, i) => [run(`line-${i}`)])
    const live = snap({
      rows: 4,
      grid: [[run('g0')], [run('g1')], [run('g2')], [run('g3')]],
      scrollback: lines,
    })
    const shifted = snap({
      rows: 4,
      version: 8,
      grid: live.grid,
      scrollback: [...lines.slice(3), [run('new-a')], [run('new-b')], [run('new-c')]],
    })
    const result = applyFrameBatch({
      ...base,
      live,
      scrollPx: 100,
      pending: [{ kind: 'snapshot', payload: shifted }],
    })
    expect(result.scrollPx).toBe(160)
  })

  it('falls back to total-row growth when the snapshot has no seam match', () => {
    const live = snap({
      rows: 4,
      grid: [[run('g0')], [run('g1')], [run('g2')], [run('g3')]],
      scrollback: Array.from({ length: 10 }, (_, i) => [run(`line-${i}`)]),
    })
    const unrelated = snap({
      rows: 4,
      version: 8,
      grid: live.grid,
      scrollback: Array.from({ length: 15 }, (_, i) => [run(`other-${i}`)]),
    })
    const result = applyFrameBatch({
      ...base,
      live,
      scrollPx: 40,
      pending: [{ kind: 'snapshot', payload: unrelated }],
    })
    expect(result.scrollPx).toBe(140)
  })
})
