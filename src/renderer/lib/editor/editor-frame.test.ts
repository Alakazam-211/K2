// @vitest-environment jsdom
// The editor frame's pure parts (build-editors.md E1b): the preview switch
// and its gate, the modes and their keys, and the status line's counts.

import { afterEach, describe, expect, it } from 'vitest'
import { initialMode, modeForKey, usableMode } from './modes'
import { countWords, lineEnding, readingMinutes } from './words'
import { EDITOR_KEYS, LEGACY_EDITOR_PREVIEW_KEY, migrateLegacyEditorPreview, newEditorKind, readEditorPreview, setEditorPreview, subscribeEditorPreview } from './preview-switch'
import { readLocalFlag, setLocalFlag } from './local-flag'

const key = (k: string, mods: { meta?: boolean; ctrl?: boolean; shift?: boolean; alt?: boolean } = {}) => ({
  key: k,
  metaKey: !!mods.meta,
  ctrlKey: !!mods.ctrl,
  shiftKey: !!mods.shift,
  altKey: !!mods.alt,
})

afterEach(() => window.localStorage.clear())

describe('the preview switches', () => {
  it('each is off by default, kept per computer, and tells listeners', () => {
    for (const kind of ['markdown', 'code'] as const) {
      expect(readEditorPreview(kind)).toBe(false)
      let told = 0
      const off = subscribeEditorPreview(() => told++)
      setEditorPreview(kind, true)
      expect(window.localStorage.getItem(EDITOR_KEYS[kind])).toBe('1')
      expect(readEditorPreview(kind)).toBe(true)
      expect(readEditorPreview(kind === 'markdown' ? 'code' : 'markdown')).toBe(false)
      setEditorPreview(kind, false)
      expect(window.localStorage.getItem(EDITOR_KEYS[kind])).toBeNull()
      expect(told).toBe(2)
      off()
    }
  })

  it('the old single switch, on, turns both on once, and goes', () => {
    window.localStorage.setItem(LEGACY_EDITOR_PREVIEW_KEY, '1')
    migrateLegacyEditorPreview()
    expect(window.localStorage.getItem(LEGACY_EDITOR_PREVIEW_KEY)).toBeNull()
    expect(readEditorPreview('markdown')).toBe(true)
    expect(readEditorPreview('code')).toBe(true)
    setEditorPreview('code', false)
    migrateLegacyEditorPreview()
    expect(readEditorPreview('code')).toBe(false)
  })

  it('the old switch, off or missing: both stay off', () => {
    window.localStorage.setItem(LEGACY_EDITOR_PREVIEW_KEY, '0')
    migrateLegacyEditorPreview()
    expect(window.localStorage.getItem(LEGACY_EDITOR_PREVIEW_KEY)).toBeNull()
    expect(readEditorPreview('markdown')).toBe(false)
    expect(readEditorPreview('code')).toBe(false)
  })

  it('markdown goes to the markdown editor; every text file K2 edits in CodeMirror to the code editor', () => {
    expect(newEditorKind('markdown')).toBe('markdown')
    for (const c of ['text', 'html', 'csv', 'mermaid', 'diagram', 'cad2d'] as const) expect(newEditorKind(c)).toBe('code')
    for (const c of ['image', 'pdf', 'docx', 'audio', 'video', 'zip', 'model3d', 'board', 'binary'] as const) expect(newEditorKind(c)).toBeNull()
  })

  it('a flag with a default reads it until set', () => {
    expect(readLocalFlag('k2.test.flag', true)).toBe(true)
    setLocalFlag('k2.test.flag', false, true)
    expect(readLocalFlag('k2.test.flag', true)).toBe(false)
  })
})

describe('modes', () => {
  it('markdown opens in Write, a read-only file in Read, code in Source', () => {
    expect(initialMode('markdown', false)).toBe('write')
    expect(initialMode('markdown', true)).toBe('read')
    expect(initialMode('code', false)).toBe('source')
    expect(initialMode('code', true)).toBe('source')
  })

  it('⌘E toggles Write and Read; ⇧⌘E opens Source', () => {
    expect(modeForKey('write', key('e', { meta: true }), false)).toBe('read')
    expect(modeForKey('read', key('e', { meta: true }), false)).toBe('write')
    expect(modeForKey('source', key('e', { meta: true }), false)).toBe('write')
    expect(modeForKey('write', key('E', { meta: true, shift: true }), false)).toBe('source')
    expect(modeForKey('read', key('e', { ctrl: true, shift: true }), false)).toBe('source')
    expect(modeForKey('write', key('e'), false)).toBeNull()
    expect(modeForKey('write', key('e', { meta: true, alt: true }), false)).toBeNull()
    expect(modeForKey('write', key('f', { meta: true }), false)).toBeNull()
  })

  it('read only: ⌘E never reaches Write; Source stays', () => {
    expect(modeForKey('read', key('e', { meta: true }), true)).toBeNull()
    expect(modeForKey('source', key('e', { meta: true }), true)).toBe('read')
    expect(modeForKey('read', key('e', { meta: true, shift: true }), true)).toBe('source')
    expect(usableMode('write', true)).toBe('read')
    expect(usableMode('source', true)).toBe('source')
    expect(usableMode('write', false)).toBe('write')
  })
})

describe('words', () => {
  it('counts words, not markup', () => {
    expect(countWords('')).toBe(0)
    expect(countWords('# Quill\n\nQuill drafts, edits and *fact-checks* writing.\n\n- [ ] Add the checker\n| a | b |\n|---|---|\n')).toBe(12)
    expect(countWords('```ts\nconst x = 1\n```')).toBe(3)
  })

  it('reading time is at least a minute, at 230 words a minute', () => {
    expect(readingMinutes(0)).toBe(1)
    expect(readingMinutes(155)).toBe(1)
    expect(readingMinutes(460)).toBe(2)
    expect(readingMinutes(1000)).toBe(4)
  })

  it('line endings', () => {
    expect(lineEnding('a\nb\n')).toBe('LF')
    expect(lineEnding('a\r\nb\r\n')).toBe('CRLF')
    expect(lineEnding('one line')).toBe('LF')
  })
})
