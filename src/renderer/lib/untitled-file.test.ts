import { beforeEach, describe, expect, it, vi } from 'vitest'
import { DaemonHttpError } from '@/lib/daemon-http-error'

const daemonCliPost = vi.hoisted(() => vi.fn())
vi.mock('@/lib/daemon-cli', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@/lib/daemon-cli')>()
  return { ...actual, daemonCliPost: (...args: unknown[]) => daemonCliPost(...args) }
})
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { HostSwitchedError } from '@/lib/daemon-cli'
import { ROOM_WRITE_ROUTES } from '@/kessel/room-writes'
import { primaryScope } from '@/kessel/server-scope'
import {
  isNewUntitledFile,
  isUntitledFileName,
  localUntitledName,
  pickUntitledName,
  saveNewUntitledFile,
  untitledNamesInUse,
} from './untitled-file'

const scope = primaryScope()
// What a server older than 0.46.8 answers: an unknown POST route.
const noRoute = (): DaemonHttpError => new DaemonHttpError('route_unclassified', 404, 'route_unclassified')
const noRouteOwner = (): DaemonHttpError => new DaemonHttpError('method not allowed for this route', 405, null)

beforeEach(() => {
  daemonCliPost.mockReset()
})

describe('names', () => {
  it('knows an Untitled name', () => {
    for (const n of ['Untitled', 'Untitled-2', 'Untitled.md', 'Untitled-12.k5board', 'Untitled.tar.gz']) {
      expect(isUntitledFileName(n), n).toBe(true)
    }
    for (const n of ['untitled', 'Untitledx', 'Untitled-', 'Untitled-x', 'notes.md', 'My Untitled', 'Untitled/x']) {
      expect(isUntitledFileName(n), n).toBe(false)
    }
  })

  it('a new document is an Untitled path not yet read as text', () => {
    expect(isNewUntitledFile('/ws/Untitled', null)).toBe(true)
    expect(isNewUntitledFile('/ws/Untitled-2.md', '/ws/other.md')).toBe(true)
    expect(isNewUntitledFile('/ws/Untitled', '/ws/Untitled')).toBe(false)
    expect(isNewUntitledFile('/ws/notes.md', null)).toBe(false)
  })

  it('the local fallback takes the first number free among the tabs', () => {
    expect(localUntitledName([])).toBe('Untitled')
    expect(localUntitledName(['Untitled'])).toBe('Untitled-2')
    expect(localUntitledName(['Untitled', 'Untitled-2', 'Untitled-4'])).toBe('Untitled-3')
  })

  it('names in use come from tab titles and file viewers, leaving out the asker', () => {
    const viewer = (filePath: string) => ({ items: [{ type: 'file-viewer', data: { filePath } }] })
    const tabs = [
      { id: 'a', title: 'Untitled', paneGroups: new Map([['p', viewer('/ws/Untitled')]]) },
      { id: 'b', title: 'renamed', paneGroups: new Map([['p', viewer('/ws/Untitled-2.md')]]) },
      { id: 'c', title: 'notes.md', paneGroups: new Map([['p', viewer('/ws/notes.md')]]) },
      { id: 'd', title: 'Term', paneGroups: new Map([['p', { items: [{ type: 'terminal', data: {} }] }]]) },
    ]
    expect(untitledNamesInUse(tabs).sort()).toEqual(['Untitled', 'Untitled-2.md'])
    expect(untitledNamesInUse(tabs, 'a')).toEqual(['Untitled-2.md'])
  })
})

describe('a remote room', () => {
  it('may send both routes to its own server', () => {
    expect(ROOM_WRITE_ROUTES.has('fs/untitled-name')).toBe(true)
    expect(ROOM_WRITE_ROUTES.has('fs/create-untitled')).toBe(true)
  })
})

