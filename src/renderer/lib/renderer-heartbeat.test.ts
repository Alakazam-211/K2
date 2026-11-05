// The renderer half of the webview watchdog (0.46.9 false reloads): every
// beat says whether the page is visible, a visibility change beats at once,
// and the flag the shell's eval probe checks is set.
import { describe, expect, it } from 'vitest'
import { installRendererHeartbeat, RENDERER_HEARTBEAT_MS } from './renderer-heartbeat'

function fakeDoc(initial: DocumentVisibilityState) {
  const listeners: Array<() => void> = []
  return {
    visibilityState: initial,
    addEventListener: (type: string, fn: () => void) => {
      expect(type).toBe('visibilitychange')
      listeners.push(fn)
    },
    fire(state: DocumentVisibilityState) {
      this.visibilityState = state
      for (const fn of listeners) fn()
    },
  }
}

function harness(initial: DocumentVisibilityState) {
  const calls: Array<{ cmd: string; visible: boolean }> = []
  const timers: Array<{ fn: () => void; ms: number }> = []
  const events: string[] = []
  const doc = fakeDoc(initial)
  const win: { __k2RendererAlive?: boolean } = {}
  installRendererHeartbeat({
    invoke: (cmd, args) => {
      calls.push({ cmd, visible: args.visible })
      return Promise.resolve(null)
    },
    onBeat: () => events.push('beat'),
    onVisibilityChange: () => events.push('visibility'),
    doc: doc as unknown as Pick<Document, 'visibilityState' | 'addEventListener'>,
    win,
    setInterval: (fn, ms) => timers.push({ fn, ms }),
  })
  return { calls, timers, events, doc, win }
}

describe('installRendererHeartbeat', () => {
  it('beats at once with the visibility, then on a 3 s timer', () => {
    const h = harness('visible')
    expect(h.calls).toEqual([{ cmd: 'renderer_heartbeat', visible: true }])
    expect(h.timers).toHaveLength(1)
    expect(h.timers[0].ms).toBe(RENDERER_HEARTBEAT_MS)
    expect(RENDERER_HEARTBEAT_MS).toBe(3000)
    h.timers[0].fn()
    expect(h.calls).toEqual([
      { cmd: 'renderer_heartbeat', visible: true },
      { cmd: 'renderer_heartbeat', visible: true },
    ])
    expect(h.events).toEqual(['beat', 'beat'])
  })

  it('a hidden page says so on every beat', () => {
    const h = harness('hidden')
    h.timers[0].fn()
    expect(h.calls.map((c) => c.visible)).toEqual([false, false])
  })

  it('a visibility change beats at once without counting a timed beat', () => {
    const h = harness('visible')
    h.doc.fire('hidden')
    h.doc.fire('visible')
    expect(h.calls.map((c) => c.visible)).toEqual([true, false, true])
    // Zen counts only timed beats; the visibility hook runs on each change.
    expect(h.events).toEqual(['beat', 'visibility', 'visibility'])
  })

  it('sets the flag the shell eval probe checks', () => {
    const h = harness('visible')
    expect(h.win.__k2RendererAlive).toBe(true)
  })
})
