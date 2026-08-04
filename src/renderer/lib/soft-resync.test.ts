// Soft-resync bus + R1 WS-drop debounce unit tests (fail loud — no swallow).

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import {
  shouldEmitSoftResync,
  subscribeSoftResync,
  emitSoftResync,
  resetSoftResyncBus,
  SOFT_RESYNC_COALESCE_MS,
} from './soft-resync'
import {
  noteRemoteEventsClosed,
  noteRemoteEventsOpened,
  resetRemoteEventsDropState,
  isRemoteEventsDropDebounceArmed,
  hadRemoteEventsPriorClose,
  REMOTE_EVENTS_DROP_DEBOUNCE_MS,
} from './remote-ws-drop'
import {
  forceSoftHealthProbe,
  registerRemoteHealthControls,
  stampRemoteReconnectFlap,
  wasR1FlapStampedThisEpisode,
  clearR1FlapEpisode,
} from './connection-gate-probe'
import { useConnectHostStore } from '@/stores/connect-host'

describe('shouldEmitSoftResync', () => {
  it('true only on remote edge into connected', () => {
    expect(
      shouldEmitSoftResync(
        { kind: 'reconnecting' },
        { kind: 'connected' },
        { isRemote: true },
      ),
    ).toBe(true)
    expect(
      shouldEmitSoftResync(
        { kind: 'reauthenticating' },
        { kind: 'connected' },
        { isRemote: true },
      ),
    ).toBe(true)
    expect(
      shouldEmitSoftResync(
        { kind: 'wedged' },
        { kind: 'connected' },
        { isRemote: true },
      ),
    ).toBe(true)
  })

  it('false for local host even on edge into connected', () => {
    expect(
      shouldEmitSoftResync(
        { kind: 'reconnecting' },
        { kind: 'connected' },
        { isRemote: false },
      ),
    ).toBe(false)
  })

  it('false for connected→connected dedupe', () => {
    expect(
      shouldEmitSoftResync(
        { kind: 'connected' },
        { kind: 'connected' },
        { isRemote: true },
      ),
    ).toBe(false)
  })

  it('false when next is not connected', () => {
    expect(
      shouldEmitSoftResync(
        { kind: 'connected' },
        { kind: 'reconnecting' },
        { isRemote: true },
      ),
    ).toBe(false)
    expect(
      shouldEmitSoftResync(
        { kind: 'reconnecting' },
        { kind: 'wedged' },
        { isRemote: true },
      ),
    ).toBe(false)
  })
})

describe('soft-resync bus', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    resetSoftResyncBus()
  })
  afterEach(() => {
    resetSoftResyncBus()
    vi.useRealTimers()
  })

  it('fans out to all subscribers and is safe with zero subscribers', () => {
    emitSoftResync('recovery-connected')
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    // no throw with zero subscribers

    const a = vi.fn()
    const b = vi.fn()
    const unsubA = subscribeSoftResync(a)
    subscribeSoftResync(b)
    emitSoftResync('events-reopen')
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(a).toHaveBeenCalledTimes(1)
    expect(a).toHaveBeenCalledWith('events-reopen')
    expect(b).toHaveBeenCalledTimes(1)
    expect(b).toHaveBeenCalledWith('events-reopen')

    unsubA()
    emitSoftResync('recovery-connected')
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(a).toHaveBeenCalledTimes(1) // unsubscribed
    expect(b).toHaveBeenCalledTimes(2)
  })

  it('coalesces same reason within the coalesce window', () => {
    const cb = vi.fn()
    subscribeSoftResync(cb)
    emitSoftResync('recovery-connected')
    emitSoftResync('recovery-connected')
    emitSoftResync('recovery-connected')
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(cb).toHaveBeenCalledTimes(1)
    expect(cb).toHaveBeenCalledWith('recovery-connected')
  })

  it('delivers distinct reasons that arrived in the same window', () => {
    const cb = vi.fn()
    subscribeSoftResync(cb)
    emitSoftResync('recovery-connected')
    emitSoftResync('events-reopen')
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(cb).toHaveBeenCalledTimes(2)
    expect(cb).toHaveBeenCalledWith('recovery-connected')
    expect(cb).toHaveBeenCalledWith('events-reopen')
  })
})

describe('connection-gate-probe registration', () => {
  beforeEach(() => {
    clearR1FlapEpisode()
    registerRemoteHealthControls(null)
  })
  afterEach(() => {
    clearR1FlapEpisode()
    registerRemoteHealthControls(null)
  })

  it('forceSoftHealthProbe calls registered fn; no-op when unregistered', () => {
    expect(() => forceSoftHealthProbe()).not.toThrow()
    const forceProbe = vi.fn()
    registerRemoteHealthControls({ forceProbe, stampFlap: vi.fn() })
    forceSoftHealthProbe()
    expect(forceProbe).toHaveBeenCalledTimes(1)
  })

  it('stampRemoteReconnectFlap stamps once per episode', () => {
    const stampFlap = vi.fn()
    registerRemoteHealthControls({ forceProbe: vi.fn(), stampFlap })
    stampRemoteReconnectFlap()
    stampRemoteReconnectFlap()
    expect(stampFlap).toHaveBeenCalledTimes(1)
    expect(wasR1FlapStampedThisEpisode()).toBe(true)
    clearR1FlapEpisode()
    expect(wasR1FlapStampedThisEpisode()).toBe(false)
    stampRemoteReconnectFlap()
    expect(stampFlap).toHaveBeenCalledTimes(2)
  })
})

