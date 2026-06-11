// Unit tests for the "Clone to" orchestration (clone-to.ts).
//
// We exercise `cloneWorkspaceTo` with a fully-mocked dep bag and assert the
// STEP SEQUENCING — the load-bearing property (each daemon call targets the
// then-active host, so order is correctness):
//   1. happy path: bundle → read → switch → wait → picker → fs/info →
//      upload → unpack, in order, with the right args.
//   2. folder-picker cancel → abort: NO upload, NO unpack.
//   3. sign-in cancel (waitForHostConnected rejects) → abort: bundle built
//      + read, but NO picker / upload / unpack.
//   4. an error in a middle step (upload) surfaces + stops (no unpack).
// Hooks (onStage/onBundled/onDone/onError) are asserted too.

import { describe, it, expect, vi, beforeEach } from 'vitest'

import {
  cloneWorkspaceTo,
  CloneCancelledError,
  basename,
  type CloneDeps,
  type CloneBundleResult,
  type CloneUnpackResult,
} from './clone-to'
import type { ConnectHost } from '@/stores/connect-host'

const DEST: ConnectHost = {
  id: 'host-1',
  label: 'Hetzner box',
  hostname: 'rosson.k2.dev',
  username: 'rosson',
  port: 443,
  secure: true,
  token: 'tok',
  remember: true,
  lastConnectedAt: null,
}

const BUNDLE: CloneBundleResult = {
  bundle_path: '/tmp/k2so-clone/myworkspace.tar.gz',
  manifest_summary: { entry_count: 42, scrubbed_secret_count: 3, size_bytes: 1234 },
}

const UNPACK: CloneUnpackResult = {
  project: { id: 'remote-proj', name: 'myworkspace', path: '/home/rosson/work/myworkspace' },
  dest_path: '/home/rosson/work/myworkspace',
}

/** Build a dep bag whose calls are recorded into `order` (a flat call log)
 *  so we can assert sequencing. Individual steps are overridable. */
function makeDeps(
  order: string[],
  overrides: Partial<CloneDeps> = {},
): { deps: CloneDeps; spies: Record<string, ReturnType<typeof vi.fn>> } {
  const daemonCliPost = vi.fn(async (route: string, _body?: unknown) => {
    order.push(`post:${route}`)
    if (route === 'clone/bundle') return BUNDLE as unknown
    if (route === 'fs/upload-binary') return { path: '/home/rosson/.k2so/clone-tmp/myworkspace.tar.gz' } as unknown
    if (route === 'clone/unpack') return UNPACK as unknown
    return {} as unknown
  }) as CloneDeps['daemonCliPost'] & ReturnType<typeof vi.fn>
  const daemonCliGet = vi.fn(async (route: string) => {
    order.push(`get:${route}`)
    if (route === 'fs/info') return { home: '/home/rosson', separator: '/', os: 'linux' } as unknown
    return {} as unknown
  }) as CloneDeps['daemonCliGet'] & ReturnType<typeof vi.fn>
  const readLocalFileBase64 = vi.fn(async (_path: string) => {
    order.push('read-base64')
    return 'BASE64BYTES'
  })
  const pickHost = vi.fn((_host: ConnectHost) => {
    order.push('pickHost')
  })
  const waitForHostConnected = vi.fn(async (_host: ConnectHost) => {
    order.push('waitForHostConnected')
  })
  const pickRemoteFolder = vi.fn(async () => {
    order.push('pickRemoteFolder')
    return '/home/rosson/work'
  })

  const deps: CloneDeps = {
    daemonCliPost,
    daemonCliGet,
    readLocalFileBase64,
    pickHost,
    waitForHostConnected,
    pickRemoteFolder,
    ...overrides,
  }
  return {
    deps,
    spies: {
      daemonCliPost,
      daemonCliGet,
      readLocalFileBase64,
      pickHost,
      waitForHostConnected,
      pickRemoteFolder,
    } as Record<string, ReturnType<typeof vi.fn>>,
  }
}

