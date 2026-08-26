// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest'
import {
  SESSION_VIEW_TAB_DEFAULT,
  parseSessionViewTab,
  readSessionViewTab,
  sessionViewTabStorageKey,
  writeSessionViewTab,
} from './sessionViewTab'

describe('session view tab (C3/C8)', () => {
  beforeEach(() => {
    if (typeof localStorage !== 'undefined') localStorage.clear()
  })

  it('defaults to Terminal', () => {
    expect(SESSION_VIEW_TAB_DEFAULT).toBe('terminal')
    expect(parseSessionViewTab(null)).toBe('terminal')
    expect(parseSessionViewTab('nope')).toBe('terminal')
    expect(parseSessionViewTab('thread')).toBe('thread')
  })

  it('keys memory per host + conversation (this window)', () => {
    const a = sessionViewTabStorageKey('local', 'conv-a')
    const b = sessionViewTabStorageKey('local', 'conv-b')
    const remote = sessionViewTabStorageKey('host:box:443', 'conv-a')
    expect(a).not.toBe(b)
    expect(a).not.toBe(remote)
    writeSessionViewTab(a, 'thread')
    expect(readSessionViewTab(a)).toBe('thread')
    expect(readSessionViewTab(b)).toBe('terminal')
    expect(readSessionViewTab(remote)).toBe('terminal')
  })
})
