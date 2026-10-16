// @vitest-environment jsdom
// Fail loud: poll cadence, the S key and Esc from inside the page, stale
// picks after navigation, and disarm on unmount.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'
import { ARMED_POLL_MS, IDLE_POLL_MS, useDesignPicker } from './useDesignPicker'
import type { DesignPickerIpc, PickEvent, PickPoll, PickResult } from './types'

function pick(id: string): PickResult {
  return {
    type: 'pick',
    pickId: id,
    url: 'http://localhost:5173/',
    title: 'Home',
    viewport: { width: 1280, height: 800, scrollX: 0, scrollY: 0, dpr: 2 },
    element: { selector: `#${id}`, tag: 'div', text: '', html: '' },
  }
}

function fakeIpc() {
  const queue: PickPoll[] = []
  const ipc = {
    pickArm: vi.fn(async () => {}),
    pickDisarm: vi.fn(async () => {}),
    pickPoll: vi.fn(async (): Promise<PickPoll> => queue.shift() ?? { events: [], navSeq: 0 }),
    setDesignBadges: vi.fn(async () => {}),
  } satisfies DesignPickerIpc
  const push = (navSeq: number, ...events: PickEvent[]): void => {
    queue.push({ events, navSeq })
  }
  return { ipc, push }
}

/** Advance fake time and let each poll's promise settle. */
async function advance(ms: number): Promise<void> {
  await act(async () => {
    await vi.advanceTimersByTimeAsync(ms)
  })
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

describe('useDesignPicker', () => {
  it('polls every 400 ms while visible and unarmed, never while hidden', async () => {
    const { ipc } = fakeIpc()
    const { rerender } = renderHook((p: { visible: boolean }) =>
      useDesignPicker({ itemId: 'd1', ipc, visible: p.visible, onPick: () => {} }),
    { initialProps: { visible: false } })
    await advance(2000)
    expect(ipc.pickPoll).not.toHaveBeenCalled()
    rerender({ visible: true })
    await advance(IDLE_POLL_MS * 3)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(3)
    expect(ipc.pickArm).not.toHaveBeenCalled()
    expect(ipc.pickDisarm).not.toHaveBeenCalled()
  })

  it('S from the page arms; armed polls every 120 ms even when the tab reports hidden (frozen)', async () => {
    const { ipc, push } = fakeIpc()
    push(1, { type: 'key', key: 'toggle-select' })
    const { result, rerender } = renderHook((p: { visible: boolean }) =>
      useDesignPicker({ itemId: 'd1', parentWindow: 'main', ipc, visible: p.visible, onPick: () => {} }),
    { initialProps: { visible: true } })
    await advance(IDLE_POLL_MS)
    expect(result.current.armed).toBe(true)
    expect(ipc.pickArm).toHaveBeenCalledWith('d1', 'main')
    // The pane freezing while a pop-up is open must not stop polling.
    rerender({ visible: false })
    ipc.pickPoll.mockClear()
    await advance(ARMED_POLL_MS * 5)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(5)
  })

  it('the Select button toggles, Esc from the page disarms', async () => {
    const { ipc, push } = fakeIpc()
    const { result } = renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {} }))
    act(() => result.current.toggle())
    expect(result.current.armed).toBe(true)
    await advance(0)
    expect(ipc.pickArm).toHaveBeenCalledTimes(1)
    push(0, { type: 'key', key: 'escape' })
    await advance(ARMED_POLL_MS)
    expect(result.current.armed).toBe(false)
    expect(ipc.pickDisarm).toHaveBeenCalledTimes(1)
  })

  it('delivers picks and drops picks from an older navigation', async () => {
    const { ipc, push } = fakeIpc()
    const onPick = vi.fn()
    const { result } = renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick }))
    act(() => result.current.setArmed(true))
    push(2, pick('a'))
    await advance(ARMED_POLL_MS)
    push(1, pick('stale'))
    await advance(ARMED_POLL_MS)
    push(3, pick('b'))
    await advance(ARMED_POLL_MS)
    expect(onPick.mock.calls.map((c) => (c[0] as PickResult).pickId)).toEqual(['a', 'b'])
  })

  it('a failed poll reports and keeps polling', async () => {
    const { ipc } = fakeIpc()
    const onError = vi.fn()
    ipc.pickPoll.mockRejectedValueOnce(new Error('webview gone'))
    renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {}, onError }))
    await advance(IDLE_POLL_MS * 2)
    expect(onError).toHaveBeenCalledWith(new Error('webview gone'))
    expect(ipc.pickPoll).toHaveBeenCalledTimes(2)
  })

  it('unmount while armed disarms and stops polling', async () => {
    const { ipc } = fakeIpc()
    const { result, unmount } = renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {} }))
    act(() => result.current.setArmed(true))
    await advance(ARMED_POLL_MS)
    unmount()
    expect(ipc.pickDisarm).toHaveBeenCalledWith('d1', undefined)
    ipc.pickPoll.mockClear()
    await advance(2000)
    expect(ipc.pickPoll).not.toHaveBeenCalled()
  })
})
