// Both Shift keys pressed together opens Send feedback (Rosson 2026-11-11).
//
// The chord fires once when the second Shift goes down while the first is
// still held and no other key was pressed in between, so Shift+typing,
// one-handed Shift and key repeat never trigger it. It listens in the
// capture phase on the window, so a focused terminal or editor can't
// swallow it, and it never prevents the keys' default behaviour.

type Side = 'ShiftLeft' | 'ShiftRight'

/** The chord's state machine, kept pure for tests. */
export class BothShiftsDetector {
  private held = new Set<Side>()
  private spoiled = false
  private fired = false

  /** A key went down. True when this press completes the chord. */
  down(code: string, repeat = false): boolean {
    if (code !== 'ShiftLeft' && code !== 'ShiftRight') {
      // Any other key while a Shift is held means the person is typing.
      if (this.held.size > 0) this.spoiled = true
      return false
    }
    if (repeat || this.held.has(code)) return false
    if (this.held.size === 0) {
      this.spoiled = false
      this.fired = false
    }
    this.held.add(code)
    if (this.held.size === 2 && !this.spoiled && !this.fired) {
      this.fired = true
      return true
    }
    return false
  }

  /** A key came up. */
  up(code: string): void {
    if (code === 'ShiftLeft' || code === 'ShiftRight') this.held.delete(code)
    if (this.held.size === 0) {
      this.spoiled = false
      this.fired = false
    }
  }

  /** The window lost focus: forget what was held. */
  reset(): void {
    this.held.clear()
    this.spoiled = false
    this.fired = false
  }
}

/** Listen on `target` and call `onChord` each time both Shifts go down
 *  together. Returns the cleanup. */
export function installBothShifts(onChord: () => void, target: Window = window): () => void {
  const d = new BothShiftsDetector()
  const onDown = (e: KeyboardEvent): void => {
    if (d.down(e.code, e.repeat)) onChord()
  }
  const onUp = (e: KeyboardEvent): void => d.up(e.code)
  const onBlur = (): void => d.reset()
  target.addEventListener('keydown', onDown, true)
  target.addEventListener('keyup', onUp, true)
  target.addEventListener('blur', onBlur)
  return () => {
    target.removeEventListener('keydown', onDown, true)
    target.removeEventListener('keyup', onUp, true)
    target.removeEventListener('blur', onBlur)
  }
}
