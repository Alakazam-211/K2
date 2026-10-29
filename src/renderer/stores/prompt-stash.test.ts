// @vitest-environment jsdom
// Prompt stash store (spec-prompt-stash-v1): IndexedDB on this device, the
// 50 cap that asks, tombstones, and a list that no server switch changes.
// A fake IndexedDB stands in for the webview's; a "reload" closes the
// database and empties the store, then reads it back. Fail loud — no skip.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { closeStashDbForTests, readAllStashRecords } from '@/lib/prompt-stash/idb'
import {
  insertStashBelow,
  STASH_CAP,
  STASH_TOMBSTONE_TTL_MS,
  stashBody,
  type StashDraft,
} from '@/lib/prompt-stash/record'
import { readStashFile, saveToStash, usePromptStashStore } from './prompt-stash'

function draft(text: string, server = 'local'): StashDraft {
  return {
    text,
    source: { server_id: server, server_label: server === 'local' ? 'This computer' : server, user: 'owner', agent: 'sales', agent_name: 'Sales' },
  }
}

async function reload(): Promise<void> {
  await closeStashDbForTests()
  usePromptStashStore.setState({ items: [], loaded: false })
  await usePromptStashStore.getState().load()
}

beforeEach(async () => {
  await closeStashDbForTests()
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  localStorage.clear()
  usePromptStashStore.setState({ items: [], loaded: false, removeAfterUse: false })
})
afterEach(async () => {
  await closeStashDbForTests()
})

