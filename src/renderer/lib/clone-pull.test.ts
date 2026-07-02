// Unit tests for the "Clone to this computer" orchestration (clone-pull.ts).
//
// We exercise `cloneWorkspaceToThisComputer` with a fully-mocked dep bag and
// assert the STEP SEQUENCING — the load-bearing property (the remote calls
// ride the active-host helpers, the unpack MUST ride the local-daemon
// helper, and cleanup MUST fire on every exit):
//   1. happy path: pack → poll → local folder pick → read-range loop into
//      clone-tmp chunks → remote pack-cleanup → LOCAL unpack, in order.
//   2. pack job fails → surfaces, NO download / unpack.
//   3. folder-pick cancel → server bundle reclaimed, NO download / unpack.
//   4. transfer-overlay cancel mid-download → part aborted + server bundle
//      reclaimed + CloneCancelledError, NO unpack.
//   5. LOCAL unpack failure → temp bundle deleted via local fs/delete, and
//      the error names the unpack stage.
// Hooks (onStage/onBundled/onDone/onError) are asserted alongside.

import { describe, it, expect, vi } from 'vitest'

import {
  cloneWorkspaceToThisComputer,
  PULL_DOWNLOAD_CHUNK_BYTES,
  type ClonePullDeps,
  type ClonePackStatus,
} from './clone-pull'
import { CloneCancelledError, type CloneUnpackResult } from './clone-to'

const PACKED: ClonePackStatus = {
  job_id: 'job-1',
  phase: 'done',
  bundle_path: '/home/rosson/.k2/clone-tmp/myws-20260701-000000.tar.gz',
  size_bytes: 10,
  entry_count: 42,
  scrubbed_secret_count: 3,
}

const UNPACK: CloneUnpackResult = {
  project: { id: 'local-proj', name: 'myws', path: '/Users/rosson/work/myws' },
  dest_path: '/Users/rosson/work/myws',
}

const LOCAL_BUNDLE = '/Users/rosson/.k2/clone-tmp/myws-20260701-000000.tar.gz'

/** Build a dep bag whose calls are recorded into `order` (a flat call log)
 *  so we can assert sequencing. Individual steps are overridable. The
 *  default remote serves the bundle in TWO read-range slices. */
function makeDeps(
  order: string[],
  overrides: Partial<ClonePullDeps> = {},
): { deps: ClonePullDeps; spies: Record<string, ReturnType<typeof vi.fn>> } {
  const daemonCliPost = vi.fn(async (route: string, _body?: unknown) => {
    order.push(`post:${route}`)
    if (route === 'clone/pack') return { job_id: 'job-1' } as unknown
    if (route === 'clone/pack-cleanup') return { success: true } as unknown
    return {} as unknown
  }) as ClonePullDeps['daemonCliPost'] & ReturnType<typeof vi.fn>
  const daemonCliGet = vi.fn(
    async (route: string, params?: Record<string, unknown>) => {
      order.push(`get:${route}`)
      if (route === 'clone/pack-status') return PACKED as unknown
      if (route === 'fs/read-range') {
        // Two 5-byte slices of a 10-byte bundle.
        const offset = Number(params?.offset ?? 0)
        return {
          base64: offset === 0 ? 'AAAAA' : 'BBBBB',
          len: 5,
          size: 10,
          eof: offset >= 5,
        } as unknown
      }
      return {} as unknown
    },
  ) as ClonePullDeps['daemonCliGet'] & ReturnType<typeof vi.fn>
  const localDaemonCliPost = vi.fn(async (route: string, _body?: unknown) => {
    order.push(`local-post:${route}`)
    if (route === 'clone/unpack') return UNPACK as unknown
    return {} as unknown
  }) as ClonePullDeps['localDaemonCliPost'] & ReturnType<typeof vi.fn>
  const pickLocalFolder = vi.fn(async () => {
    order.push('pick-local-folder')
    return '/Users/rosson/work'
  })
  const localDownloadChunk = vi.fn(
    async (_id: string, _name: string, _offset: number, _b64: string, isLast: boolean) => {
      order.push('download-chunk')
      return isLast ? LOCAL_BUNDLE : null
    },
  )
  const localDownloadAbort = vi.fn(async (_id: string) => {
    order.push('download-abort')
  })
  const progress = {
    begin: vi.fn((_kind: 'download', _label: string) => {
      order.push('progress-begin')
      return 'tid-1'
    }),
    update: vi.fn(),
    end: vi.fn(() => order.push('progress-end')),
    isCancelRequested: vi.fn(() => false),
  }
  const sleep = vi.fn(async () => undefined)

  const deps: ClonePullDeps = {
    daemonCliPost,
    daemonCliGet,
    localDaemonCliPost,
    pickLocalFolder,
    localDownloadChunk,
    localDownloadAbort,
    progress,
    sleep,
    ...overrides,
  }
  return {
    deps,
    spies: {
      daemonCliPost,
      daemonCliGet,
      localDaemonCliPost,
      pickLocalFolder,
      localDownloadChunk,
      localDownloadAbort,
      progressBegin: progress.begin,
      progressEnd: progress.end,
      isCancelRequested: progress.isCancelRequested,
    },
  }
}

