// @vitest-environment jsdom
// The checked write: `expect` (the SHA-256 of the text K2 last read, or
// "absent") on every write, the 409 refusals read back, other failures
// thrown, and the leave write keeping the words as a draft when it can't land.

import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const post = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: (...a: unknown[]) => post(...a),
  daemonCliGet: vi.fn(),
  isHostSwitchedError: () => false,
}))

import { DaemonHttpError } from '@/lib/daemon-http-error'
import { getDraft, resetDraftsForTests } from './drafts'
import { conflictOf, failureReason, leaveWrite, sha256Hex, writeChecked } from './write'

const scope = { hostKey: 'local' } as never
const EMPTY_SHA = 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855'
const conflict = (code: string, current?: object): DaemonHttpError => new DaemonHttpError('conflict', 409, code, null, { error: 'conflict', code, ...(current ? { current } : {}) })

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory()
  resetDraftsForTests()
  post.mockReset()
})
afterEach(() => vi.restoreAllMocks())

describe('sha256Hex', () => {
  it('64 lowercase hex of the UTF-8 bytes', async () => {
    expect(await sha256Hex('')).toBe(EMPTY_SHA)
    expect(await sha256Hex('abc')).toBe('ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad')
    expect(await sha256Hex('é')).toMatch(/^[0-9a-f]{64}$/)
  })
})

describe('writeChecked', () => {
  it('sends expect = the hash of the base on every write', async () => {
    post.mockResolvedValue({ ok: true })
    await writeChecked(scope, '/ws/a.md', 'new', 'abc')
    await writeChecked(scope, '/ws/a.md', 'newer', '')
    expect(post.mock.calls.map((c) => c[2])).toEqual([
      { path: '/ws/a.md', content: 'new', expect: await sha256Hex('abc') },
      { path: '/ws/a.md', content: 'newer', expect: EMPTY_SHA },
    ])
    expect(post.mock.calls[0][1]).toBe('fs/write-file')
  })

  it('a file K2 saw missing sends expect "absent"', async () => {
    post.mockResolvedValue({})
    await writeChecked(scope, '/ws/new.md', 'x', null)
    expect(post.mock.calls[0][2]).toMatchObject({ expect: 'absent' })
  })

  it('reads each 409 code back, with the file’s text when it came', async () => {
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'f'.repeat(64), content: 'theirs' }))
    expect(await writeChecked(scope, '/a', 'y', 'b')).toEqual({ ok: false, code: 'file_changed', current: { sha256: 'f'.repeat(64), content: 'theirs' } })
    post.mockRejectedValueOnce(conflict('file_missing'))
    expect(await writeChecked(scope, '/a', 'y', 'b')).toEqual({ ok: false, code: 'file_missing', current: null })
    post.mockRejectedValueOnce(conflict('file_exists', { sha256: 'a'.repeat(64), content: 'made' }))
    expect(await writeChecked(scope, '/a', 'y', null)).toMatchObject({ ok: false, code: 'file_exists', current: { content: 'made' } })
    // A guest: the hash only.
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'a'.repeat(64) }))
    expect(await writeChecked(scope, '/a', 'y', 'b')).toEqual({ ok: false, code: 'file_changed', current: { sha256: 'a'.repeat(64) } })
  })

  it('any other failure is thrown (a 500, a 409 of another kind)', async () => {
    post.mockRejectedValueOnce(new DaemonHttpError('No space left on device (os error 28)', 500, null))
    await expect(writeChecked(scope, '/a', 'y', 'b')).rejects.toThrow('No space left')
    post.mockRejectedValueOnce(new DaemonHttpError('locked', 409, 'board_locked'))
    await expect(writeChecked(scope, '/a', 'y', 'b')).rejects.toThrow('locked')
    expect(conflictOf(new Error('x'))).toBeNull()
  })
})

describe('failureReason', () => {
  it('says why in plain words', () => {
    expect(failureReason(new Error('No space left on device (os error 28)'))).toBe('the drive is full')
    expect(failureReason(new Error('Permission denied (os error 13)'))).toBe('K2 isn’t allowed to write it')
    expect(failureReason(new Error('Read-only file system (os error 30)'))).toBe('the drive is read-only')
    expect(failureReason(new Error('Failed to fetch'))).toBe('the server can’t be reached')
    expect(failureReason(new Error('Something odd.'))).toBe('something odd')
  })
})

describe('leaveWrite (closing the tab)', () => {
  it('a landed write leaves no draft', async () => {
    post.mockResolvedValue({})
    await leaveWrite(scope, '/ws/a.md', 'mine', 'base')
    expect(post.mock.calls[0][2]).toMatchObject({ expect: await sha256Hex('base') })
    expect(await getDraft('local', '/ws/a.md')).toBeNull()
  })
  it('a refused or failed write overwrites nothing and keeps the words as a draft', async () => {
    post.mockRejectedValueOnce(conflict('file_changed', { sha256: 'f'.repeat(64), content: 'theirs' }))
    await leaveWrite(scope, '/ws/a.md', 'mine', 'base')
    expect(await getDraft('local', '/ws/a.md')).toMatchObject({ text: 'mine', base: 'base' })
    post.mockRejectedValueOnce(new Error('No space left on device'))
    await leaveWrite(scope, '/ws/b.md', 'mine b', null)
    expect(await getDraft('local', '/ws/b.md')).toMatchObject({ text: 'mine b', base: null })
  })
})
