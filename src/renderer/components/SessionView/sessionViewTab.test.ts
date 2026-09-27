// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest'
import {
  SESSION_VIEW_TAB_DEFAULT,
  overlayViewer,
  parseSessionViewTab,
  readSessionSplitSides,
  readSessionViewTab,
  sessionViewSplitStorageKey,
  sessionViewTabStorageKey,
  writeSessionSplitSides,
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
    expect(parseSessionViewTab('chat')).toBe('chat')
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

  it('opens a stored split with no side record as Terminal + Thread', () => {
    const tabKey = sessionViewTabStorageKey('local', 'old-split')
    const splitKey = sessionViewSplitStorageKey('local', 'old-split')
    localStorage.setItem(tabKey, 'split')
    expect(readSessionViewTab(tabKey)).toBe('split')
    expect(readSessionSplitSides(splitKey)).toEqual({ left: 'terminal', right: 'thread' })
    expect(parseSessionViewTab('chat')).toBe('chat')
  })

  it('remembers each split side without resetting the other, including the same view twice', () => {
    const splitKey = sessionViewSplitStorageKey('local', 'sides')
    writeSessionSplitSides(splitKey, { left: 'chatter', right: 'thread' })
    expect(readSessionSplitSides(splitKey)).toEqual({ left: 'chatter', right: 'thread' })
    writeSessionSplitSides(splitKey, { left: 'chatter', right: 'chatter' })
    expect(readSessionSplitSides(splitKey)).toEqual({ left: 'chatter', right: 'chatter' })
  })
})

describe('overlayViewer', () => {
  it('terminal mounts neither overlay and keeps the PTY visible', () => {
    expect(overlayViewer('terminal')).toEqual({
      thread: false,
      chatter: false,
      chat: false,
      hidePty: false,
    })
  })

  it('thread mounts Thread only and hides the PTY', () => {
    expect(overlayViewer('thread')).toEqual({
      thread: true,
      chatter: false,
      chat: false,
      hidePty: true,
    })
  })

  it('chatter mounts Chatter only and hides the PTY', () => {
    expect(overlayViewer('chatter')).toEqual({
      thread: false,
      chatter: true,
      chat: false,
      hidePty: true,
    })
  })

  it('split mounts Thread with the PTY still visible', () => {
    expect(overlayViewer('split')).toEqual({
      thread: true,
      chatter: false,
      chat: false,
      hidePty: false,
    })
  })

  it('chat hides the PTY and does not mount Thread or Chatter', () => {
    expect(overlayViewer('chat')).toEqual({
      thread: false,
      chatter: false,
      chat: true,
      hidePty: true,
    })
  })

  it('split sides follow the stored pair, including the same view on both sides', () => {
    expect(overlayViewer('split', { left: 'chat', right: 'chatter' })).toEqual({
      thread: false,
      chatter: true,
      chat: true,
      hidePty: true,
    })
    expect(overlayViewer('split', { left: 'thread', right: 'thread' })).toEqual({
      thread: true,
      chatter: false,
      chat: false,
      hidePty: true,
    })
  })
})