describe('cloneWorkspaceToThisComputer', () => {
  it('sequences pack → poll → pick → download → remote cleanup → LOCAL unpack', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    const stages: string[] = []
    const onBundled = vi.fn()
    const onDone = vi.fn()

    const result = await cloneWorkspaceToThisComputer(
      '/srv/work/myws',
      'myws',
      deps,
      { onStage: (s) => stages.push(s), onBundled, onDone },
      false, // carrySecrets
      false, // includeAllHistory
    )

    expect(result).toEqual(UNPACK)
    expect(order).toEqual([
      'post:clone/pack',
      'get:clone/pack-status',
      'pick-local-folder',
      'progress-begin',
      'get:fs/read-range',
      'download-chunk',
      'get:fs/read-range',
      'download-chunk',
      'progress-end',
      'post:clone/pack-cleanup',
      'local-post:clone/unpack',
    ])
    expect(stages).toEqual([
      'packing',
      'choosing-folder',
      'downloading',
      'unpacking',
      'done',
    ])

    // Pack body carries the options (live_only is the INVERSE of history).
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/pack', {
      project_path: '/srv/work/myws',
      carry_secrets: false,
      live_only: true,
    })
    // Download slices are ordered and sized by the shared chunk constant.
    expect(spies.daemonCliGet).toHaveBeenCalledWith('fs/read-range', {
      path: PACKED.bundle_path,
      offset: 0,
      len: PULL_DOWNLOAD_CHUNK_BYTES,
    })
    expect(spies.localDownloadChunk).toHaveBeenNthCalledWith(
      1,
      expect.stringMatching(/^clone-pull-/),
      'myws-20260701-000000.tar.gz',
      0,
      'AAAAA',
      false,
    )
    expect(spies.localDownloadChunk).toHaveBeenNthCalledWith(
      2,
      expect.stringMatching(/^clone-pull-/),
      'myws-20260701-000000.tar.gz',
      5,
      'BBBBB',
      true,
    )
    // The unpack rides the LOCAL daemon helper with the downloaded bundle
    // + the picked parent — never the active-host helper.
    expect(spies.localDaemonCliPost).toHaveBeenCalledWith('clone/unpack', {
      bundle_path: LOCAL_BUNDLE,
      dest_parent: '/Users/rosson/work',
    })
    expect(onBundled).toHaveBeenCalledWith({
      entry_count: 42,
      scrubbed_secret_count: 3,
      size_bytes: 10,
    })
    expect(onDone).toHaveBeenCalledWith(UNPACK)
  })

  it('surfaces a failed pack job and never downloads or unpacks', async () => {
    const order: string[] = []
    const failed: ClonePackStatus = {
      job_id: 'job-1',
      phase: 'failed',
      error: 'bundle is 999 bytes — over the 10 GiB transfer ceiling',
    }
    const { deps, spies } = makeDeps(order, {
      daemonCliGet: vi.fn(async (route: string) => {
        order.push(`get:${route}`)
        return failed as unknown
      }) as ClonePullDeps['daemonCliGet'],
    })
    const onError = vi.fn()

    await expect(
      cloneWorkspaceToThisComputer('/srv/work/myws', 'myws', deps, { onError }),
    ).rejects.toThrow(/Packing failed on the server: .*transfer ceiling/)
    expect(onError).toHaveBeenCalledWith(
      expect.stringContaining('Packing failed on the server'),
    )
    expect(spies.pickLocalFolder).not.toHaveBeenCalled()
    expect(spies.localDownloadChunk).not.toHaveBeenCalled()
    expect(spies.localDaemonCliPost).not.toHaveBeenCalled()
  })

  it('reclaims the server bundle on folder-pick cancel and aborts', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order, {
      pickLocalFolder: vi.fn(async () => {
        order.push('pick-local-folder')
        return null
      }),
    })

    await expect(
      cloneWorkspaceToThisComputer('/srv/work/myws', 'myws', deps),
    ).rejects.toThrow(CloneCancelledError)
    expect(order).toContain('post:clone/pack-cleanup')
    expect(spies.localDownloadChunk).not.toHaveBeenCalled()
    expect(spies.localDaemonCliPost).not.toHaveBeenCalled()
  })

  it('overlay cancel mid-download aborts the part, reclaims the server bundle, and never unpacks', async () => {
    const order: string[] = []
    // Cancel is requested from the SECOND loop iteration onward.
    let polls = 0
    const { deps, spies } = makeDeps(order)
    ;(deps.progress.isCancelRequested as ReturnType<typeof vi.fn>).mockImplementation(
      () => {
        polls += 1
        return polls > 1
      },
    )

    await expect(
      cloneWorkspaceToThisComputer('/srv/work/myws', 'myws', deps),
    ).rejects.toThrow(CloneCancelledError)
    expect(spies.localDownloadAbort).toHaveBeenCalledTimes(1)
    expect(order).toContain('post:clone/pack-cleanup')
    expect(order).toContain('progress-end')
    expect(spies.localDaemonCliPost).not.toHaveBeenCalled()
  })

  it('deletes the downloaded temp bundle when the LOCAL unpack fails, naming the stage', async () => {
    const order: string[] = []
    const localDaemonCliPost = vi.fn(async (route: string, _body?: unknown) => {
      order.push(`local-post:${route}`)
      if (route === 'clone/unpack') throw new Error('register project: db locked')
      return {} as unknown
    }) as ClonePullDeps['localDaemonCliPost'] & ReturnType<typeof vi.fn>
    const { deps } = makeDeps(order, { localDaemonCliPost })
    const onError = vi.fn()

    await expect(
      cloneWorkspaceToThisComputer('/srv/work/myws', 'myws', deps, { onError }),
    ).rejects.toThrow(/Unpacking on this computer failed: register project/)
    expect(localDaemonCliPost).toHaveBeenCalledWith('fs/delete', {
      paths: [LOCAL_BUNDLE],
      permanent: true,
    })
    expect(onError).toHaveBeenCalledWith(
      expect.stringContaining('Unpacking on this computer failed'),
    )
  })
})
