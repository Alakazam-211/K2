// @vitest-environment jsdom
import { describe, it, expect } from 'vitest'
import { keyEventToSequence } from './key-mapping'

function keyEvent(partial: Partial<KeyboardEvent> & { key: string }): KeyboardEvent {
  return {
    key: partial.key,
    ctrlKey: partial.ctrlKey ?? false,
    metaKey: partial.metaKey ?? false,
    altKey: partial.altKey ?? false,
    shiftKey: partial.shiftKey ?? false,
  } as KeyboardEvent
}

describe('keyEventToSequence — paste chords leave the browser paste path free', () => {
  it('Ctrl+V returns null (do not send 0x16 / SYN — web + Windows paste)', () => {
    expect(keyEventToSequence(keyEvent({ key: 'v', ctrlKey: true }))).toBeNull()
    expect(keyEventToSequence(keyEvent({ key: 'V', ctrlKey: true }))).toBeNull()
    expect(
      keyEventToSequence(keyEvent({ key: 'v', ctrlKey: true, shiftKey: true })),
    ).toBeNull()
  })

  it('Cmd+V returns null (macOS paste — already covered by meta early-out)', () => {
    expect(keyEventToSequence(keyEvent({ key: 'v', metaKey: true }))).toBeNull()
  })

  it('Ctrl+C still maps to ETX (interrupt) — not copy', () => {
    expect(keyEventToSequence(keyEvent({ key: 'c', ctrlKey: true }))).toBe('\x03')
  })

  it('plain v still sends the character', () => {
    expect(keyEventToSequence(keyEvent({ key: 'v' }))).toBe('v')
  })
})
