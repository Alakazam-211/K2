// @vitest-environment jsdom
// Fail loud: Select on when the tab opens, the palette toggle, poll
// cadence, Esc from inside the page (no S key), arm retry before the page
// exists, stale picks after navigation, and disarm on unmount.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, renderHook } from '@testing-library/react'
import { ARMED_POLL_MS, useDesignPicker } from './useDesignPicker'
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
  it('Select is on when the tab opens: arms at once and polls every 120 ms', async () => {
    const { ipc } = fakeIpc()
    const { result } = renderHook(() =>
      useDesignPicker({ itemId: 'd1', parentWindow: 'main', ipc, visible: true, onPick: () => {} }),
    )
    expect(result.current.armed).toBe(true)
    await advance(0)
    expect(ipc.pickArm).toHaveBeenCalledWith('d1', 'main')
    expect(ipc.pickArm).toHaveBeenCalledTimes(1)
    await advance(ARMED_POLL_MS * 5)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(5)
    expect(ipc.pickArm).toHaveBeenCalledTimes(1)
    expect(ipc.pickDisarm).not.toHaveBeenCalled()
  })

  it('an arm before the page exists is retried before the next poll until it lands', async () => {
    const { ipc } = fakeIpc()
    ipc.pickArm.mockRejectedValueOnce(new Error('no such browser view'))
    ipc.pickArm.mockRejectedValueOnce(new Error('no such browser view'))
    const onError = vi.fn()
    renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {}, onError }))
    await advance(0)
    expect(onError).toHaveBeenCalledWith(new Error('no such browser view'))
    await advance(ARMED_POLL_MS)
    // The retry failed too: no poll yet.
    expect(ipc.pickArm).toHaveBeenCalledTimes(2)
    expect(ipc.pickPoll).not.toHaveBeenCalled()
    await advance(ARMED_POLL_MS)
    expect(ipc.pickArm).toHaveBeenCalledTimes(3)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(1)
    await advance(ARMED_POLL_MS * 2)
    expect(ipc.pickArm).toHaveBeenCalledTimes(3)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(3)
  })

  it('the palette toggles: off disarms and stops polling, on arms again', async () => {
    const { ipc } = fakeIpc()
    const { result } = renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {} }))
    await advance(ARMED_POLL_MS)
    act(() => result.current.toggle())
    expect(result.current.armed).toBe(false)
    await advance(0)
    expect(ipc.pickDisarm).toHaveBeenCalledTimes(1)
    ipc.pickPoll.mockClear()
    await advance(2000)
    expect(ipc.pickPoll).not.toHaveBeenCalled()
    act(() => result.current.toggle())
    expect(result.current.armed).toBe(true)
    await advance(ARMED_POLL_MS)
    expect(ipc.pickArm).toHaveBeenCalledTimes(2)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(1)
  })

  it('a hidden tab or an unarmed start never polls', async () => {
    const { ipc } = fakeIpc()
    const { rerender } = renderHook((p: { visible: boolean }) =>
      useDesignPicker({ itemId: 'd1', ipc, visible: p.visible, onPick: () => {} }),
    { initialProps: { visible: false } })
    await advance(2000)
    expect(ipc.pickPoll).not.toHaveBeenCalled()
    rerender({ visible: true })
    await advance(ARMED_POLL_MS * 2)
    expect(ipc.pickPoll).toHaveBeenCalledTimes(2)

    const other = fakeIpc()
    renderHook(() => useDesignPicker({ itemId: 'd2', ipc: other.ipc, visible: true, onPick: () => {}, initialArmed: false }))
    await advance(2000)
    expect(other.ipc.pickPoll).not.toHaveBeenCalled()
    expect(other.ipc.pickArm).not.toHaveBeenCalled()
  })

  it('Esc from the page disarms; an old page\'s S toggle does nothing (no S key)', async () => {
    const { ipc, push } = fakeIpc()
    const { result } = renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {} }))
    push(0, { type: 'key', key: 'toggle-select' } as unknown as PickEvent)
    await advance(ARMED_POLL_MS)
    expect(result.current.armed).toBe(true)
    push(0, { type: 'key', key: 'escape' })
    await advance(ARMED_POLL_MS)
    expect(result.current.armed).toBe(false)
    expect(ipc.pickDisarm).toHaveBeenCalledTimes(1)
  })

  it('delivers picks and drops picks from an older navigation', async () => {
    const { ipc, push } = fakeIpc()
    const onPick = vi.fn()
    renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick }))
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
    await advance(ARMED_POLL_MS * 2)
    expect(onError).toHaveBeenCalledWith(new Error('webview gone'))
    expect(ipc.pickPoll).toHaveBeenCalledTimes(2)
  })

  it('unmount while armed disarms and stops polling', async () => {
    const { ipc } = fakeIpc()
    const { unmount } = renderHook(() => useDesignPicker({ itemId: 'd1', ipc, visible: true, onPick: () => {} }))
    await advance(ARMED_POLL_MS)
    unmount()
    expect(ipc.pickDisarm).toHaveBeenCalledWith('d1', undefined)
    ipc.pickPoll.mockClear()
    await advance(2000)
    expect(ipc.pickPoll).not.toHaveBeenCalled()
  })
})
