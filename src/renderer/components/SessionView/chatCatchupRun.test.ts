// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: async () => {
    throw new Error('no reads here')
  },
  daemonCliPost: async () => {
    throw new Error('no posts here')
  },
}))
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import type { ServerScope } from '@/kessel/server-scope'
import { CATCHUP_MIN_GAP_MS, resetChatCatchupRunForTests, runChatCatchup, startChatCatchup, type ChatCatchupTarget } from './chatCatchupRun'

function scopeOf(hostKey: string, catchup = true): ServerScope {
  return { hostKey, isRemote: hostKey !== 'local', serverSupports: (f: string) => catchup && f === 'inbox-catchup-v1' } as unknown as ServerScope
}

let clock = 1_000_000
const targets: ChatCatchupTarget[] = [
  { hostKey: 'local', addr: 'sales' },
  { hostKey: 'local', addr: 'ops' },
  { hostKey: 'local', addr: 'sales' },
  { hostKey: 'b.example.test', addr: 'k2' },
  { hostKey: 'old.example.test', addr: 'k2' },
  { hostKey: 'removed.example.test', addr: 'k2' },
]

function deps() {
  const calls: Array<[string, string[]]> = []
  return {
    calls,
    d: {
      now: () => clock,
      targets: async () => targets,
      scopeFor: (h: string) => {
        if (h === 'removed.example.test') throw new Error('no saved server')
        return scopeOf(h, h !== 'old.example.test')
      },
      catchUp: vi.fn(async (s: ServerScope, addrs: readonly string[]) => {
        calls.push([s.hostKey, [...addrs]])
        return { asked: addrs.length, saved: [], dropped: [] }
      }),
    },
  }
}

beforeEach(() => {
  resetChatCatchupRunForTests()
  clock = 1_000_000
})
afterEach(() => vi.restoreAllMocks())

describe('runChatCatchup', () => {
  it('one call per server that has the catch-up, its Threads once each', async () => {
    const { calls, d } = deps()
    await runChatCatchup(d)
    expect(calls.sort()).toEqual([
      ['b.example.test', ['k2']],
      ['local', ['sales', 'ops']],
    ])
  })

  it('a burst of triggers is one run; again only after the gap', async () => {
    const { d } = deps()
    await Promise.all([runChatCatchup(d), runChatCatchup(d), runChatCatchup(d)])
    expect(d.catchUp).toHaveBeenCalledTimes(2)
    clock += CATCHUP_MIN_GAP_MS - 1
    await runChatCatchup(d)
    expect(d.catchUp).toHaveBeenCalledTimes(2)
    clock += 1
    await runChatCatchup(d)
    expect(d.catchUp).toHaveBeenCalledTimes(4)
  })

  it('runs on launch, and on focus and on the window becoming visible', async () => {
    const { d } = deps()
    const stop = startChatCatchup(d)
    await vi.waitFor(() => expect(d.catchUp).toHaveBeenCalledTimes(2))
    clock += CATCHUP_MIN_GAP_MS
    window.dispatchEvent(new Event('focus'))
    await vi.waitFor(() => expect(d.catchUp).toHaveBeenCalledTimes(4))
    clock += CATCHUP_MIN_GAP_MS
    document.dispatchEvent(new Event('visibilitychange'))
    await vi.waitFor(() => expect(d.catchUp).toHaveBeenCalledTimes(6))
    stop()
    clock += CATCHUP_MIN_GAP_MS
    window.dispatchEvent(new Event('focus'))
    await new Promise((r) => setTimeout(r, 5))
    expect(d.catchUp).toHaveBeenCalledTimes(6)
  })
})
