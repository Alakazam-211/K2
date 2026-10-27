// @vitest-environment jsdom
import { describe, expect, it, beforeEach } from 'vitest'
import {
  SESSION_VIEW_TAB_DEFAULT,
  normalizeNewAgentView,
  overlayViewer,
  parseSessionSplitSides,
  parseSessionViewTab,
  readNewAgentView,
  resolveSessionView,
  savedViewNamesLegacyChat,
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
    // The Chat view is gone (0.46.5): a saved `chat` opens the agent's Thread.
    expect(parseSessionViewTab('chat')).toBe('thread')
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

// The Chat view was deleted in 0.46.5. Rosson 2026-10-27: "Anyone using it
// as a default point them at threads." Saved state that names it migrates on
// read to the Thread, and the caller writes the fixed value back.
describe('saved Chat view migrates to Threads on read (0.46.5)', () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it('a saved chat pane opens the Thread view', () => {
    const tabKey = sessionViewTabStorageKey('local', 'conv-chat')
    const splitKey = sessionViewSplitStorageKey('local', 'conv-chat')
    localStorage.setItem(tabKey, 'chat')
    expect(readSessionViewTab(tabKey)).toBe('thread')
    const resolved = resolveSessionView(tabKey, splitKey)
    expect(resolved).toEqual({
      tab: 'thread',
      sides: { left: 'terminal', right: 'thread' },
      fresh: false,
      migrated: true,
    })
    expect(overlayViewer(resolved.tab, resolved.sides)).toEqual({
      thread: true,
      chatter: false,
      hidePty: true,
    })
  })

  it('a saved Chat split side becomes the Thread beside the terminal', () => {
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chat', right: 'terminal' }))).toEqual({
      left: 'thread',
      right: 'terminal',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'terminal', right: 'chat' }))).toEqual({
      left: 'terminal',
      right: 'thread',
    })
    // The other side is already the Thread: the Chat side becomes the terminal.
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chat', right: 'thread' }))).toEqual({
      left: 'terminal',
      right: 'thread',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'thread', right: 'chat' }))).toEqual({
      left: 'thread',
      right: 'terminal',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chatter', right: 'chat' }))).toEqual({
      left: 'chatter',
      right: 'thread',
    })
    expect(parseSessionSplitSides(JSON.stringify({ left: 'chat', right: 'chat' }))).toEqual({
      left: 'terminal',
      right: 'thread',
    })

    const tabKey = sessionViewTabStorageKey('local', 'conv-split-chat')
    const splitKey = sessionViewSplitStorageKey('local', 'conv-split-chat')
    localStorage.setItem(tabKey, 'split')
    localStorage.setItem(splitKey, JSON.stringify({ left: 'chat', right: 'terminal' }))
    const resolved = resolveSessionView(tabKey, splitKey)
    expect(resolved.tab).toBe('split')
    expect(resolved.sides).toEqual({ left: 'thread', right: 'terminal' })
    expect(resolved.migrated).toBe(true)
    expect(overlayViewer(resolved.tab, resolved.sides)).toEqual({
      thread: true,
      chatter: false,
      hidePty: false,
    })
  })

  it('a view with no Chat in it is not flagged for a write-back', () => {
    const tabKey = sessionViewTabStorageKey('local', 'conv-plain')
    const splitKey = sessionViewSplitStorageKey('local', 'conv-plain')
    localStorage.setItem(tabKey, 'split')
    localStorage.setItem(splitKey, JSON.stringify({ left: 'chatter', right: 'thread' }))
    expect(resolveSessionView(tabKey, splitKey).migrated).toBe(false)
    expect(savedViewNamesLegacyChat('thread', null)).toBe(false)
    expect(savedViewNamesLegacyChat('terminal', 'not json')).toBe(false)
    expect(savedViewNamesLegacyChat('chatter', '{"left":"chatter"}')).toBe(false)
    expect(savedViewNamesLegacyChat('chat', null)).toBe(true)
    expect(savedViewNamesLegacyChat('split', '{"right":"chat"}')).toBe(true)
    expect(savedViewNamesLegacyChat(null, '{"view":"chat"}')).toBe(true)
  })

  it('a saved "New agents open in" Chat pick reads back as Thread, Chat sides as the Thread beside the terminal', () => {
    expect(normalizeNewAgentView({ view: 'chat', left: 'terminal', right: 'thread' })).toEqual({
      view: 'thread',
      split: { left: 'terminal', right: 'thread' },
    })
    expect(normalizeNewAgentView({ view: 'split', left: 'chat', right: 'terminal' })).toEqual({
      view: 'split',
      split: { left: 'thread', right: 'terminal' },
    })
    expect(normalizeNewAgentView({ view: 'split', left: 'thread', right: 'chat' })).toEqual({
      view: 'split',
      split: { left: 'thread', right: 'terminal' },
    })
  })

  it('reading a saved "New agents open in" Chat pick writes the Thread choice back', () => {
    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'chat', left: 'terminal', right: 'thread' }))
    expect(readNewAgentView()).toEqual({ view: 'thread', split: { left: 'terminal', right: 'thread' } })
    expect(JSON.parse(localStorage.getItem('k2.newAgentView') ?? 'null')).toEqual({
      view: 'thread',
      left: 'terminal',
      right: 'thread',
    })

    localStorage.setItem('k2.newAgentView', JSON.stringify({ view: 'split', left: 'chat', right: 'chatter' }))
    expect(readNewAgentView()).toEqual({ view: 'split', split: { left: 'thread', right: 'chatter' } })
    expect(localStorage.getItem('k2.newAgentView')).not.toContain('"chat"')
  })

  it('a choice with no Chat in it is read without a write', () => {
    const stored = JSON.stringify({ view: 'chatter', left: 'terminal', right: 'thread' })
    localStorage.setItem('k2.newAgentView', stored)
    expect(readNewAgentView().view).toBe('chatter')
    expect(localStorage.getItem('k2.newAgentView')).toBe(stored)
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

  it('split sides follow the stored pair, including the same view on both sides', () => {
    expect(overlayViewer('split', { left: 'thread', right: 'chatter' })).toEqual({
      thread: true,
      chatter: true,
      hidePty: true,
    })
    expect(overlayViewer('split', { left: 'thread', right: 'thread' })).toEqual({
      thread: true,
      chatter: false,
      hidePty: true,
    })
  })
})
