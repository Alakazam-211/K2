// @vitest-environment jsdom
//
// k2-activity@1 (0.46.5 activity SDK): the real checked-in library, loaded
// the way K2 loads it into a widget frame (K2's resolver, then the file),
// against a fake `k2.thread.onActivity` that pushes what the bridge pushes
// (ThreadActivityState, the App guest's turn from the shared fixture).
import { readFileSync } from 'node:fs'
import { resolve } from 'node:path'
import { afterEach, beforeAll, describe, expect, it, vi } from 'vitest'
import fixture from '../../../../tests/fixtures/activity/guest-turn-parity.json'
import { zenLibResolve } from './zen-lib-loader'

type State = { address: string; turn: unknown; live: boolean; source: string; fresh: string }
type Handlers = { data(v: State): void; error?(e: unknown): void }

type Bar = { el: HTMLElement; setAddress(a: string | null): void; setLook(l: string): void; destroy(): void }
type Lib = {
  version: string
  mount(el: HTMLElement, opts: { address?: string; look?: string; endLingerMs?: number }): Bar
  coerce(raw: unknown, at: number): unknown
  describe(turn: unknown, now: number): { headline: string; animate: boolean; tone: string } | null
}

const LIB = resolve(__dirname, '../../zen-lib/k2-activity@1.0.0/k2-activity.js')
const subs = new Map<string, Handlers>()
let offs = 0
let K2Activity: Lib

const guest = (part: string) => {
  const c = (fixture.cases as Array<{ name: string; guest: Record<string, unknown> }>).find((x) => x.name.includes(part))
  if (!c) throw new Error(part)
  return c.guest
}
const state = (address: string, turn: Record<string, unknown> | null): State => ({
  address,
  turn,
  live: !!turn && turn.end === null,
  source: 'thread',
  fresh: 'live',
})

beforeAll(() => {
  const r = zenLibResolve(['k2-activity@1'])
  expect(r.findings.filter((f) => f.error)).toEqual([])
  expect(r.keys.map(String)).toEqual(['k2-activity@1.0.0'])
  ;(window as unknown as { k2: unknown }).k2 = {
    thread: {
      onActivity(address: string, h: Handlers) {
        subs.set(address, h)
        return () => {
          offs += 1
          subs.delete(address)
        }
      },
    },
  }
  ;(0, eval)(readFileSync(LIB, 'utf8'))
  K2Activity = (window as unknown as { K2Activity: Lib }).K2Activity
  expect(K2Activity.version).toBe('1.0.0')
})

afterEach(() => {
  document.body.replaceChildren()
  vi.useRealTimers()
})

describe('K2Activity (k2-activity@1)', () => {
  it('mounts hidden, listens on k2.thread.onActivity and draws the guest turn in K2’s words', () => {
    const host = document.createElement('div')
    document.body.appendChild(host)
    const bar = K2Activity.mount(host, { address: 'sales::local' })
    expect(bar.el.hidden).toBe(true)
    expect(document.head.querySelectorAll('style[data-k2-lib="k2-activity"]')).toHaveLength(1)
    subs.get('sales::local')!.data(state('sales::local', guest('tool step')))
    expect(bar.el.hidden).toBe(false)
    expect(bar.el.getAttribute('data-tone')).toBe('working')
    expect(bar.el.getAttribute('data-animate')).toBe('true')
    expect(bar.el.querySelector('.k2a-head')!.textContent).toBe('Running a command')
    expect([...bar.el.querySelectorAll('.k2a-chip')].map((c) => c.getAttribute('data-key'))).toEqual(['subagents', 'background', 'read', 'search', 'cmd'])
    bar.destroy()
    expect(offs).toBeGreaterThan(0)
    expect(host.children).toHaveLength(0)
  })

  it('monitoring and needs-you never pulse (no typing dots); a guest waits on the owner', () => {
    const host = document.body.appendChild(document.createElement('div'))
    const bar = K2Activity.mount(host, { address: 'ops::local' })
    subs.get('ops::local')!.data(state('ops::local', guest('monitoring')))
    expect([bar.el.getAttribute('data-tone'), bar.el.getAttribute('data-animate')]).toEqual(['monitoring', 'false'])
    subs.get('ops::local')!.data(state('ops::local', guest('permission')))
    expect([bar.el.getAttribute('data-tone'), bar.el.getAttribute('data-animate')]).toEqual(['needs-you', 'false'])
    expect(bar.el.querySelector('.k2a-head')!.textContent).toBe('Waiting on the owner')
    bar.destroy()
  })

  it('an end shows briefly, then the bar hides; no turn = no bar', () => {
    vi.useFakeTimers()
    const host = document.body.appendChild(document.createElement('div'))
    const bar = K2Activity.mount(host, { address: 'qa::local', endLingerMs: 2_000 })
    subs.get('qa::local')!.data(state('qa::local', { ...guest('done'), turnId: 'msg-x' }))
    expect(bar.el.querySelector('.k2a-head')!.textContent).toBe('Done')
    vi.advanceTimersByTime(3_000)
    expect(bar.el.hidden).toBe(true)
    subs.get('qa::local')!.data(state('qa::local', null))
    expect(bar.el.hidden).toBe(true)
    bar.destroy()
  })

  it('setAddress moves the subscription; text is never parsed as markup', () => {
    const host = document.body.appendChild(document.createElement('div'))
    const bar = K2Activity.mount(host, { address: 'a::local' })
    bar.setAddress('b::local')
    expect(subs.has('a::local')).toBe(false)
    expect(subs.has('b::local')).toBe(true)
    // describe() on an owner-shaped turn with a hostile line stays text.
    const t = K2Activity.coerce({ ...guest('tool step'), line: 'Running `<img src=x onerror=alert(1)>`' }, Date.now())
    expect(K2Activity.describe(t, Date.now())!.headline).toBe('Running `<img src=x onerror=alert(1)>`')
    bar.destroy()
  })
})
