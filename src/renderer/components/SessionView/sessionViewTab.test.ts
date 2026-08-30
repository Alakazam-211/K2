// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest'
import {
  SESSION_VIEW_TAB_DEFAULT,
  overlayViewer,
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
    expect(parseSessionViewTab('chatter')).toBe('chatter')
    expect(parseSessionViewTab('split')).toBe('split')
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

describe('overlayViewer', () => {
  it('terminal mounts neither overlay and keeps the PTY visible', () => {
    expect(overlayViewer('terminal')).toEqual({
      thread: false,
      chatter: false,
      hidePty: false,
    })
  })

  it('thread mounts Thread only and hides the PTY', () => {
    expect(overlayViewer('thread')).toEqual({
      thread: true,
      chatter: false,
      hidePty: true,
    })
  })

  it('chatter mounts Chatter only and hides the PTY', () => {
    expect(overlayViewer('chatter')).toEqual({
      thread: false,
      chatter: true,
      hidePty: true,
    })
  })

  it('split mounts Thread with the PTY still visible', () => {
    expect(overlayViewer('split')).toEqual({
      thread: true,
      chatter: false,
      hidePty: false,
    })
  })
})
