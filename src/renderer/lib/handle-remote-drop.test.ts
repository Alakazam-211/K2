// K2 Connect remote-files Phase 3 — drop router tests.
//
// `resolveDropDestination` is pure, so its tests are plain input→output.
// `executeRemoteDrop` is exercised through a mocked `uploadToRemote` +
// an injected picker (no zustand) to verify the destination routing and
// the terminal payload hand-off. Failures throw loudly — no swallowed
// catches, no skip-if-missing fallbacks.
//
// PR-B R3: concurrent same paths+dest join one upload flight.

import { describe, it, expect, vi, beforeEach } from 'vitest'

import {
  resolveDropDestination,
  executeRemoteDrop,
  remoteDropFlightKey,
  __clearRemoteDropFlightsForTests,
} from './handle-remote-drop'

// Mock the upload substrate (the IO half) + the toast store. The router
// imports `uploadToRemote` from upload-to-remote and the toast store; we
// replace both so the test stays in-memory.
const uploadMock = vi.fn<(local: string, dir: string) => Promise<string>>()
vi.mock('./upload-to-remote', () => ({
  uploadToRemote: (local: string, dir: string) => uploadMock(local, dir),
}))
vi.mock('@/stores/toast', () => ({
  useToastStore: { getState: () => ({ addToast: () => {} }) },
}))
vi.mock('@/stores/connect-host', () => ({
  activeHostKey: () => 'local',
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))

beforeEach(() => {
  __clearRemoteDropFlightsForTests()
  uploadMock.mockReset()
  // Default: echo a remote path inside the destination dir.
  uploadMock.mockImplementation(async (local: string, dir: string) => {
    const name = local.split('/').pop() ?? local
    return `${dir}/${name}`
  })
})

// ── resolveDropDestination (pure) ──────────────────────────────────────

describe('resolveDropDestination', () => {
  it('terminal → <workspace>/.k2/downloads, inject:true', () => {
    const dest = resolveDropDestination(
      { kind: 'terminal' },
      { workspacePath: '/home/u/ws' },
    )
    expect(dest).toEqual({
      destDir: '/home/u/ws/.k2/downloads',
      inject: true,
    })
  })

  it('terminal strips a trailing slash on the workspace path', () => {
    const dest = resolveDropDestination(
      { kind: 'terminal' },
      { workspacePath: '/home/u/ws/' },
    )
    expect(dest.destDir).toBe('/home/u/ws/.k2/downloads')
  })

  it('terminal with NO workspace → destDir:null (executor picks), inject:true', () => {
    const dest = resolveDropDestination({ kind: 'terminal' }, {})
    expect(dest).toEqual({ destDir: null, inject: true })
  })

  it('folder → that folder path, inject:false', () => {
    const dest = resolveDropDestination(
      { kind: 'folder', path: '/srv/data/inbox' },
      {},
    )
    expect(dest).toEqual({ destDir: '/srv/data/inbox', inject: false })
  })

  it('miss → destDir:null (resolved by picker), inject:false', () => {
    const dest = resolveDropDestination({ kind: 'miss' }, {})
    expect(dest).toEqual({ destDir: null, inject: false })
  })
})

describe('remoteDropFlightKey', () => {
  it('sorts paths so multi-file order does not split the key', () => {
    expect(remoteDropFlightKey(['/b', '/a'], '/dest', 'local')).toBe(
      remoteDropFlightKey(['/a', '/b'], '/dest', 'local'),
    )
  })

  it('differs by destDir and host', () => {
    const base = remoteDropFlightKey(['/a'], '/d1', 'local')
    expect(remoteDropFlightKey(['/a'], '/d2', 'local')).not.toBe(base)
    expect(remoteDropFlightKey(['/a'], '/d1', 'host-2')).not.toBe(base)
  })
})

// ── executeRemoteDrop (composed with mocked upload + injected picker) ───

describe('executeRemoteDrop', () => {
  it('terminal: uploads to downloads dir and returns the built payload from REMOTE paths', async () => {
    const buildPayload = vi.fn((paths: string[]) => `PAYLOAD(${paths.join(',')})`)
    const payload = await executeRemoteDrop(
      ['/local/a.png', '/local/b.txt'],
      { kind: 'terminal' },
      { workspacePath: '/ws' },
      buildPayload,
    )
    // Both files uploaded into the downloads dir, in order.
    expect(uploadMock).toHaveBeenNthCalledWith(1, '/local/a.png', '/ws/.k2/downloads')
    expect(uploadMock).toHaveBeenNthCalledWith(2, '/local/b.txt', '/ws/.k2/downloads')
    // The payload is built from the REMOTE paths, not the local ones.
    expect(buildPayload).toHaveBeenCalledWith([
      '/ws/.k2/downloads/a.png',
      '/ws/.k2/downloads/b.txt',
    ])
    expect(payload).toBe('PAYLOAD(/ws/.k2/downloads/a.png,/ws/.k2/downloads/b.txt)')
  })

  it('folder: uploads into the folder and returns null (no injection)', async () => {
    const buildPayload = vi.fn()
    const payload = await executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
      buildPayload,
    )
    expect(uploadMock).toHaveBeenCalledWith('/local/report.pdf', '/srv/inbox')
    expect(buildPayload).not.toHaveBeenCalled()
    expect(payload).toBeNull()
  })

  it('miss: opens the picker, uploads into the chosen dir, returns null', async () => {
    const openPicker = vi.fn(async () => '/chosen/dir')
    const payload = await executeRemoteDrop(
      ['/local/x.bin'],
      { kind: 'miss' },
      { openPicker },
    )
    expect(openPicker).toHaveBeenCalledOnce()
    expect(uploadMock).toHaveBeenCalledWith('/local/x.bin', '/chosen/dir')
    expect(payload).toBeNull()
  })

  it('miss: a cancelled picker uploads nothing and returns null', async () => {
    const openPicker = vi.fn(async () => null)
    const payload = await executeRemoteDrop(
      ['/local/x.bin'],
      { kind: 'miss' },
      { openPicker },
    )
    expect(openPicker).toHaveBeenCalledOnce()
    expect(uploadMock).not.toHaveBeenCalled()
    expect(payload).toBeNull()
  })

  it('terminal with no workspace: falls back to the picker, then injects', async () => {
    const openPicker = vi.fn(async () => '/picked')
    const buildPayload = vi.fn((paths: string[]) => paths.join('|'))
    const payload = await executeRemoteDrop(
      ['/local/y.txt'],
      { kind: 'terminal' },
      { openPicker },
      buildPayload,
    )
    expect(openPicker).toHaveBeenCalledOnce()
    expect(uploadMock).toHaveBeenCalledWith('/local/y.txt', '/picked')
    expect(payload).toBe('/picked/y.txt')
  })

  it('empty local paths → returns null without uploading', async () => {
    const payload = await executeRemoteDrop([], { kind: 'terminal' }, { workspacePath: '/ws' })
    expect(uploadMock).not.toHaveBeenCalled()
    expect(payload).toBeNull()
  })

  it('an upload error returns null (does not throw) and stops', async () => {
    uploadMock.mockRejectedValueOnce(new Error('boom'))
    const buildPayload = vi.fn()
    const payload = await executeRemoteDrop(
      ['/local/a', '/local/b'],
      { kind: 'terminal' },
      { workspacePath: '/ws' },
      buildPayload,
    )
    expect(payload).toBeNull()
    expect(buildPayload).not.toHaveBeenCalled()
    // Stopped after the first (failed) upload — did not attempt the second.
    expect(uploadMock).toHaveBeenCalledTimes(1)
  })

  // ── R3: in-flight dedupe ───────────────────────────────────────────

  it('R3: concurrent same paths+dest join one upload (one uploadToRemote)', async () => {
    let release!: (path: string) => void
    uploadMock.mockImplementation(
      () =>
        new Promise<string>((resolve) => {
          release = resolve
        }),
    )

    const p1 = executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
    )
    const p2 = executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
    )

    // Let both hit the inflight map before the upload settles.
    await Promise.resolve()
    await Promise.resolve()
    expect(uploadMock).toHaveBeenCalledTimes(1)

    release('/srv/inbox/report.pdf')
    const [r1, r2] = await Promise.all([p1, p2])
    expect(r1).toBeNull()
    expect(r2).toBeNull()
    expect(uploadMock).toHaveBeenCalledTimes(1)
  })

  it('R3: concurrent multi-file drops with different path order still share one flight', async () => {
    let release!: (path: string) => void
    let calls = 0
    uploadMock.mockImplementation(async (local: string, dir: string) => {
      calls++
      if (calls === 1) {
        return new Promise<string>((resolve) => {
          release = resolve
        })
      }
      const name = local.split('/').pop() ?? local
      return `${dir}/${name}`
    })

    const p1 = executeRemoteDrop(
      ['/local/b.txt', '/local/a.txt'],
      { kind: 'folder', path: '/srv' },
      {},
    )
    const p2 = executeRemoteDrop(
      ['/local/a.txt', '/local/b.txt'],
      { kind: 'folder', path: '/srv' },
      {},
    )

    await Promise.resolve()
    await Promise.resolve()
    // First flight is still stuck on the first file; second call joined —
    // no second batch started.
    expect(uploadMock).toHaveBeenCalledTimes(1)

    release('/srv/b.txt')
    await Promise.all([p1, p2])
    // One sequential batch: a and b (order of the first caller).
    expect(uploadMock).toHaveBeenCalledTimes(2)
  })

  it('R3: different destDir does not join', async () => {
    await Promise.all([
      executeRemoteDrop(['/local/a.pdf'], { kind: 'folder', path: '/srv/a' }, {}),
      executeRemoteDrop(['/local/a.pdf'], { kind: 'folder', path: '/srv/b' }, {}),
    ])
    expect(uploadMock).toHaveBeenCalledTimes(2)
  })

  it('R3: key clears on settle — a later drop uploads again', async () => {
    await executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
    )
    expect(uploadMock).toHaveBeenCalledTimes(1)
    await executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
    )
    expect(uploadMock).toHaveBeenCalledTimes(2)
  })

  it('R3: key clears on failure so a retry can upload', async () => {
    uploadMock.mockRejectedValueOnce(new Error('network'))
    const failed = await executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
    )
    expect(failed).toBeNull()

    uploadMock.mockImplementation(async (local: string, dir: string) => {
      const name = local.split('/').pop() ?? local
      return `${dir}/${name}`
    })
    await executeRemoteDrop(
      ['/local/report.pdf'],
      { kind: 'folder', path: '/srv/inbox' },
      {},
    )
    expect(uploadMock).toHaveBeenCalledTimes(2)
  })
})
