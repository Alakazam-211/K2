// The computer's keyboard → the device (sim-keyboard.ts), fail loud:
//  - printable keys are text; Return/Backspace/Delete/Tab/arrows are keys,
//    with Shift and Option riding along;
//  - ⌘V is the paste chord; ⌘A/⌘X/⌘C/⌘Z/⇧⌘Z are edit chords inside the
//    device; ⇧⌘H Home, ⌘D the dev menu; every other ⌘ chord stays with K2;
//  - Ctrl is the chord key off the Mac (AltGr characters still type);
//  - an input method composes: no keystroke goes out while it does, and
//    the commit is one frame (a paste when long or multi-line);
//  - pastes are capped at 64 KB and lose stray control characters.

import { describe, expect, it } from 'vitest'
import { commitInput, deviceChords, keyVerdict, type KeyLike, PASTE_MAX_BYTES, pasteInput, TEXT_MAX_CHARS } from './sim-keyboard'

const k = (key: string, mods: Partial<KeyLike> = {}, mac = true): ReturnType<typeof keyVerdict> =>
  keyVerdict({ key, metaKey: false, ctrlKey: false, altKey: false, shiftKey: false, ...mods }, mac)

describe('keys while the device has focus', () => {
  it('printable characters are text, as Shift and Option made them', () => {
    expect(k('a')).toEqual({ send: { t: 'text', text: 'a' } })
    expect(k('A', { shiftKey: true })).toEqual({ send: { t: 'text', text: 'A' } })
    expect(k('é')).toEqual({ send: { t: 'text', text: 'é' } })
    expect(k('∂', { altKey: true })).toEqual({ send: { t: 'text', text: '∂' } })
    expect(k(' ')).toEqual({ send: { t: 'text', text: ' ' } })
    expect(k('😀')).toEqual({ send: { t: 'text', text: '😀' } })
  })

  it('named keys go as keys, with Shift and Option', () => {
    expect(k('Enter')).toEqual({ send: { t: 'key', key: 'enter' } })
    expect(k('Backspace')).toEqual({ send: { t: 'key', key: 'backspace' } })
    expect(k('Delete')).toEqual({ send: { t: 'key', key: 'delete' } })
    expect(k('Tab')).toEqual({ send: { t: 'key', key: 'tab' } })
    expect(k('Tab', { shiftKey: true })).toEqual({ send: { t: 'key', key: 'tab', shift: true } })
    expect(k('ArrowLeft', { shiftKey: true })).toEqual({ send: { t: 'key', key: 'arrow-left', shift: true } })
    expect(k('ArrowRight', { altKey: true })).toEqual({ send: { t: 'key', key: 'arrow-right', alt: true } })
    expect(k('Backspace', { altKey: true })).toEqual({ send: { t: 'key', key: 'backspace', alt: true } })
    expect(k('ArrowUp', { shiftKey: true, altKey: true })).toEqual({ send: { t: 'key', key: 'arrow-up', shift: true, alt: true } })
  })

  it('⌘ chords: paste, edit chords and the device buttons go to the device; the rest stay with K2', () => {
    expect(k('v', { metaKey: true })).toBe('paste')
    expect(k('V', { metaKey: true })).toBe('paste')
    expect(k('a', { metaKey: true })).toEqual({ send: { t: 'edit', op: 'select-all' } })
    expect(k('x', { metaKey: true })).toEqual({ send: { t: 'edit', op: 'cut' } })
    expect(k('c', { metaKey: true })).toEqual({ send: { t: 'edit', op: 'copy' } })
    expect(k('z', { metaKey: true })).toEqual({ send: { t: 'edit', op: 'undo' } })
    expect(k('Z', { metaKey: true, shiftKey: true })).toEqual({ send: { t: 'edit', op: 'redo' } })
    expect(k('H', { metaKey: true, shiftKey: true })).toEqual({ send: { t: 'button', button: 'home' } })
    expect(k('d', { metaKey: true })).toEqual({ send: { t: 'button', button: 'dev-menu' } })
    // K2's own: close tab, new tab, Cmd+L, Cmd+K, settings, tab 1, zoom, palette…
    for (const chord of ['w', 't', 'l', 'k', ',', '1', '9', '=', '-', '0', 'n', 'p', 'f', 'r', 'q', '[', ']', 'Enter', 'ArrowLeft']) {
      expect(k(chord, { metaKey: true }), `⌘${chord}`).toBe('pass')
    }
    expect(k('V', { metaKey: true, shiftKey: true }), '⇧⌘V').toBe('pass')
    expect(k('v', { metaKey: true, altKey: true }), '⌥⌘V').toBe('pass')
    expect(k('A', { metaKey: true, shiftKey: true }), '⇧⌘A').toBe('pass')
    expect(k('D', { metaKey: true, shiftKey: true }), '⇧⌘D').toBe('pass')
    expect(k('v', { metaKey: true, ctrlKey: true }), '⌃⌘V').toBe('pass')
    // Ctrl on a Mac is never a device chord (Ctrl+Tab switches K2 tabs).
    expect(k('c', { ctrlKey: true })).toBe('pass')
    expect(k('Tab', { ctrlKey: true })).toBe('pass')
  })

  it('off the Mac, Ctrl is the chord key and AltGr characters still type', () => {
    expect(k('v', { ctrlKey: true }, false)).toBe('paste')
    expect(k('a', { ctrlKey: true }, false)).toEqual({ send: { t: 'edit', op: 'select-all' } })
    expect(k('w', { ctrlKey: true }, false)).toBe('pass')
    expect(k('v', { metaKey: true }, false), 'the Windows key stays with the system').toBe('pass')
    expect(k('@', { ctrlKey: true, altKey: true }, false), 'AltGr+Q on a German keyboard').toEqual({ send: { t: 'text', text: '@' } })
    expect(k('Enter', { ctrlKey: true }, false)).toBe('pass')
  })

  it('Esc stops typing into the device; lone modifiers, F-keys and Escape chords stay with K2', () => {
    expect(k('Escape')).toBe('blur')
    expect(k('Escape', { metaKey: true })).toBe('pass')
    expect(k('Shift', { shiftKey: true })).toBe('pass')
    expect(k('Meta', { metaKey: true })).toBe('pass')
    expect(k('Alt', { altKey: true })).toBe('pass')
    expect(k('F5')).toBe('pass')
    expect(k('CapsLock')).toBe('pass')
  })

  it('while an input method composes nothing is sent (isComposing, keyCode 229, Process, dead keys)', () => {
    expect(k('a', { isComposing: true })).toBe('ime')
    expect(k('Enter', { keyCode: 229 }), 'Safari’s commit Enter').toBe('ime')
    expect(k('Process')).toBe('ime')
    expect(k('Dead', { altKey: true }), 'Option+E').toBe('ime')
    expect(k('Escape', { isComposing: true }), 'Esc cancels the composition, not the focus').toBe('ime')
    expect(k('Backspace', { isComposing: true })).toBe('ime')
  })
})

