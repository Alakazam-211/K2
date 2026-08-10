import { describe, expect, it } from 'vitest'
import {
  APP_MENU_ACTION_IDS,
  appMenuActionHasHandler,
  isAppMenuActionId,
} from './app-menu-actions'

describe('app-menu-actions', () => {
  it('every menu action ID has a handler', () => {
    for (const id of APP_MENU_ACTION_IDS) {
      expect(appMenuActionHasHandler(id)).toBe(true)
    }
  })

  it('isAppMenuActionId accepts known IDs only', () => {
    expect(isAppMenuActionId('settings')).toBe(true)
    expect(isAppMenuActionId('quit')).toBe(true)
    expect(isAppMenuActionId('not-a-real-id')).toBe(false)
  })

  it('menu IDs are a subset of handler-covered IDs', () => {
    const covered = APP_MENU_ACTION_IDS.filter(appMenuActionHasHandler)
    expect(covered).toEqual([...APP_MENU_ACTION_IDS])
  })
})
