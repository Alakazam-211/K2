import { describe, expect, it } from 'vitest'
import { terminalClipboardChord } from './terminal-clipboard-chord'

const key = (k: string, mods: Partial<Record<'ctrlKey' | 'shiftKey' | 'altKey' | 'metaKey', boolean>>, code = '') => ({
  key: k,
  code,
  ctrlKey: false,
  shiftKey: false,
  altKey: false,
  metaKey: false,
  ...mods,
})

describe('terminal copy / paste chords', () => {
  it('Windows: Ctrl+Shift+C copies, Ctrl+Shift+V pastes', () => {
    expect(terminalClipboardChord(key('C', { ctrlKey: true, shiftKey: true }, 'KeyC'), 'windows')).toBe('copy')
    expect(terminalClipboardChord(key('V', { ctrlKey: true, shiftKey: true }, 'KeyV'), 'windows')).toBe('paste')
    // A layout where the letter is elsewhere: the character decides.
    expect(terminalClipboardChord(key('C', { ctrlKey: true, shiftKey: true }, 'KeyJ'), 'windows')).toBe('copy')
  })

  it('Windows: plain Ctrl+C / Ctrl+V and other mixes stay with the shell', () => {
    expect(terminalClipboardChord(key('c', { ctrlKey: true }, 'KeyC'), 'windows')).toBeNull()
    expect(terminalClipboardChord(key('v', { ctrlKey: true }, 'KeyV'), 'windows')).toBeNull()
    expect(terminalClipboardChord(key('C', { ctrlKey: true, shiftKey: true, altKey: true }, 'KeyC'), 'windows')).toBeNull()
    expect(terminalClipboardChord(key('X', { ctrlKey: true, shiftKey: true }, 'KeyX'), 'windows')).toBeNull()
  })

  it('macOS and Linux: no chord (⌘C / ⌘V on a Mac; Linux unchanged)', () => {
    for (const os of ['mac', 'linux', 'other'] as const) {
      expect(terminalClipboardChord(key('C', { ctrlKey: true, shiftKey: true }, 'KeyC'), os)).toBeNull()
      expect(terminalClipboardChord(key('V', { ctrlKey: true, shiftKey: true }, 'KeyV'), os)).toBeNull()
    }
  })
})