describe('pickUntitledName', () => {
  it('takes the daemon’s name, sending the open tabs', async () => {
    daemonCliPost.mockResolvedValue({ name: 'Untitled-3', path: '/ws/Untitled-3' })
    await expect(pickUntitledName(scope, '/ws', ['Untitled'])).resolves.toBe('Untitled-3')
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    expect(daemonCliPost.mock.calls[0].slice(1)).toEqual(['fs/untitled-name', { dir: '/ws', taken: ['Untitled'] }])
  })

  it.each([
    ['an older server, as a login', noRoute],
    ['an older server, as the owner', noRouteOwner],
    ['a daemon error', () => new DaemonHttpError('Not a folder: ~', 400, null)],
    ['no connection', () => new Error('fetch failed')],
  ])('%s: numbers from the open tabs, so Cmd+N still opens', async (_label, err) => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    daemonCliPost.mockRejectedValue(err())
    await expect(pickUntitledName(scope, '/ws', ['Untitled'])).resolves.toBe('Untitled-2')
    warn.mockRestore()
  })

  it('an answer without a name counts as no answer', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => undefined)
    daemonCliPost.mockResolvedValue({})
    await expect(pickUntitledName(scope, '/ws', [])).resolves.toBe('Untitled')
    warn.mockRestore()
  })

  it('a window that left the host gets no name', async () => {
    daemonCliPost.mockRejectedValue(new HostSwitchedError('a', 'b'))
    await expect(pickUntitledName(scope, '/ws', [])).rejects.toThrow('host switched')
  })
})

describe('saveNewUntitledFile', () => {
  it('creates under the tab’s name and never calls write-file', async () => {
    daemonCliPost.mockResolvedValue({ name: 'Untitled-2', path: '/ws/Untitled-2' })
    await expect(saveNewUntitledFile(scope, '/ws/Untitled-2', 'typed', ['Untitled'])).resolves.toBe('/ws/Untitled-2')
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
    expect(daemonCliPost.mock.calls[0].slice(1)).toEqual([
      'fs/create-untitled',
      { dir: '/ws', prefer: 'Untitled-2', taken: ['Untitled'], content: 'typed' },
    ])
  })

  it('returns the other name when someone took this one', async () => {
    daemonCliPost.mockResolvedValue({ name: 'Untitled-2.md', path: 'C:\\ws\\Untitled-2.md' })
    await expect(saveNewUntitledFile(scope, '/ws/deep/Untitled.md', 'typed', [])).resolves.toBe('/ws/deep/Untitled-2.md')
  })

  it('keeps a Windows path’s separators', async () => {
    daemonCliPost.mockResolvedValue({ name: 'Untitled-2' })
    await expect(saveNewUntitledFile(scope, 'C:\\ws\\Untitled', 'x', [])).resolves.toBe('C:\\ws\\Untitled-2')
    expect(daemonCliPost.mock.calls[0][2]).toMatchObject({ dir: 'C:\\ws', prefer: 'Untitled' })
  })

  it('a file at the root of the disk still names the root', async () => {
    daemonCliPost.mockResolvedValue({ name: 'Untitled' })
    await saveNewUntitledFile(scope, '/Untitled', 'x', [])
    expect(daemonCliPost.mock.calls[0][2]).toMatchObject({ dir: '/', prefer: 'Untitled' })
  })

  it.each([noRoute, noRouteOwner])('an older server gets the plain write it always got', async (err) => {
    daemonCliPost.mockRejectedValueOnce(err())
    daemonCliPost.mockResolvedValueOnce({ success: true })
    await expect(saveNewUntitledFile(scope, '/ws/Untitled', 'typed', [])).resolves.toBe('/ws/Untitled')
    expect(daemonCliPost.mock.calls[1].slice(1)).toEqual(['fs/write-file', { path: '/ws/Untitled', content: 'typed' }])
  })

  it('an answer without a name fails the save', async () => {
    daemonCliPost.mockResolvedValue({ success: true })
    await expect(saveNewUntitledFile(scope, '/ws/Untitled', 'typed', [])).rejects.toThrow('without a name')
  })

  it('any other refusal fails the save and writes nothing else', async () => {
    daemonCliPost.mockRejectedValue(new DaemonHttpError('Permission denied', 400, null))
    await expect(saveNewUntitledFile(scope, '/ws/Untitled', 'typed', [])).rejects.toThrow('Permission denied')
    expect(daemonCliPost).toHaveBeenCalledTimes(1)
  })
})
