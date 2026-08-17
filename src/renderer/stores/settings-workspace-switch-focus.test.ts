// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest'
import {
  LS_WORKSPACE_SWITCH_FOCUS,
  normalizeWorkspaceSwitchFocus,
  readWorkspaceSwitchFocus,
  writeWorkspaceSwitchFocus,
} from './settings'

describe('workspaceSwitchFocus localStorage', () => {
  beforeEach(() => {
    localStorage.removeItem(LS_WORKSPACE_SWITCH_FOCUS)
  })

  it('normalizes only composer; everything else is terminal', () => {
    expect(normalizeWorkspaceSwitchFocus('composer')).toBe('composer')
    expect(normalizeWorkspaceSwitchFocus('terminal')).toBe('terminal')
    expect(normalizeWorkspaceSwitchFocus('')).toBe('terminal')
    expect(normalizeWorkspaceSwitchFocus('nope')).toBe('terminal')
    expect(normalizeWorkspaceSwitchFocus(null)).toBe('terminal')
  })

  it('reads terminal when unset', () => {
    expect(readWorkspaceSwitchFocus()).toBe('terminal')
  })

  it('round-trips composer on this install', () => {
    writeWorkspaceSwitchFocus('composer')
    expect(localStorage.getItem(LS_WORKSPACE_SWITCH_FOCUS)).toBe('composer')
    expect(readWorkspaceSwitchFocus()).toBe('composer')
  })
})