describe('what is sent for a commit or a paste', () => {
  it('an IME commit is one text frame; long or multi-line commits are a paste; empty is nothing', () => {
    expect(commitInput('日本語')).toEqual({ t: 'text', text: '日本語' })
    expect(commitInput('é')).toEqual({ t: 'text', text: 'é' })
    expect(commitInput('')).toBeNull()
    expect(commitInput(null)).toBeNull()
    expect(commitInput('\u0007')).toBeNull()
    const long = 'あ'.repeat(TEXT_MAX_CHARS + 1)
    expect(commitInput(long)).toEqual({ t: 'paste', text: long })
    expect(commitInput('a'.repeat(TEXT_MAX_CHARS))).toEqual({ t: 'text', text: 'a'.repeat(TEXT_MAX_CHARS) })
    expect(commitInput('two\nlines')).toEqual({ t: 'paste', text: 'two\nlines' })
  })

  it('a paste keeps lines and tabs, drops stray control characters, and refuses over 64 KB', () => {
    expect(pasteInput('line 1\nline 2\tx')).toEqual({ send: { t: 'paste', text: 'line 1\nline 2\tx' } })
    expect(pasteInput('a\u0000b\u001bc')).toEqual({ send: { t: 'paste', text: 'abc' } })
    expect(pasteInput('')).toBeNull()
    expect(pasteInput(undefined)).toBeNull()
    expect(pasteInput('x'.repeat(PASTE_MAX_BYTES))).toEqual({ send: { t: 'paste', text: 'x'.repeat(PASTE_MAX_BYTES) } })
    const big = pasteInput('x'.repeat(PASTE_MAX_BYTES + 1))
    expect(big).toEqual({ error: 'The clipboard text is over 64 KB; paste less at a time.' })
    // Bytes, not characters: 3-byte characters hit the cap sooner.
    expect(pasteInput('日'.repeat(Math.floor(PASTE_MAX_BYTES / 3) + 1))).toMatchObject({ error: expect.stringContaining('64 KB') })
  })

  it('the ⋯ help spells the chords for this computer', () => {
    const mac = deviceChords(true).map((c) => c.keys)
    expect(mac).toContain('⌘V')
    expect(mac).toContain('⌘Z ⇧⌘Z')
    const pc = deviceChords(false).map((c) => c.keys)
    expect(pc).toContain('Ctrl+V')
    expect(pc).toContain('Ctrl+Z Ctrl+Shift+Z')
    expect(pc.join(' ')).not.toContain('⌘')
  })
})
