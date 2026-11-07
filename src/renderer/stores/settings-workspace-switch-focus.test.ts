// @vitest-environment jsdom
import { describe, it, expect, beforeEach } from 'vitest'
import {
  LS_WORKSPACE_SWITCH_FOCUS,
  WORKSPACE_SWITCH_FOCUS_DEFAULT,
  normalizeWorkspaceSwitchFocus,
  readWorkspaceSwitchFocus,
  writeWorkspaceSwitchFocus,
} from './settings'

describe('workspaceSwitchFocus localStorage', () => {
  beforeEach(() => {
    localStorage.removeItem(LS_WORKSPACE_SWITCH_FOCUS)
  })

  it('defaults to Message agent (composer) since 0.46.10', () => {
    expect(WORKSPACE_SWITCH_FOCUS_DEFAULT).toBe('composer')
  })

  it('keeps every legal pick; blank, missing or garbage is the default (composer)', () => {
    expect(normalizeWorkspaceSwitchFocus('composer')).toBe('composer')
    expect(normalizeWorkspaceSwitchFocus('off')).toBe('off')
    expect(normalizeWorkspaceSwitchFocus('terminal')).toBe('terminal')
    expect(normalizeWorkspaceSwitchFocus('')).toBe('composer')
    expect(normalizeWorkspaceSwitchFocus('nope')).toBe('composer')
    expect(normalizeWorkspaceSwitchFocus(null)).toBe('composer')
    expect(normalizeWorkspaceSwitchFocus(undefined)).toBe('composer')
  })

  it('reads Message agent (composer) when unset', () => {
    expect(localStorage.getItem(LS_WORKSPACE_SWITCH_FOCUS)).toBeNull()
    expect(readWorkspaceSwitchFocus()).toBe('composer')
  })

  it('a saved Terminal pick keeps winning over the new default', () => {
    localStorage.setItem(LS_WORKSPACE_SWITCH_FOCUS, 'terminal')
    expect(readWorkspaceSwitchFocus()).toBe('terminal')
    expect(localStorage.getItem(LS_WORKSPACE_SWITCH_FOCUS)).toBe('terminal')
  })

  it('round-trips terminal on this install', () => {
    writeWorkspaceSwitchFocus('terminal')
    expect(localStorage.getItem(LS_WORKSPACE_SWITCH_FOCUS)).toBe('terminal')
    expect(readWorkspaceSwitchFocus()).toBe('terminal')
  })

  it('round-trips composer on this install', () => {
    writeWorkspaceSwitchFocus('composer')
    expect(localStorage.getItem(LS_WORKSPACE_SWITCH_FOCUS)).toBe('composer')
    expect(readWorkspaceSwitchFocus()).toBe('composer')
  })

  it('round-trips off on this install', () => {
    writeWorkspaceSwitchFocus('off')
    expect(readWorkspaceSwitchFocus()).toBe('off')
  })
})