describe('basename', () => {
  it('takes the last segment for unix and windows paths', () => {
    expect(basename('/tmp/a/b.tar.gz')).toBe('b.tar.gz')
    expect(basename('C:\\tmp\\a\\b.tar.gz')).toBe('b.tar.gz')
    expect(basename('bare.tar.gz')).toBe('bare.tar.gz')
  })
})

describe('cloneWorkspaceTo — happy path', () => {
  let order: string[]
  let deps: CloneDeps
  let spies: Record<string, ReturnType<typeof vi.fn>>

  beforeEach(() => {
    order = []
    ;({ deps, spies } = makeDeps(order))
  })

  it('runs bundle → read → switch → wait → picker → fs/info → upload → unpack in order', async () => {
    const onStage = vi.fn()
    const onBundled = vi.fn()
    const onDone = vi.fn()
    const onError = vi.fn()

    const result = await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, {
      onStage,
      onBundled,
      onDone,
      onError,
    })

    expect(result).toEqual(UNPACK)
    expect(order).toEqual([
      'post:clone/bundle',
      'read-base64',
      'pickHost',
      'waitForHostConnected',
      'pickRemoteFolder',
      'get:fs/info',
      'post:fs/upload-binary',
      'post:clone/unpack',
    ])

    // Right args at each step. carry_secrets + all-history both default to
    // include (live_only defaults false).
    expect(spies.daemonCliPost).toHaveBeenNthCalledWith(1, 'clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: true,
      live_only: false,
    })
    expect(spies.readLocalFileBase64).toHaveBeenCalledWith(BUNDLE.bundle_path)
    expect(spies.pickHost).toHaveBeenCalledWith(DEST)
    expect(spies.waitForHostConnected).toHaveBeenCalledWith(DEST)
    // Upload targets the host temp dir derived from fs/info home, carries the
    // bundle basename + the base64 read locally.
    expect(spies.daemonCliPost).toHaveBeenCalledWith('fs/upload-binary', {
      dir: '/home/rosson/.k2so/clone-tmp',
      filename: 'myworkspace.tar.gz',
      base64: 'BASE64BYTES',
    })
    // Unpack uses the UPLOADED remote path + the chosen parent.
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/unpack', {
      bundle_path: '/home/rosson/.k2so/clone-tmp/myworkspace.tar.gz',
      dest_parent: '/home/rosson/work',
    })

    // Hooks.
    expect(onBundled).toHaveBeenCalledWith(BUNDLE.manifest_summary)
    expect(onDone).toHaveBeenCalledWith(UNPACK)
    expect(onError).not.toHaveBeenCalled()
    expect(onStage.mock.calls.map((c) => c[0])).toEqual([
      'bundling',
      'connecting',
      'choosing-folder',
      'uploading',
      'unpacking',
      'done',
    ])
  })

  it('builds the bundle while local is active (bundle + read precede pickHost)', async () => {
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps)
    const bundleIdx = order.indexOf('post:clone/bundle')
    const readIdx = order.indexOf('read-base64')
    const switchIdx = order.indexOf('pickHost')
    expect(bundleIdx).toBeGreaterThanOrEqual(0)
    expect(readIdx).toBeGreaterThan(bundleIdx)
    expect(switchIdx).toBeGreaterThan(readIdx)
  })

  it('falls back to dest_parent for the upload dir when fs/info fails', async () => {
    order = []
    ;({ deps, spies } = makeDeps(order, {
      daemonCliGet: (vi.fn(async (route: string) => {
        order.push(`get:${route}`)
        throw new Error('fs/info not supported')
      }) as unknown) as CloneDeps['daemonCliGet'],
    }))
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('fs/upload-binary', {
      dir: '/home/rosson/work',
      filename: 'myworkspace.tar.gz',
      base64: 'BASE64BYTES',
    })
  })
})

describe('cloneWorkspaceTo — carry_secrets toggle', () => {
  it('passes carry_secrets: true to clone/bundle by default (include)', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: true,
      live_only: false,
    })
  })

  it('passes carry_secrets: true when explicitly opted in', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, {}, true)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: true,
      live_only: false,
    })
  })

  it('passes carry_secrets: false to clone/bundle when the toggle is unchecked', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, {}, false)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: false,
      live_only: false,
    })
  })
})

