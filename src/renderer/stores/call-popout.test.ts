// @vitest-environment jsdom
//
// Pop out (Rosson 2026-11-10): the call's own window. The store follows it
// over a fake bridge: opening it hides the tile at once, the window's own
// open/closed event is the truth, a window opened before this one is caught
// up with, and a failure to open puts the call back and says why.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { CALL_WINDOW_FRAME_KEY } from '@/lib/voice/call-view'
import { __resetCallPopoutForTests, popOutCall, setCallPopoutBridge, useCallPopout, watchCallPopout, type CallPopoutBridge } from './call-popout'

function fakeBridge(opts: { openNow?: boolean; failOpen?: string } = {}) {
  let handler: ((open: boolean) => void) | null = null
  const opened: unknown[] = []
  let unsubscribed = 0
  const bridge: CallPopoutBridge = {
    open: async (frame) => {
      opened.push(frame)
      if (opts.failOpen) throw new Error(opts.failOpen)
    },
    isOpen: async () => opts.openNow ?? false,
    watch: async (on) => {
      handler = on
      return () => {
        unsubscribed += 1
        handler = null
      }
    },
  }
  return {
    bridge,
    opened,
    emit: (open: boolean) => {
      if (!handler) throw new Error('nobody is watching')
      handler(open)
    },
    unsubscribed: () => unsubscribed,
  }
}

const flush = () => new Promise((r) => setTimeout(r, 0))

beforeEach(() => {
  localStorage.clear()
  __resetCallPopoutForTests()
})
afterEach(() => __resetCallPopoutForTests())

describe('the call in its own window', () => {
  it('is unavailable without the app (the web client, tests)', async () => {
    expect(useCallPopout.getState().available).toBe(false)
    await popOutCall()
    expect(useCallPopout.getState().open).toBe(false)
  })

  it("opens at the frame kept for this client, and the window's events are the truth", async () => {
    const f = fakeBridge()
    setCallPopoutBridge(f.bridge)
    const stop = watchCallPopout()
    await flush()
    localStorage.setItem(CALL_WINDOW_FRAME_KEY, JSON.stringify({ x: 40, y: 50, width: 900, height: 600 }))
    await popOutCall()
    expect(f.opened).toEqual([{ x: 40, y: 50, width: 900, height: 600 }])
    expect(useCallPopout.getState().open).toBe(true)
    // The window closed (its close button, ⌘W, or it crashed): the call comes back.
    f.emit(false)
    expect(useCallPopout.getState().open).toBe(false)
    f.emit(true)
    expect(useCallPopout.getState().open).toBe(true)
    stop()
    expect(f.unsubscribed()).toBe(1)
  })

  it('catches up with a window opened before this one', async () => {
    const f = fakeBridge({ openNow: true })
    setCallPopoutBridge(f.bridge)
    const stop = watchCallPopout()
    await flush()
    expect(useCallPopout.getState().open).toBe(true)
    stop()
  })

  it('a window that would not open puts the call back and says why', async () => {
    const f = fakeBridge({ failOpen: 'no display' })
    setCallPopoutBridge(f.bridge)
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    await popOutCall()
    expect(useCallPopout.getState().open).toBe(false)
    expect(useCallPopout.getState().error).toBe("Couldn't open the call in its own window: no display")
    expect(warn).toHaveBeenCalled()
  })

  it('one watch however many windows parts mount it', async () => {
    const f = fakeBridge()
    const watch = vi.spyOn(f.bridge, 'watch')
    setCallPopoutBridge(f.bridge)
    const a = watchCallPopout()
    const b = watchCallPopout()
    await flush()
    expect(watch).toHaveBeenCalledTimes(1)
    a()
    expect(f.unsubscribed()).toBe(0)
    b()
    expect(f.unsubscribed()).toBe(1)
  })
})
