// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest'
import {
  SESSION_VIEW_TAB_DEFAULT,
  normalizeNewAgentView,
  overlayViewer,
  parseSessionSplitSides,
  parseSessionViewTab,
  resolveSessionView,
  retireChatSides,
  retireChatTab,
  savedViewNamesChat,
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
    // Chat view retired from the UI (0.46.4): a saved `chat` opens as the terminal.
    expect(parseSessionViewTab('chat')).toBe('terminal')
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
  })

  it('remembers each split side without resetting the other, including the same view twice', () => {
    const splitKey = sessionViewSplitStorageKey('local', 'sides')
    writeSessionSplitSides(splitKey, { left: 'chatter', right: 'thread' })
    expect(readSessionSplitSides(splitKey)).toEqual({ left: 'chatter', right: 'thread' })
    writeSessionSplitSides(splitKey, { left: 'chatter', right: 'chatter' })
    expect(readSessionSplitSides(splitKey)).toEqual({ left: 'chatter', right: 'chatter' })
  })
})

describe('saved Chat view migrates on read (Chat view retired from the UI in 0.46.4)', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('a saved chat pane opens as the terminal', () => {
    const tabKey = sessionViewTabStorageKey('local', 'conv-chat')
    const splitKey = sessionViewSplitStorageKey('local', 'conv-chat')
    localStorage.setItem(tabKey, 'chat')
    expect(readSessionViewTab(tabKey)).toBe('terminal')
    const resolved = resolveSessionView(tabKey, splitKey)
    expect(resolved).toEqual({
      tab: 'terminal',
      sides: { left: 'terminal', right: 'thread' },
      fresh: false,
      migrated: true,
    })
    const viewer = overlayViewer(resolved.tab, resolved.sides)
    expect(viewer.chat).toBe(false)
    expect(viewer.hidePty).toBe(false)
  })

  it('a saved Chat split side becomes the terminal, or the Thread beside a terminal', () => {
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chat', right: 'thread' }))).toEqual({
      left: 'terminal',
      right: 'thread',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chat', right: 'terminal' }))).toEqual({
      left: 'thread',
      right: 'terminal',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chatter', right: 'chat' }))).toEqual({
      left: 'chatter',
      right: 'terminal',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'terminal', right: 'chat' }))).toEqual({
      left: 'terminal',
      right: 'thread',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chat', right: 'chat' }))).toEqual({
      left: 'terminal',
      right: 'thread',
    })

    const tabKey = sessionViewTabStorageKey('local', 'conv-split-chat')
    const splitKey = sessionViewSplitStorageKey('local', 'conv-split-chat')
    localStorage.setItem(tabKey, 'split')
    localStorage.setItem(splitKey, JSON.stringify({ left: 'chat', right: 'chatter' }))
    const resolved = resolveSessionView(tabKey, splitKey)
    expect(resolved.tab).toBe('split')
    expect(resolved.sides).toEqual({ left: 'terminal', right: 'chatter' })
    expect(resolved.migrated).toBe(true)
    const viewer = overlayViewer(resolved.tab, resolved.sides)
    expect(viewer).toEqual({ thread: false, chatter: true, chat: false, hidePty: false })
  })

  it('a view with no Chat in it is not flagged for a write-back', () => {
    const tabKey = sessionViewTabStorageKey('local', 'conv-plain')
    const splitKey = sessionViewSplitStorageKey('local', 'conv-plain')
    localStorage.setItem(tabKey, 'split')
    localStorage.setItem(splitKey, JSON.stringify({ left: 'chatter', right: 'thread' }))
    expect(resolveSessionView(tabKey, splitKey).migrated).toBe(false)
    expect(savedViewNamesChat('thread', null)).toBe(false)
    expect(savedViewNamesChat('terminal', 'not json')).toBe(false)
    expect(savedViewNamesChat('chat', null)).toBe(true)
    expect(savedViewNamesChat('split', '{"right":"chat"}')).toBe(true)
  })

  it('a saved "New agents open in" Chat pick reads back as Terminal, Chat sides too', () => {
    expect(normalizeNewAgentView({ view: 'chat', left: 'terminal', right: 'thread' })).toEqual({
      view: 'terminal',
      split: { left: 'terminal', right: 'thread' },
    })
    expect(normalizeNewAgentView({ view: 'split', left: 'chat', right: 'thread' })).toEqual({
      view: 'split',
      split: { left: 'terminal', right: 'thread' },
    })
    expect(normalizeNewAgentView({ view: 'split', left: 'thread', right: 'chat' })).toEqual({
      view: 'split',
      split: { left: 'thread', right: 'terminal' },
    })
  })

  it('retireChatTab and retireChatSides leave every other view alone', () => {
    for (const tab of ['terminal', 'thread', 'chatter', 'split'] as const) {
      expect(retireChatTab(tab)).toBe(tab)
    }
    expect(retireChatTab('chat')).toBe('terminal')
    const sides = { left: 'chatter', right: 'chatter' } as const
    expect(retireChatSides(sides)).toBe(sides)
  })
})

// `overlayViewer` is a pure function of the view it is handed. Its `chat`
// flag is kept (with the retained Chat view code) until 0.46.5; since 0.46.4
// no live view state can hand it `chat`, so the two `chat` cases below are
// the only callers that do.
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
