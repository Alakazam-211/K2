// PR-B R4 — single-flight around uploadToRemote.
//
// Two overlapping uploadToRemote(local, dest) share one daemon POST so a
// connection blip / double handler cannot mint name + name (1).
//
// Residual (documented for PR-C): daemonCliPost's own withRemoteRetry may
// still re-POST a single-shot body after the server already wrote bytes if
// the response is lost. A daemon-side client_upload_id would close that.
// These tests assert the client-side flight join, not internal post retries.

import { describe, it, expect, vi, beforeEach } from 'vitest'

const invokeMock = vi.fn()
const daemonCliPostMock = vi.fn()

vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

vi.mock('./daemon-cli', () => ({
  daemonCliPost: (...args: unknown[]) => daemonCliPostMock(...args),
}))

vi.mock('@/stores/connect-host', () => ({
  activeHostKey: () => 'local',
  useConnectHostStore: { getState: () => ({ activeHost: 'local' }) },
}))

import {
  uploadToRemote,
  uploadToRemoteFlightKey,
  uploadFileChunked,
  SINGLE_SHOT_MAX_BYTES,
  __clearUploadToRemoteFlightsForTests,
} from './upload-to-remote'

beforeEach(() => {
  __clearUploadToRemoteFlightsForTests()
  invokeMock.mockReset()
  daemonCliPostMock.mockReset()
})

describe('uploadToRemoteFlightKey', () => {
  it('is host + dest + local path', () => {
    expect(uploadToRemoteFlightKey('/tmp/a', '/dest', 'local')).toBe(
      'local\n/dest\n/tmp/a',
    )
    expect(uploadToRemoteFlightKey('/tmp/a', '/dest', 'h2')).not.toBe(
      uploadToRemoteFlightKey('/tmp/a', '/dest', 'local'),
    )
  })
})

