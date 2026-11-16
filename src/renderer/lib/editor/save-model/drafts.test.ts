// @vitest-environment jsdom
// Drafts: unsaved words kept on this computer per server and file, and what
// a restart brings back (as is, merged with a file changed meanwhile, or
// nothing when the file already has them).

import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { clearDraft, DRAFT_MAX_AGE_MS, getDraft, putDraft, resetDraftsForTests, type Draft } from './drafts'
import { restoreDraft } from './useSaveModel'

beforeEach(() => {
  globalThis.indexedDB = new IDBFactory()
  resetDraftsForTests()
})

describe('drafts', () => {
  it('put, get and clear, per server and path', async () => {
    await putDraft('local', '/ws/README.md', 'mine', 'disk')
    await putDraft('box.k2.dev', '/ws/README.md', 'other server', 'disk')
    expect(await getDraft('local', '/ws/README.md')).toMatchObject({ text: 'mine', base: 'disk', path: '/ws/README.md', hostKey: 'local' })
    expect((await getDraft('box.k2.dev', '/ws/README.md'))?.text).toBe('other server')
    await clearDraft('local', '/ws/README.md')
    expect(await getDraft('local', '/ws/README.md')).toBeNull()
    expect(await getDraft('box.k2.dev', '/ws/README.md')).not.toBeNull()
  })

  it('survives a restart (a new window reads the database, not memory)', async () => {
    await putDraft('local', '/ws/a.md', 'kept', 'disk')
    resetDraftsForTests()
    expect((await getDraft('local', '/ws/a.md'))?.text).toBe('kept')
  })

  it('without IndexedDB, this window still keeps them', async () => {
    // @ts-expect-error: a window without it
    globalThis.indexedDB = undefined
    resetDraftsForTests()
    await putDraft('local', '/ws/a.md', 'memory', null)
    expect((await getDraft('local', '/ws/a.md'))?.text).toBe('memory')
  })

  it('a draft older than a month is dropped', async () => {
    const now = Date.now()
    const spy = vi.spyOn(Date, 'now').mockReturnValue(now - DRAFT_MAX_AGE_MS - 1000)
    await putDraft('local', '/ws/old.md', 'old', 'disk')
    spy.mockRestore()
    expect(await getDraft('local', '/ws/old.md')).toBeNull()
  })
})

describe('restoreDraft', () => {
  const draft = (text: string, base: string | null): Draft => ({ key: 'k', hostKey: 'local', path: '/ws/a.md', text, base, savedAt: Date.now(), v: 1 })
  it('the file is as it was: the draft comes back as is', () => {
    expect(restoreDraft(draft('a\nmine\n', 'a\n'), 'a\n', false)).toEqual({ text: 'a\nmine\n', kept: [] })
  })
  it('the file already has the words: nothing to restore', () => {
    expect(restoreDraft(draft('a\n', 'x\n'), 'a\n', false)).toBeNull()
  })
  it('the file changed while K2 was closed: merged, never overwritten', () => {
    const r = restoreDraft(draft('one\ntwo mine\nthree\n', 'one\ntwo\nthree\n'), 'one\ntwo\nthree theirs\n', false)
    expect(r?.text).toBe('one\ntwo mine\nthree theirs\n')
  })
  it('a new file that is still missing: the draft comes back', () => {
    expect(restoreDraft(draft('new words', null), '', true)).toEqual({ text: 'new words', kept: [] })
  })
})