describe('R1 remote events drop debounce', () => {
  const remoteHost = {
    id: 'h1',
    label: 'scout',
    hostname: 'scout.example',
    port: 443,
    secure: true,
    username: 'u',
    token: 't',
    remember: true,
    lastConnectedAt: null,
  }

  beforeEach(() => {
    vi.useFakeTimers()
    resetSoftResyncBus()
    resetRemoteEventsDropState()
    registerRemoteHealthControls(null)
    useConnectHostStore.setState({
      activeHost: remoteHost,
      recovery: { kind: 'connected' },
      connectionStatus: 'connected',
    })
  })

  afterEach(() => {
    resetRemoteEventsDropState()
    resetSoftResyncBus()
    registerRemoteHealthControls(null)
    useConnectHostStore.setState({
      activeHost: 'local',
      recovery: { kind: 'connected' },
    })
    vi.useRealTimers()
  })

  it('local host is a no-op', () => {
    useConnectHostStore.setState({ activeHost: 'local' })
    noteRemoteEventsClosed()
    expect(isRemoteEventsDropDebounceArmed()).toBe(false)
    expect(hadRemoteEventsPriorClose()).toBe(false)
  })

  it('fire → reconnecting + force probe + flap stamp', () => {
    const forceProbe = vi.fn()
    const stampFlap = vi.fn()
    registerRemoteHealthControls({ forceProbe, stampFlap })

    noteRemoteEventsClosed()
    expect(isRemoteEventsDropDebounceArmed()).toBe(true)
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')

    // Before deadline — still connected
    vi.advanceTimersByTime(REMOTE_EVENTS_DROP_DEBOUNCE_MS - 1)
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
    expect(forceProbe).not.toHaveBeenCalled()

    vi.advanceTimersByTime(1)
    expect(useConnectHostStore.getState().recovery).toEqual({
      kind: 'reconnecting',
      bootPhase: null,
    })
    expect(forceProbe).toHaveBeenCalledTimes(1)
    expect(stampFlap).toHaveBeenCalledTimes(1)
    expect(wasR1FlapStampedThisEpisode()).toBe(true)
  })

  it('cancel on reopen before deadline; events-reopen soft-resync once', () => {
    const forceProbe = vi.fn()
    registerRemoteHealthControls({ forceProbe, stampFlap: vi.fn() })
    const soft = vi.fn()
    subscribeSoftResync(soft)

    noteRemoteEventsClosed()
    expect(isRemoteEventsDropDebounceArmed()).toBe(true)

    noteRemoteEventsOpened()
    expect(isRemoteEventsDropDebounceArmed()).toBe(false)
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
    expect(forceProbe).not.toHaveBeenCalled()

    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(soft).toHaveBeenCalledTimes(1)
    expect(soft).toHaveBeenCalledWith('events-reopen')

    // Debounce must not fire later
    vi.advanceTimersByTime(REMOTE_EVENTS_DROP_DEBOUNCE_MS)
    expect(forceProbe).not.toHaveBeenCalled()
    expect(useConnectHostStore.getState().recovery.kind).toBe('connected')
  })

  it('does not override wedged on debounce fire', () => {
    const forceProbe = vi.fn()
    registerRemoteHealthControls({ forceProbe, stampFlap: vi.fn() })
    noteRemoteEventsClosed()
    // Race: something else latched wedged before fire
    useConnectHostStore.setState({ recovery: { kind: 'wedged' } })
    vi.advanceTimersByTime(REMOTE_EVENTS_DROP_DEBOUNCE_MS)
    expect(useConnectHostStore.getState().recovery.kind).toBe('wedged')
    expect(forceProbe).not.toHaveBeenCalled()
  })

  it('does not start debounce when already reconnecting; still latches prior close', () => {
    useConnectHostStore.setState({
      recovery: { kind: 'reconnecting', bootPhase: null },
    })
    noteRemoteEventsClosed()
    expect(isRemoteEventsDropDebounceArmed()).toBe(false)
    expect(hadRemoteEventsPriorClose()).toBe(true)
  })

  it('coalesces closes across factories (single debounce)', () => {
    const forceProbe = vi.fn()
    registerRemoteHealthControls({ forceProbe, stampFlap: vi.fn() })
    noteRemoteEventsClosed()
    noteRemoteEventsClosed()
    noteRemoteEventsClosed()
    vi.advanceTimersByTime(REMOTE_EVENTS_DROP_DEBOUNCE_MS)
    expect(forceProbe).toHaveBeenCalledTimes(1)
    expect(useConnectHostStore.getState().recovery.kind).toBe('reconnecting')
  })

  it('resetRemoteEventsDropState clears debounce and prior close', () => {
    noteRemoteEventsClosed()
    expect(isRemoteEventsDropDebounceArmed()).toBe(true)
    resetRemoteEventsDropState()
    expect(isRemoteEventsDropDebounceArmed()).toBe(false)
    expect(hadRemoteEventsPriorClose()).toBe(false)
  })

  it('recovery edge setRecovery → soft-resync recovery-connected', () => {
    const soft = vi.fn()
    subscribeSoftResync(soft)
    useConnectHostStore.setState({
      recovery: { kind: 'reconnecting', bootPhase: null },
    })
    useConnectHostStore.getState().setRecovery({ kind: 'connected' })
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(soft).toHaveBeenCalledWith('recovery-connected')
  })

  it('connected→connected setRecovery does not soft-resync', () => {
    const soft = vi.fn()
    subscribeSoftResync(soft)
    useConnectHostStore.getState().setRecovery({ kind: 'connected' })
    vi.advanceTimersByTime(SOFT_RESYNC_COALESCE_MS)
    expect(soft).not.toHaveBeenCalled()
  })
})
