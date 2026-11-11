// @vitest-environment jsdom
//
// Drawer tab order (0.46.13): the pure order rules and this client's storage.

import { afterEach, describe, expect, it } from 'vitest'
import {
  applyTabOrder,
  drawerTabOrderKey,
  dropSlotAt,
  forgetTab,
  isCustomOrder,
  moveByStep,
  moveToSlot,
  readDrawerTabOrder,
  writeDrawerTabOrder,
} from './drawer-tab-order'

afterEach(() => {
  localStorage.clear()
})

describe('applyTabOrder', () => {
  it('no saved order keeps the base order', () => {
    expect(applyTabOrder(['files', 'workspace'], null)).toEqual(['files', 'workspace'])
    expect(applyTabOrder(['files', 'workspace'], [])).toEqual(['files', 'workspace'])
  })

  it('applies a saved order', () => {
    expect(applyTabOrder(['files', 'workspace', 'agent'], ['agent', 'files', 'workspace'])).toEqual([
      'agent',
      'files',
      'workspace',
    ])
  })

  it('appends tabs the saved order does not name, in base order', () => {
    expect(applyTabOrder(['files', 'workspace', 'agent', 'history'], ['workspace', 'files'])).toEqual([
      'workspace',
      'files',
      'agent',
      'history',
    ])
  })

  it('drops saved ids the drawer no longer has, and duplicates', () => {
    expect(applyTabOrder(['files', 'workspace'], ['agent', 'workspace', 'gone', 'workspace', 'files'])).toEqual([
      'workspace',
      'files',
    ])
  })

  it('isCustomOrder says whether a reset would change anything', () => {
    expect(isCustomOrder(['a', 'b'], ['a', 'b'])).toBe(false)
    expect(isCustomOrder(['a', 'b'], ['b', 'a'])).toBe(true)
  })
})

describe('moveToSlot (drag)', () => {
  const order = ['a', 'b', 'c', 'd'] as const

  it('moves right: slot after the target', () => {
    expect(moveToSlot(order, 0, 3)).toEqual(['b', 'c', 'a', 'd'])
    expect(moveToSlot(order, 0, 4)).toEqual(['b', 'c', 'd', 'a'])
  })

  it('moves left: slot before the target', () => {
    expect(moveToSlot(order, 3, 0)).toEqual(['d', 'a', 'b', 'c'])
    expect(moveToSlot(order, 2, 1)).toEqual(['a', 'c', 'b', 'd'])
  })

  it('its own slots are a no-op and return the same array', () => {
    expect(moveToSlot(order, 1, 1)).toBe(order)
    expect(moveToSlot(order, 1, 2)).toBe(order)
  })

  it('clamps an out-of-range slot and ignores a bad source', () => {
    expect(moveToSlot(order, 0, 99)).toEqual(['b', 'c', 'd', 'a'])
    expect(moveToSlot(order, 3, -5)).toEqual(['d', 'a', 'b', 'c'])
    expect(moveToSlot(order, 7, 0)).toBe(order)
  })
})

describe('moveByStep (keyboard)', () => {
  const order = ['a', 'b', 'c'] as const

  it('moves one place either way', () => {
    expect(moveByStep(order, 0, 1)).toEqual(['b', 'a', 'c'])
    expect(moveByStep(order, 2, -1)).toEqual(['a', 'c', 'b'])
    expect(moveByStep(order, 1, 1)).toEqual(['a', 'c', 'b'])
  })

  it('stops at the edges (same array back)', () => {
    expect(moveByStep(order, 0, -1)).toBe(order)
    expect(moveByStep(order, 2, 1)).toBe(order)
  })
})

describe('dropSlotAt', () => {
  const tabs = [
    { left: 0, width: 100 },
    { left: 100, width: 100 },
    { left: 200, width: 100 },
  ]

  it('picks the slot by tab midpoints', () => {
    expect(dropSlotAt(tabs, -10)).toBe(0)
    expect(dropSlotAt(tabs, 49)).toBe(0)
    expect(dropSlotAt(tabs, 50)).toBe(1)
    expect(dropSlotAt(tabs, 160)).toBe(2)
    expect(dropSlotAt(tabs, 250)).toBe(3)
    expect(dropSlotAt(tabs, 999)).toBe(3)
  })
})

describe('forgetTab', () => {
  it('removes the id; an empty result means no custom order', () => {
    expect(forgetTab(['a', 'b'], 'a')).toEqual(['b'])
    expect(forgetTab(['a'], 'a')).toBeNull()
    expect(forgetTab(null, 'a')).toBeNull()
  })
})

describe('storage', () => {
  it('round-trips per side and clears with null', () => {
    writeDrawerTabOrder('left', ['workspace', 'files'])
    expect(localStorage.getItem(drawerTabOrderKey('left'))).toBe('["workspace","files"]')
    expect(readDrawerTabOrder('left')).toEqual(['workspace', 'files'])
    expect(readDrawerTabOrder('right')).toBeNull()
    writeDrawerTabOrder('left', null)
    expect(localStorage.getItem(drawerTabOrderKey('left'))).toBeNull()
    expect(readDrawerTabOrder('left')).toBeNull()
  })

  it('a bad saved value reads as no order', () => {
    localStorage.setItem(drawerTabOrderKey('right'), '{"not":"a list"}')
    expect(readDrawerTabOrder('right')).toBeNull()
    localStorage.setItem(drawerTabOrderKey('right'), '[1,2]')
    expect(readDrawerTabOrder('right')).toBeNull()
    localStorage.setItem(drawerTabOrderKey('right'), 'not json')
    expect(readDrawerTabOrder('right')).toBeNull()
  })
})
