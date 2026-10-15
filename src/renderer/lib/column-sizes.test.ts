// @vitest-environment jsdom
//
// Workspace column widths are per-window view state (split-view D2):
// `k2.windowChrome.columns.<label>`, per workspace and per column count.

import { beforeEach, describe, expect, it } from 'vitest'
import {
  columnSizesKey,
  equalColumnFlexes,
  readColumnFlexes,
  writeColumnFlexes,
} from './column-sizes'

beforeEach(() => {
  localStorage.clear()
})

describe('column sizes', () => {
  it('starts at an even split before anything is saved', () => {
    expect(readColumnFlexes('local|p1:w1', 2, 'main')).toEqual([50, 50])
    expect(readColumnFlexes('local|p1:w1', 3, 'main')).toEqual([34, 33, 33])
    expect(readColumnFlexes(null, 2, 'main')).toEqual([50, 50])
    expect(equalColumnFlexes(1)).toEqual([100])
  })

  it('reads back what was written, per workspace and per column count', () => {
    writeColumnFlexes('local|p1:w1', [70, 30], 'main')
    writeColumnFlexes('local|p1:w1', [20, 30, 50], 'main')
    expect(readColumnFlexes('local|p1:w1', 2, 'main')).toEqual([70, 30])
    expect(readColumnFlexes('local|p1:w1', 3, 'main')).toEqual([20, 30, 50])
    // Another workspace, and the same ids on another server, are separate.
    expect(readColumnFlexes('local|p1:w2', 2, 'main')).toEqual([50, 50])
    expect(readColumnFlexes('b.test|p1:w1', 2, 'main')).toEqual([50, 50])
  })

  it('keeps each window its own widths', () => {
    writeColumnFlexes('local|p1:w1', [70, 30], 'main')
    expect(readColumnFlexes('local|p1:w1', 2, 'window-2')).toEqual([50, 50])
    expect(JSON.parse(localStorage.getItem(columnSizesKey('main'))!)).toEqual({
      'local|p1:w1': { '2': [70, 30] },
    })
    expect(localStorage.getItem(columnSizesKey('window-2'))).toBeNull()
  })

  it('ignores a saved entry whose length or values do not fit', () => {
    localStorage.setItem(
      columnSizesKey('main'),
      JSON.stringify({ 'local|p1:w1': { '2': [70, 30, 0], '3': [50, -1, 51] } }),
    )
    expect(readColumnFlexes('local|p1:w1', 2, 'main')).toEqual([50, 50])
    expect(readColumnFlexes('local|p1:w1', 3, 'main')).toEqual([34, 33, 33])
    localStorage.setItem(columnSizesKey('main'), 'not json')
    expect(readColumnFlexes('local|p1:w1', 2, 'main')).toEqual([50, 50])
  })

  it('does not save one column, a bad array, or no workspace', () => {
    writeColumnFlexes('local|p1:w1', [100], 'main')
    writeColumnFlexes('local|p1:w1', [Number.NaN, 50], 'main')
    writeColumnFlexes('', [60, 40], 'main')
    expect(localStorage.getItem(columnSizesKey('main'))).toBeNull()
  })
})
