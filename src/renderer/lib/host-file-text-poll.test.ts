import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))

import { startHostFileTextPoll, shouldSkipHostFilePollTick } from './host-file-text-poll'
import { useWindowFocusStore } from '@/stores/window-focus'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'
import { HostSwitchedError } from './daemon-cli'

function makeRemoteHost(): ConnectHost {
  return {
    id: 'scout-1',
    label: 'Scout',
    hostname: 'scout.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

async function flush(): Promise<void> {
  await Promise.resolve()
  await Promise.resolve()
}

describe('host-file-text-poll', () => {
  beforeEach(() => {
    vi.useFakeTimers()
    __resetConnectHostStoreForTests()
    useWindowFocusStore.setState({ isFocused: true })
  })

  afterEach(() => {
    vi.useRealTimers()
    useWindowFocusStore.setState({ isFocused: true })
    __resetConnectHostStoreForTests()
  })

  it('blurred window skips the GET; focus-gain fires one read', async () => {
    useWindowFocusStore.setState({ isFocused: false })
    expect(shouldSkipHostFilePollTick()).toBe(true)
    const read = vi.fn(async () => 'html')
    const apply = vi.fn()
    const stop = startHostFileTextPoll({
      filePath: '/ws/dash.html',
      intervalMs: 2000,
      immediate: true,
      read,
      apply,
    })

    await flush()
    expect(read).not.toHaveBeenCalled()

    useWindowFocusStore.setState({ isFocused: true })
    await flush()
    expect(read).toHaveBeenCalledTimes(1)
    expect(apply).toHaveBeenCalledWith('html')

    await vi.advanceTimersByTimeAsync(2000)
    await flush()
    expect(read).toHaveBeenCalledTimes(2)
    stop()
  })

  it('skips while remote recovery.kind !== connected', async () => {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    useConnectHostStore.getState().setRecovery({ kind: 'reconnecting', bootPhase: null })
    expect(shouldSkipHostFilePollTick()).toBe(true)

    const read = vi.fn(async () => 'x')
    const stop = startHostFileTextPoll({
      filePath: '/ws/dash.html',
      intervalMs: 2000,
      immediate: true,
      read,
      apply: vi.fn(),
    })
    await flush()
    expect(read).not.toHaveBeenCalled()
    stop()
  })

  it('CORS / connection-level error stops the interval', async () => {
    const read = vi
      .fn()
      .mockRejectedValueOnce(
        new TypeError(
          'Origin tauri://localhost is not allowed by Access-Control-Allow-Origin. Status code: 404',
        ),
      )
    const onError = vi.fn()
    const stop = startHostFileTextPoll({
      filePath: '/ws/dash.html',
      intervalMs: 2000,
      immediate: true,
      read,
      apply: vi.fn(),
      onError,
    })
    await flush()
    expect(onError).toHaveBeenCalledTimes(1)

    await vi.advanceTimersByTimeAsync(6000)
    await flush()
    expect(read).toHaveBeenCalledTimes(1)
    stop()
  })

  it('host-switch error drops the result and stops polling', async () => {
    const read = vi.fn(async () => {
      throw new HostSwitchedError('local', 'r1:scout.k2.dev:443')
    })
    const apply = vi.fn()
    const onError = vi.fn()
    const stop = startHostFileTextPoll({
      filePath: '/ws/dash.html',
      intervalMs: 2000,
      immediate: true,
      read,
      apply,
      onError,
    })
    await flush()
    expect(apply).not.toHaveBeenCalled()
    expect(onError).not.toHaveBeenCalled()
    await vi.advanceTimersByTimeAsync(4000)
    await flush()
    expect(read).toHaveBeenCalledTimes(1)
    stop()
  })

  it('remote + /var/folders does not call read', async () => {
    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    const read = vi.fn(async () => 'nope')
    const stop = startHostFileTextPoll({
      filePath: '/var/folders/zz/abc/T/Screenshot.png',
      intervalMs: 2000,
      immediate: true,
      read,
      apply: vi.fn(),
      onError: vi.fn(),
    })
    await flush()
    expect(read).not.toHaveBeenCalled()
    stop()
  })
})
