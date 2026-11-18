// @vitest-environment jsdom
// Typing in agent Threads, both ways, on the Thread's overlay socket:
//   - the daemon's frame `{collection:"typing", seq:0, id, member, name,
//     kind, at}` parses, and nothing else does;
//   - a name holds 10 s from its member's last frame, and goes at once when
//     that member's post arrives;
//   - the viewer's own member is never shown;
//   - one entry per member (all their devices), oldest first;
//   - the words: one name, two names, "Several people";
//   - this window's signal `{"t":"typing","thread"}` goes only while the
//     box has text, at most every 8 s, and never to a server without
//     `thread-typing-v1`.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'

vi.mock('@/lib/shared-whoami', () => ({
  loadWhoami: vi.fn(async () => null),
  resetSharedWhoamiForTests: () => {},
}))
vi.mock('@/components/Settings/sections/notify-api', () => ({
  fetchNotifyPrefs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
}))

import type { ServerScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import {
  THREAD_TYPING_HOLD_MS,
  THREAD_TYPING_SEND_EVERY_MS,
  clearThreadTypist,
  noteThreadTyping,
  parseThreadTypingFrame,
  registerThreadTypingSocket,
  resetThreadTypingForTests,
  threadTypingWords,
  useThreadTyping,
  useThreadTypingSignal,
} from './thread-typing'

const FRAME = { collection: 'typing', seq: 0, id: 'sales', member: 'mira', name: 'Mira', kind: 'person', at: 1_700_000_000_000 }

function scopeWith(features: string[], hostKey = 'box'): ServerScope {
  return { ...fakeScope(hostKey), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

beforeEach(() => {
  vi.useFakeTimers()
  resetThreadTypingForTests()
  resetThreadViewersForTests()
})
afterEach(() => {
  resetThreadTypingForTests()
  vi.useRealTimers()
})

describe('parseThreadTypingFrame', () => {
  it('reads the daemon frame', () => {
    expect(parseThreadTypingFrame(FRAME)).toEqual({ thread: 'sales', member: 'mira', name: 'Mira', kind: 'person', at: 1_700_000_000_000 })
  })
  it('reads an App guest', () => {
    expect(parseThreadTypingFrame({ ...FRAME, member: 'g-7', name: 'Ana', kind: 'app_guest' })?.kind).toBe('app_guest')
  })
  it('names the member when the frame has no name', () => {
    expect(parseThreadTypingFrame({ ...FRAME, name: '' })?.name).toBe('mira')
  })
  it('is null for other frames and broken ones', () => {
    expect(parseThreadTypingFrame({ ...FRAME, collection: 'activity' })).toBeNull()
    expect(parseThreadTypingFrame({ ...FRAME, member: '' })).toBeNull()
    expect(parseThreadTypingFrame({ ...FRAME, id: undefined })).toBeNull()
    expect(parseThreadTypingFrame('typing')).toBeNull()
    expect(parseThreadTypingFrame(null)).toBeNull()
  })
})

describe('who is typing', () => {
  const scope = scopeWith(['thread-typing-v1'])
  const frame = (member: string, name: string) => parseThreadTypingFrame({ ...FRAME, member, name })!

  it('holds a name 10 s from its last frame', () => {
    const { result } = renderHook(() => useThreadTyping(scope, 'sales'))
    expect(result.current).toEqual([])
    act(() => noteThreadTyping('box', frame('mira', 'Mira'), Date.now()))
    expect(result.current).toEqual(['Mira'])
    act(() => vi.advanceTimersByTime(THREAD_TYPING_HOLD_MS - 2_000))
    // Another frame, 8 s in: 10 s more from here.
    act(() => noteThreadTyping('box', frame('mira', 'Mira'), Date.now()))
    act(() => vi.advanceTimersByTime(THREAD_TYPING_HOLD_MS - 1_000))
    expect(result.current).toEqual(['Mira'])
    act(() => vi.advanceTimersByTime(1_100))
    expect(result.current).toEqual([])
  })

  it('names several, oldest first', () => {
    const { result } = renderHook(() => useThreadTyping(scope, 'sales'))
    act(() => {
      noteThreadTyping('box', frame('mira', 'Mira'))
      noteThreadTyping('box', frame('ben', 'Ben'))
      noteThreadTyping('box', frame('ana', 'Ana'))
    })
    expect(result.current).toEqual(['Mira', 'Ben', 'Ana'])
  })

  it('drops a name at once when that member posts', () => {
    const { result } = renderHook(() => useThreadTyping(scope, 'sales'))
    act(() => {
      noteThreadTyping('box', frame('mira', 'Mira'))
      noteThreadTyping('box', frame('ben', 'Ben'))
    })
    act(() => clearThreadTypist('box', 'sales', 'mira'))
    expect(result.current).toEqual(['Ben'])
  })

  it('keeps each server and Thread apart', () => {
    const { result } = renderHook(() => useThreadTyping(scope, 'ops'))
    act(() => noteThreadTyping('box', frame('mira', 'Mira')))
    expect(result.current).toEqual([])
    const other = renderHook(() => useThreadTyping(scopeWith([], 'elsewhere'), 'sales'))
    expect(other.result.current).toEqual([])
  })

  it('hands back the same names across re-renders while the viewer lookup is pending', () => {
    // A remote with a saved login: the viewer is the guess until the server answers.
    const remote = { ...scopeWith(['thread-typing-v1']), connectHost: () => ({ username: 'alice' }) } as unknown as ServerScope
    const { result, rerender } = renderHook(() => useThreadTyping(remote, 'sales'))
    act(() => noteThreadTyping('box', frame('mira', 'Mira')))
    const first = result.current
    expect(first).toEqual(['Mira'])
    rerender()
    rerender()
    expect(result.current).toBe(first)
  })

  it('never shows the viewer', () => {
    act(() => setThreadViewerForTests(scope, { principal: 'owner', aliases: ['rosson'] }))
    const { result } = renderHook(() => useThreadTyping(scope, 'sales'))
    act(() => {
      noteThreadTyping('box', frame('owner', 'Owner'))
      noteThreadTyping('box', frame('Rosson', 'Rosson'))
      noteThreadTyping('box', frame('mira', 'Mira'))
    })
    expect(result.current).toEqual(['Mira'])
  })
})

describe('one entry per member, oldest first', () => {
  const scope = scopeWith(['thread-typing-v1'])
  const frame = (member: string, name: string) => parseThreadTypingFrame({ ...FRAME, member, name })!

  it('a member on two devices is one name', () => {
    const { result } = renderHook(() => useThreadTyping(scope, 'sales'))
    act(() => {
      noteThreadTyping('box', frame('mira', 'Mira'))
      noteThreadTyping('box', frame('mira', 'Mira'))
    })
    expect(result.current).toEqual(['Mira'])
  })

  it('keeps the order they started in, whoever typed last', () => {
    const { result } = renderHook(() => useThreadTyping(scope, 'sales'))
    act(() => {
      noteThreadTyping('box', frame('mira', 'Mira'))
      noteThreadTyping('box', frame('ben', 'Ben'))
      noteThreadTyping('box', frame('mira', 'Mira'))
    })
    expect(result.current).toEqual(['Mira', 'Ben'])
  })
})

describe('threadTypingWords', () => {
  it('names one, two, and says several for more', () => {
    expect(threadTypingWords([])).toBe('')
    expect(threadTypingWords(['Mira'])).toBe('Mira is typing…')
    expect(threadTypingWords(['Mira', 'Ben'])).toBe('Mira and Ben are typing…')
    expect(threadTypingWords(['Mira', 'Ben', 'Ana'])).toBe('Several people are typing…')
    expect(threadTypingWords(['Mira', 'Ben', 'Ana', 'Lu'])).toBe('Several people are typing…')
  })
})

describe("this window's signal", () => {
  function socket(hostKey: string, thread: string) {
    const sent: string[] = []
    const release = registerThreadTypingSocket(hostKey, () => thread, (f) => {
      sent.push(f)
      return true
    })
    return { sent, release }
  }

  it('says {"t":"typing","thread"} at most every 8 s', () => {
    const s = socket('box', 'sales')
    const { result } = renderHook(() => useThreadTypingSignal(scopeWith(['thread-typing-v1']), 'sales'))
    result.current('H')
    result.current('Hi')
    expect(s.sent).toEqual([JSON.stringify({ t: 'typing', thread: 'sales' })])
    vi.advanceTimersByTime(THREAD_TYPING_SEND_EVERY_MS - 1)
    result.current('Hi t')
    expect(s.sent).toHaveLength(1)
    vi.advanceTimersByTime(1)
    result.current('Hi th')
    expect(s.sent).toHaveLength(2)
    expect(THREAD_TYPING_SEND_EVERY_MS).toBe(8_000)
    s.release()
  })

  it('sends nothing while the box is empty', () => {
    const s = socket('box', 'sales')
    const { result } = renderHook(() => useThreadTypingSignal(scopeWith(['thread-typing-v1']), 'sales'))
    result.current('')
    vi.advanceTimersByTime(THREAD_TYPING_SEND_EVERY_MS)
    result.current('')
    expect(s.sent).toEqual([])
    s.release()
  })

  it('sends nothing to a server without thread-typing-v1', () => {
    const s = socket('box', 'sales')
    const { result } = renderHook(() => useThreadTypingSignal(scopeWith([]), 'sales'))
    result.current('Hi')
    vi.advanceTimersByTime(THREAD_TYPING_SEND_EVERY_MS)
    result.current('Hi there')
    expect(s.sent).toEqual([])
    s.release()
  })

  it('goes only on the socket that reads that Thread on that server', () => {
    const ops = socket('box', 'ops')
    const away = socket('elsewhere', 'sales')
    const sales = socket('box', 'sales')
    const { result } = renderHook(() => useThreadTypingSignal(scopeWith(['thread-typing-v1']), 'sales'))
    result.current('Hi')
    expect(ops.sent).toEqual([])
    expect(away.sent).toEqual([])
    expect(sales.sent).toHaveLength(1)
  })
})