describe('cloneWorkspaceTo — include-all-history toggle (GitHub #21)', () => {
  it('passes live_only: false (carry ALL history) by default', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: true,
      live_only: false,
    })
  })

  it('passes live_only: false when all-history is explicitly opted in', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, {}, true, true)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: true,
      live_only: false,
    })
  })

  it('passes live_only: true when the all-history toggle is unchecked', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order)
    await cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, {}, true, false)
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', {
      project_path: '/Users/rosson/myworkspace',
      carry_secrets: true,
      live_only: true,
    })
  })
})

describe('cloneWorkspaceTo — cancellation & errors', () => {
  it('aborts (no upload/unpack) when the folder picker is cancelled', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order, {
      pickRemoteFolder: vi.fn(async () => {
        order.push('pickRemoteFolder')
        return null
      }),
    })
    const onError = vi.fn()
    const onDone = vi.fn()

    await expect(
      cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, { onError, onDone }),
    ).rejects.toBeInstanceOf(CloneCancelledError)

    expect(spies.daemonCliPost).not.toHaveBeenCalledWith('fs/upload-binary', expect.anything())
    expect(spies.daemonCliPost).not.toHaveBeenCalledWith('clone/unpack', expect.anything())
    expect(onDone).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledOnce()
  })

  it('aborts when sign-in is cancelled (waitForHostConnected rejects)', async () => {
    const order: string[] = []
    const { deps, spies } = makeDeps(order, {
      waitForHostConnected: vi.fn(async () => {
        order.push('waitForHostConnected')
        throw new CloneCancelledError('Sign-in cancelled.')
      }),
    })
    const onError = vi.fn()

    await expect(
      cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, { onError }),
    ).rejects.toBeInstanceOf(CloneCancelledError)

    // Bundle + read happened; picker / upload / unpack did NOT.
    expect(spies.daemonCliPost).toHaveBeenCalledWith('clone/bundle', expect.anything())
    expect(spies.readLocalFileBase64).toHaveBeenCalled()
    expect(spies.pickRemoteFolder).not.toHaveBeenCalled()
    expect(spies.daemonCliPost).not.toHaveBeenCalledWith('fs/upload-binary', expect.anything())
    expect(spies.daemonCliPost).not.toHaveBeenCalledWith('clone/unpack', expect.anything())
    expect(onError).toHaveBeenCalledOnce()
  })

  it('surfaces + stops on a mid-step error (upload fails → no unpack)', async () => {
    const order: string[] = []
    const failingPost = vi.fn(async (route: string) => {
      order.push(`post:${route}`)
      if (route === 'clone/bundle') return BUNDLE as unknown
      if (route === 'fs/upload-binary') throw new Error('disk full on host')
      if (route === 'clone/unpack') return UNPACK as unknown
      return {} as unknown
    }) as unknown as CloneDeps['daemonCliPost']
    const { deps } = makeDeps(order, { daemonCliPost: failingPost })
    const onError = vi.fn()
    const onDone = vi.fn()
    const onStage = vi.fn()

    await expect(
      cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, { onError, onDone, onStage }),
    ).rejects.toThrow('disk full on host')

    // unpack never fired.
    expect(order).not.toContain('post:clone/unpack')
    expect(onDone).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledWith('disk full on host')
    expect(onStage).toHaveBeenLastCalledWith('error')
  })

  it('surfaces a bundle-step failure before any host switch', async () => {
    const order: string[] = []
    const failingPost = vi.fn(async (route: string) => {
      order.push(`post:${route}`)
      if (route === 'clone/bundle') throw new Error('no such project')
      return {} as unknown
    }) as unknown as CloneDeps['daemonCliPost']
    const { deps, spies } = makeDeps(order, { daemonCliPost: failingPost })
    const onError = vi.fn()

    await expect(
      cloneWorkspaceTo('/Users/rosson/myworkspace', DEST, deps, { onError }),
    ).rejects.toThrow('no such project')

    expect(spies.pickHost).not.toHaveBeenCalled()
    expect(spies.readLocalFileBase64).not.toHaveBeenCalled()
    expect(onError).toHaveBeenCalledWith('no such project')
  })
})