describe('uploadToRemote — single-flight (R4)', () => {
  it('two overlapping same-args calls share one fs/upload-binary POST', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'local_file_size') return 100
      if (cmd === 'read_local_file_base64') return 'YmFzZTY0'
      throw new Error(`unexpected invoke ${cmd}`)
    })

    let release!: (v: { path: string }) => void
    daemonCliPostMock.mockImplementation(
      () =>
        new Promise<{ path: string }>((resolve) => {
          release = resolve
        }),
    )

    const p1 = uploadToRemote('/Users/me/report.pdf', '/srv/inbox')
    const p2 = uploadToRemote('/Users/me/report.pdf', '/srv/inbox')

    await Promise.resolve()
    await Promise.resolve()
    expect(daemonCliPostMock).toHaveBeenCalledTimes(1)
    expect(daemonCliPostMock).toHaveBeenCalledWith('fs/upload-binary', {
      dir: '/srv/inbox',
      filename: 'report.pdf',
      base64: 'YmFzZTY0',
    })

    release({ path: '/srv/inbox/report.pdf' })
    const [r1, r2] = await Promise.all([p1, p2])
    expect(r1).toBe('/srv/inbox/report.pdf')
    expect(r2).toBe('/srv/inbox/report.pdf')
    expect(daemonCliPostMock).toHaveBeenCalledTimes(1)
  })

  it('different destDir does not share a flight', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'local_file_size') return 10
      if (cmd === 'read_local_file_base64') return 'QQ=='
      throw new Error(`unexpected invoke ${cmd}`)
    })
    daemonCliPostMock.mockImplementation(async (_route: string, body: { dir: string; filename: string }) => ({
      path: `${body.dir}/${body.filename}`,
    }))

    const [a, b] = await Promise.all([
      uploadToRemote('/tmp/x.bin', '/srv/a'),
      uploadToRemote('/tmp/x.bin', '/srv/b'),
    ])
    expect(a).toBe('/srv/a/x.bin')
    expect(b).toBe('/srv/b/x.bin')
    expect(daemonCliPostMock).toHaveBeenCalledTimes(2)
  })

  it('clears the flight on settle so a later call POSTs again', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'local_file_size') return 10
      if (cmd === 'read_local_file_base64') return 'QQ=='
      throw new Error(`unexpected invoke ${cmd}`)
    })
    daemonCliPostMock.mockResolvedValue({ path: '/srv/inbox/x.bin' })

    await uploadToRemote('/tmp/x.bin', '/srv/inbox')
    await uploadToRemote('/tmp/x.bin', '/srv/inbox')
    expect(daemonCliPostMock).toHaveBeenCalledTimes(2)
  })

  it('clears the flight on failure so a retry can POST', async () => {
    invokeMock.mockImplementation(async (cmd: string) => {
      if (cmd === 'local_file_size') return 10
      if (cmd === 'read_local_file_base64') return 'QQ=='
      throw new Error(`unexpected invoke ${cmd}`)
    })
    daemonCliPostMock
      .mockRejectedValueOnce(new Error('Load failed'))
      .mockResolvedValueOnce({ path: '/srv/inbox/x.bin' })

    await expect(uploadToRemote('/tmp/x.bin', '/srv/inbox')).rejects.toThrow('Load failed')
    await expect(uploadToRemote('/tmp/x.bin', '/srv/inbox')).resolves.toBe('/srv/inbox/x.bin')
    expect(daemonCliPostMock).toHaveBeenCalledTimes(2)
  })

  it('chunked path: overlapping calls share one upload_id across chunks', async () => {
    const size = SINGLE_SHOT_MAX_BYTES + 1
    invokeMock.mockImplementation(async (cmd: string, args?: { offset?: number; len?: number }) => {
      if (cmd === 'local_file_size') return size
      if (cmd === 'read_local_file_range') {
        // One byte of base64-ish payload is enough for the mock.
        return 'YQ=='
      }
      throw new Error(`unexpected invoke ${cmd} ${JSON.stringify(args)}`)
    })

    const seenIds = new Set<string>()
    let releaseFirst!: () => void
    let firstChunkPending = true
    daemonCliPostMock.mockImplementation(
      async (route: string, body: { upload_id?: string; is_last?: boolean; offset?: number }) => {
        expect(route).toBe('fs/upload-chunk')
        if (body.upload_id) seenIds.add(body.upload_id)
        if (firstChunkPending && body.offset === 0) {
          firstChunkPending = false
          await new Promise<void>((resolve) => {
            releaseFirst = resolve
          })
        }
        if (body.is_last) return { path: '/srv/big.bin', done: true }
        return { received: (body.offset ?? 0) + 1, done: false }
      },
    )

    const p1 = uploadToRemote('/tmp/big.bin', '/srv')
    const p2 = uploadToRemote('/tmp/big.bin', '/srv')

    // Wait until the first chunk is blocked.
    for (let i = 0; i < 20 && !releaseFirst; i++) await Promise.resolve()
    expect(releaseFirst).toBeTypeOf('function')

    releaseFirst()
    const [r1, r2] = await Promise.all([p1, p2])
    expect(r1).toBe('/srv/big.bin')
    expect(r2).toBe('/srv/big.bin')
    // One flight → one upload_id for the whole chunk stream.
    expect(seenIds.size).toBe(1)
  })
})

describe('uploadFileChunked — stable uploadId option', () => {
  it('reuses the provided uploadId on every chunk', async () => {
    const posts: Array<{ upload_id: string; offset: number }> = []
    const deps = {
      daemonCliPost: async <T = unknown>(_route: string, body?: unknown): Promise<T> => {
        const b = body as { upload_id: string; offset: number; is_last: boolean }
        posts.push({ upload_id: b.upload_id, offset: b.offset })
        if (b.is_last) return { path: '/d/f', done: true } as T
        return { done: false } as T
      },
      readLocalFileRange: async () => 'YQ==',
    }
    const path = await uploadFileChunked(deps, '/local/f', 3, {
      dir: '/d',
      filename: 'f',
      uploadId: 'stable-drop-id',
      chunkBytes: 2,
    })
    expect(path).toBe('/d/f')
    expect(posts.every((p) => p.upload_id === 'stable-drop-id')).toBe(true)
    expect(posts.map((p) => p.offset)).toEqual([0, 2])
  })
})