describe('saving', () => {
  it('writes the shared record shape and survives a reload, newest first', async () => {
    const s = usePromptStashStore.getState()
    const a = await s.save(draft('first'), [], { now: 1_000 })
    const b = await s.save(
      { ...draft('second'), quotes: [{ text: 'quoted', from: 'k2' }] },
      [{ name: 'shot.png', type: 'image/png', bytes: new Blob([new Uint8Array([1, 2, 3])], { type: 'image/png' }) }],
      { now: 2_000 },
    )
    if (a.status !== 'saved' || b.status !== 'saved') throw new Error('save did not save')
    await reload()
    const items = usePromptStashStore.getState().items
    expect(items.map((r) => r.text)).toEqual(['second', 'first'])
    const rec = items[0]
    expect(rec.v).toBe(1)
    expect(rec.id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
    expect(rec.created_at).toBe(2_000)
    expect(rec.updated_at).toBe(2_000)
    expect(rec.deleted_at).toBeNull()
    expect(rec.quotes).toEqual([{ text: 'quoted', from: 'k2' }])
    expect(rec.source).toEqual({ server_id: 'local', server_label: 'This computer', user: 'owner', agent: 'sales', agent_name: 'Sales' })
    // Bytes beside the record, never inside it.
    expect(rec.attachments).toHaveLength(1)
    expect(Object.keys(rec.attachments[0]).sort()).toEqual(['id', 'name', 'size', 'type'])
    expect(rec.attachments[0]).toMatchObject({ name: 'shot.png', type: 'image/png', size: 3 })
    const file = await readStashFile(rec.attachments[0])
    if (!file) throw new Error('attachment bytes were not stored')
    expect(new Uint8Array(await file.arrayBuffer())).toEqual(new Uint8Array([1, 2, 3]))
  })

  it('a delete leaves a tombstone with text and attachments cleared, and drops the bytes', async () => {
    const s = usePromptStashStore.getState()
    const r = await s.save(draft('gone soon'), [{ name: 'a.txt', type: 'text/plain', bytes: new Blob(['hi']) }])
    if (r.status !== 'saved') throw new Error('not saved')
    await s.remove(r.record.id)
    expect(usePromptStashStore.getState().items).toEqual([])
    const all = await readAllStashRecords()
    expect(all).toHaveLength(1)
    expect(all[0]).toMatchObject({ id: r.record.id, text: '', attachments: [], quotes: [] })
    expect(typeof all[0].deleted_at).toBe('number')
    expect(await readStashFile(r.record.attachments[0])).toBeNull()
  })

  it('tombstones older than 30 days are dropped on the next save', async () => {
    const s = usePromptStashStore.getState()
    const r = await s.save(draft('old'), [], { now: 1_000 })
    if (r.status !== 'saved') throw new Error('not saved')
    await s.remove(r.record.id)
    const deletedAt = (await readAllStashRecords())[0].deleted_at
    if (deletedAt === null) throw new Error('no tombstone')
    await s.save(draft('new'), [], { now: deletedAt + STASH_TOMBSTONE_TTL_MS + 1 })
    const all = await readAllStashRecords()
    expect(all.map((x) => x.text)).toEqual(['new'])
  })
})

describe('the 50 cap', () => {
  async function fill(): Promise<void> {
    for (let i = 0; i < STASH_CAP; i++) {
      const r = await usePromptStashStore.getState().save(draft(`p${i}`), [], { now: 1_000 + i })
      if (r.status !== 'saved') throw new Error(`save ${i} refused`)
    }
  }

  it('the 51st save writes nothing and reports the oldest', async () => {
    await fill()
    const r = await usePromptStashStore.getState().save(draft('one more'), [], { now: 9_999 })
    expect(r.status).toBe('full')
    if (r.status !== 'full') throw new Error('unreachable')
    expect(r.oldest.text).toBe('p0')
    const live = usePromptStashStore.getState().items
    expect(live).toHaveLength(STASH_CAP)
    expect(live.some((x) => x.text === 'one more')).toBe(false)
  })

  it('asks before removing the oldest; no means nothing changes', async () => {
    await fill()
    const ask = vi.fn(async () => false)
    const r = await saveToStash(draft('one more'), [], ask)
    expect(ask).toHaveBeenCalledTimes(1)
    expect(r.status).toBe('cancelled')
    expect(usePromptStashStore.getState().items.map((x) => x.text)).toContain('p0')
    expect(usePromptStashStore.getState().items).toHaveLength(STASH_CAP)
  })

  it('yes tombstones the oldest and saves the new one; the cap counts live items only', async () => {
    await fill()
    const r = await saveToStash(draft('one more'), [], async () => true)
    expect(r.status).toBe('saved')
    const live = usePromptStashStore.getState().items
    expect(live).toHaveLength(STASH_CAP)
    expect(live[0].text).toBe('one more')
    expect(live.some((x) => x.text === 'p0')).toBe(false)
    const all = await readAllStashRecords()
    expect(all).toHaveLength(STASH_CAP + 1)
    expect(all.filter((x) => x.deleted_at !== null)).toHaveLength(1)
  })
})

describe('remove after use', () => {
  it('is off by default: a used item stays', async () => {
    const s = usePromptStashStore.getState()
    expect(s.removeAfterUse).toBe(false)
    const r = await s.save(draft('template'))
    if (r.status !== 'saved') throw new Error('not saved')
    await s.markUsed(r.record.id)
    expect(usePromptStashStore.getState().items.map((x) => x.text)).toEqual(['template'])
  })

  it('on: a used item goes, and the setting is kept on this device', async () => {
    usePromptStashStore.getState().setRemoveAfterUse(true)
    expect(localStorage.getItem('k2.promptStash.removeAfterUse')).toBe('1')
    const r = await usePromptStashStore.getState().save(draft('once'))
    if (r.status !== 'saved') throw new Error('not saved')
    await usePromptStashStore.getState().markUsed(r.record.id)
    expect(usePromptStashStore.getState().items).toEqual([])
  })
})

describe('per person on this device, not per server', () => {
  it('items from two servers sit in one list, whatever server the window is on', async () => {
    const s = usePromptStashStore.getState()
    await s.save(draft('on local', 'local'), [], { now: 1 })
    await s.save(draft('on remote', 'box.k2.dev'), [], { now: 2 })
    // A server switch: the connect-host store moves; the stash never looks.
    const { useConnectHostStore, onActiveHostChange } = await import('@/stores/connect-host')
    const seen = vi.fn()
    const off = onActiveHostChange(seen)
    const before = usePromptStashStore.getState().items
    useConnectHostStore.setState({
      activeHost: { id: 'h1', label: 'Box', hostname: 'box.k2.dev', port: 443, token: 't' } as never,
    })
    off()
    useConnectHostStore.setState({ activeHost: 'local' })
    expect(seen).toHaveBeenCalledTimes(1)
    // The switch touched nothing: same list object, same items.
    expect(usePromptStashStore.getState().items).toBe(before)
    await reload()
    const items = usePromptStashStore.getState().items
    expect(items.map((x) => [x.text, x.source.server_id])).toEqual([
      ['on remote', 'box.k2.dev'],
      ['on local', 'local'],
    ])
  })

  it('the store never subscribes to server switches', async () => {
    const { readFileSync } = await import('node:fs')
    const { resolve } = await import('node:path')
    const src = readFileSync(resolve(__dirname, 'prompt-stash.ts'), 'utf8')
    expect(src).not.toMatch(/onActiveHostChange\(/)
    expect(src).not.toMatch(/from '@\/stores\/connect-host'/)
  })
})

describe('putting a stash back', () => {
  it('goes below existing text after a blank line, quotes first', () => {
    const body = stashBody({ text: 'do the thing', quotes: [{ text: 'line one\nline two', from: 'k2' }] })
    expect(body).toBe('> k2:\n> line one\n> line two\n\ndo the thing')
    expect(insertStashBelow('already here  \n', body)).toBe(`already here\n\n${body}`)
    expect(insertStashBelow('', 'x')).toBe('x')
    expect(insertStashBelow('   ', 'x')).toBe('x')
  })
})
