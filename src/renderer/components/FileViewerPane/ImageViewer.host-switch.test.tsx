// @vitest-environment jsdom

import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest'
import { render, cleanup, waitFor } from '@testing-library/react'

const daemonCliGet = vi.fn()
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return {
    ...actual,
    daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
    daemonCliPost: vi.fn(async () => ({})),
  }
})
vi.mock('@tauri-apps/api/core', () => ({
  invoke: vi.fn(async () => null),
}))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))

import { ImageViewer } from './ImageViewer'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
  type ConnectHost,
} from '@/stores/connect-host'

const SCREENSHOT =
  '/var/folders/zz/abc/T/NSIRD_screencaptureui_xxx/Screenshot.png'

function makeRemoteHost(): ConnectHost {
  return {
    id: 'dtl-1',
    label: 'DTL',
    hostname: 'anna.k2.dev',
    username: 'rosson',
    port: 443,
    secure: true,
    token: 'tok',
    remember: false,
    lastConnectedAt: null,
  }
}

describe('ImageViewer host-switch leftover', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    __resetConnectHostStoreForTests()
  })

  afterEach(() => {
    cleanup()
    __resetConnectHostStoreForTests()
  })

  it('remount after selectHost(remote) issues zero daemonCliGet for Mac tmp PNG', async () => {
    daemonCliGet.mockResolvedValue({ base64: 'aGVsbG8=' })
    const first = render(<ImageViewer filePath={SCREENSHOT} />)
    await waitFor(() => expect(daemonCliGet).toHaveBeenCalled())
    daemonCliGet.mockClear()

    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    first.unmount()
    render(<ImageViewer filePath={SCREENSHOT} />)
    await waitFor(() => {
      expect(document.body.textContent).toMatch(/Not available on this server/i)
    })
    const leftover = daemonCliGet.mock.calls.filter((args) =>
      args.some((a) => JSON.stringify(a).includes(SCREENSHOT) || JSON.stringify(a).includes('/var/folders')),
    )
    expect(leftover).toHaveLength(0)
  })

  it('in-flight local read is dropped after host switch (no paint of A on B)', async () => {
    let resolveGet: (value: unknown) => void = () => {}
    daemonCliGet.mockImplementation(
      () =>
        new Promise((resolve) => {
          resolveGet = resolve
        }),
    )
    render(<ImageViewer filePath={SCREENSHOT} />)
    await waitFor(() => expect(daemonCliGet).toHaveBeenCalledTimes(1))

    const host = makeRemoteHost()
    useConnectHostStore.getState().addHost(host)
    useConnectHostStore.getState().selectHost(host)
    resolveGet({ base64: 'aGVsbG8=' })

    await waitFor(() => {
      expect(document.querySelector('img')).toBeNull()
    })
  })
})
