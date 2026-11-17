// @vitest-environment jsdom
//
// One call, one home window, across windows: two or three watchers (one per
// window) over the same localStorage, as K2's windows share it. Owner
// election, Show call here, the owner closing (re-election, or the call
// ending when no window is left), and the call's own window.

import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { CALL_HOME_LEASE_MS, noteWindowFocus, readCallHome } from '@/lib/voice/call-home'
import { createCallHomeWatcher, type CallHomeWatcher } from './call-home'

const CALL = 'sewlab.k2.dev|dm:42'

let now = 100_000
let live: string[] = []
const ended: string[] = []

interface Win {
  label: string
  owner: string | null
  watcher: CallHomeWatcher
}

function win(label: string): Win {
  const w: Win = { label, owner: null, watcher: null as unknown as CallHomeWatcher }
  w.watcher = createCallHomeWatcher({
    label,
    call: CALL,
    bridge: { liveWindows: async () => [...live] },
    now: () => now,
    onOwner: (o) => {
      w.owner = o
    },
    onEmpty: () => ended.push(label),
  })
  return w
}

async function tickAll(...ws: Win[]): Promise<void> {
  for (const w of ws) await w.watcher.tick()
}

beforeEach(() => {
  localStorage.clear()
  now = 100_000
  live = []
  ended.length = 0
})
afterEach(() => localStorage.clear())

describe('one call, one home window', () => {
  it('the window the call was started or answered in (focused last) is home; every window agrees', async () => {
    live = ['main', 'window-2']
    noteWindowFocus('main', 1)
    noteWindowFocus('window-2', 2)
    const a = win('main')
    const b = win('window-2')
    await tickAll(a, b)
    expect(a.owner).toBe('window-2')
    expect(b.owner).toBe('window-2')
    expect(readCallHome()).toMatchObject({ call: CALL, owner: 'window-2' })
  })

  it('Show call here moves the stage to that window; the old one shows the slim bar', async () => {
    live = ['main', 'window-2']
    noteWindowFocus('main', 5)
    const a = win('main')
    const b = win('window-2')
    await tickAll(a, b)
    expect(a.owner).toBe('main')
    b.watcher.showHere()
    expect(b.owner).toBe('window-2')
    // The other window hears it (storage event, or its next look).
    a.watcher.decide()
    expect(a.owner).toBe('window-2')
    await tickAll(a, b)
    expect([a.owner, b.owner]).toEqual(['window-2', 'window-2'])
  })

  it('the owner closing: past its lease, the window focused last takes it, never silently dropped', async () => {
    live = ['main', 'window-2', 'focus-a']
    noteWindowFocus('main', 1)
    noteWindowFocus('focus-a', 2)
    noteWindowFocus('window-2', 3)
    const a = win('main')
    const f = win('focus-a')
    const b = win('window-2')
    await tickAll(a, f, b)
    expect(a.owner).toBe('window-2')
    // window-2 closes: the app lists it no more, and it renews no more.
    live = ['main', 'focus-a']
    b.watcher.stop()
    await tickAll(a, f)
    expect(a.owner).toBe('window-2')
    now += CALL_HOME_LEASE_MS + 1
    await tickAll(a, f)
    expect([a.owner, f.owner]).toEqual(['focus-a', 'focus-a'])
    expect(ended).toEqual([])
  })

  it('a background owner that renews late keeps it while it is open', async () => {
    live = ['main', 'window-2']
    noteWindowFocus('main', 5)
    const a = win('main')
    const b = win('window-2')
    await tickAll(a, b)
    now += CALL_HOME_LEASE_MS * 10
    noteWindowFocus('window-2', now)
    await b.watcher.tick()
    expect(b.owner).toBe('main')
  })

  it('no window left (the last one closed or hidden): the call ends, once the owner is surely gone', async () => {
    live = ['main']
    const a = win('main')
    await a.watcher.tick()
    expect(a.owner).toBe('main')
    // The window was closed (hidden, the server kept running): not listed.
    live = []
    now += CALL_HOME_LEASE_MS + 1
    await a.watcher.tick()
    expect(ended).toEqual([])
    await a.watcher.tick()
    expect(ended).toEqual(['main'])
  })

  it("the call's own window holds it while open; closed by the OS, the window it came from takes it back at once", async () => {
    live = ['main', 'window-2']
    noteWindowFocus('main', 9)
    const a = win('main')
    const b = win('window-2')
    await tickAll(a, b)
    // Pop out from window-2.
    b.watcher.handTo('window-call', 'window-2')
    const c = win('window-call')
    live = ['main', 'window-2', 'window-call']
    await tickAll(a, b, c)
    expect([a.owner, b.owner, c.owner]).toEqual(['window-call', 'window-call', 'window-call'])
    // Its window is destroyed (the app's event): no waiting for the lease.
    live = ['main', 'window-2']
    a.watcher.callWindowClosed()
    b.watcher.callWindowClosed()
    await tickAll(a, b)
    expect([a.owner, b.owner]).toEqual(['window-2', 'window-2'])
  })

  it("is never fooled by the call's own window still opening (its fresh lease holds)", async () => {
    live = ['main']
    const a = win('main')
    await a.watcher.tick()
    a.watcher.handTo('window-call', 'main')
    await a.watcher.tick()
    expect(a.owner).toBe('window-call')
  })

  it('the call ending clears its token, from the owner', async () => {
    live = ['main']
    const a = win('main')
    await a.watcher.tick()
    a.watcher.stop({ ended: true })
    expect(readCallHome()).toBeNull()
  })
})
