// Fail loud: the tray caps at 6 (a 3 × 2 grid), ⌘↩ sends the typed note
// past a full tray, clears only delivered notes, and Retry
// redelivers the stored Thread item instead of posting a second one.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliPost: vi.fn(),
}))

import { daemonCliPost } from '@/lib/daemon-cli'
import type { ServerScope } from '@/kessel/server-scope'
import { MAX_DESIGN_NOTES, NOTES_FULL_REASON, badgesFor, useDesignNotesStore } from './useDesignNotes'
import { DESIGN_NOTES_REDELIVER_ROUTE, DESIGN_NOTES_ROUTE, buildNotesBody } from './designApi'

const scope = { id: 'primary', isPrimary: true } as unknown as ServerScope
const target = { scope, workspace: 'docs' }
const ITEM = 'design-1'
const post = vi.mocked(daemonCliPost)

function addNote(comment: string, selector = 'button.cta'): string {
  const r = useDesignNotesStore.getState().add(ITEM, {
    comment,
    element: { selector, tag: 'button', text: 'Buy', html: '<button>Buy</button>' },
    url: 'http://localhost:5173/pricing',
    title: 'Pricing',
    viewport: { width: 1280, height: 800 },
  })
  if (!r.ok) throw new Error(`add failed: ${r.reason}`)
  return r.id
}

function tray() {
  const t = useDesignNotesStore.getState().trays[ITEM]
  if (!t) throw new Error('no tray')
  return t
}

function ok(delivered: boolean, threadItemId = 'thr-1') {
  return { ok: true, threadItemId, delivered, targetSessionId: delivered ? 'sess-1' : null, addr: 'docs', notes: 1 }
}

beforeEach(() => {
  useDesignNotesStore.setState({ trays: {} })
  post.mockReset()
})

afterEach(() => {
  vi.clearAllMocks()
})

describe('useDesignNotes', () => {
  it('caps the tray at 6 and refuses the 7th with a reason', () => {
    expect(MAX_DESIGN_NOTES).toBe(6)
    expect(NOTES_FULL_REASON).toBe('Send these first; up to 6 notes')
    for (let i = 0; i < MAX_DESIGN_NOTES; i++) addNote(`n${i}`)
    const r = useDesignNotesStore.getState().add(ITEM, { comment: 'one more', url: 'http://x/', title: '' })
    expect(r).toEqual({ ok: false, reason: NOTES_FULL_REASON })
    expect(tray().notes).toHaveLength(MAX_DESIGN_NOTES)
  })

  it('sendNow on a full tray sends all 6 plus the typed note in one post', async () => {
    for (let i = 0; i < MAX_DESIGN_NOTES; i++) addNote(`n${i}`)
    post.mockResolvedValueOnce(ok(true))
    await useDesignNotesStore.getState().sendNow(ITEM, target, { comment: 'typed', url: 'http://localhost:5173/pricing', title: 'Pricing' })
    expect(post).toHaveBeenCalledTimes(1)
    const body = post.mock.calls[0][2] as { notes: { comment: string }[] }
    expect(body.notes.map((n) => n.comment)).toEqual(['n0', 'n1', 'n2', 'n3', 'n4', 'n5', 'typed'])
    expect(tray().notes).toEqual([])
  })

  it('sendNow with nothing typed sends the tray', async () => {
    addNote('only')
    post.mockResolvedValueOnce(ok(true))
    await useDesignNotesStore.getState().sendNow(ITEM, target)
    expect((post.mock.calls[0][2] as { notes: unknown[] }).notes).toHaveLength(1)
  })

  it('refuses an empty note', () => {
    const r = useDesignNotesStore.getState().add(ITEM, { comment: '   ', url: 'http://x/', title: '' })
    expect(r.ok).toBe(false)
  })

  it('keeps trays per Design item', () => {
    addNote('a')
    useDesignNotesStore.getState().add('design-2', { comment: 'b', url: 'http://x/', title: '' })
    expect(tray().notes.map((n) => n.comment)).toEqual(['a'])
    expect(useDesignNotesStore.getState().trays['design-2']?.notes.map((n) => n.comment)).toEqual(['b'])
  })

  it('edit, remove and badge numbers follow tray order', () => {
    const a = addNote('a', 'h1')
    addNote('b', 'p')
    useDesignNotesStore.getState().edit(ITEM, a, { comment: 'a2', intent: 'question' })
    expect(tray().notes[0]).toMatchObject({ comment: 'a2', intent: 'question' })
    expect(badgesFor(tray())).toEqual([
      { n: 1, selector: 'h1' },
      { n: 2, selector: 'p' },
    ])
    useDesignNotesStore.getState().remove(ITEM, a)
    expect(badgesFor(tray())).toEqual([{ n: 1, selector: 'p' }])
  })

  it('Send posts every note once and clears them when delivered', async () => {
    addNote('a')
    addNote('b')
    post.mockResolvedValueOnce(ok(true))
    await useDesignNotesStore.getState().send(ITEM, target)
    expect(post).toHaveBeenCalledTimes(1)
    const [, route, body] = post.mock.calls[0]
    expect(route).toBe(DESIGN_NOTES_ROUTE)
    expect(body).toMatchObject({
      workspace: 'docs',
      itemId: ITEM,
      page: { url: 'http://localhost:5173/pricing', title: 'Pricing', viewport: { width: 1280, height: 800 } },
    })
    expect((body as { notes: unknown[] }).notes).toHaveLength(2)
    expect(tray().notes).toHaveLength(0)
    expect(tray().sending).toBe(false)
    expect(tray().error).toBeNull()
  })

  it('a note added while sending survives a delivered send', async () => {
    addNote('a')
    let resolve!: (v: unknown) => void
    post.mockImplementationOnce(() => new Promise((r) => (resolve = r)))
    const p = useDesignNotesStore.getState().send(ITEM, target)
    expect(tray().sending).toBe(true)
    addNote('late')
    resolve(ok(true))
    await p
    expect(tray().notes.map((n) => n.comment)).toEqual(['late'])
  })

  it('undelivered keeps the notes; Retry redelivers the same item and never posts again', async () => {
    addNote('a')
    post.mockResolvedValueOnce(ok(false, 'thr-9'))
    await useDesignNotesStore.getState().send(ITEM, target)
    expect(tray().notes).toHaveLength(1)
    expect(tray().pendingThreadItemId).toBe('thr-9')
    expect(tray().error).toMatch(/did not get it/)

    // Send while pending is a Retry too.
    post.mockResolvedValueOnce(ok(false, 'thr-9'))
    await useDesignNotesStore.getState().send(ITEM, target)
    post.mockResolvedValueOnce(ok(true, 'thr-9'))
    await useDesignNotesStore.getState().retry(ITEM, target)

    const routes = post.mock.calls.map((c) => c[1])
    expect(routes).toEqual([DESIGN_NOTES_ROUTE, DESIGN_NOTES_REDELIVER_ROUTE, DESIGN_NOTES_REDELIVER_ROUTE])
    expect(post.mock.calls[2][2]).toEqual({ workspace: 'docs', threadItemId: 'thr-9' })
    expect(tray().notes).toHaveLength(0)
    expect(tray().pendingThreadItemId).toBeNull()
  })

  it('a note inside an undelivered item cannot be edited', async () => {
    const a = addNote('a')
    post.mockResolvedValueOnce(ok(false))
    await useDesignNotesStore.getState().send(ITEM, target)
    useDesignNotesStore.getState().edit(ITEM, a, { comment: 'changed' })
    expect(tray().notes[0].comment).toBe('a')
  })

  it('an HTTP error keeps the notes and shows the error', async () => {
    addNote('a')
    post.mockRejectedValueOnce(new Error('workspace not found'))
    await useDesignNotesStore.getState().send(ITEM, target)
    expect(tray().notes).toHaveLength(1)
    expect(tray().error).toBe('workspace not found')
    expect(tray().sending).toBe(false)
    expect(tray().pendingThreadItemId).toBeNull()
  })

  it('a malformed 200 is an error, not a silent clear', async () => {
    addNote('a')
    post.mockResolvedValueOnce({ ok: true })
    await useDesignNotesStore.getState().send(ITEM, target)
    expect(tray().notes).toHaveLength(1)
    expect(tray().error).toMatch(/unexpected design notes response/)
  })

  it('Send with no notes makes no call', async () => {
    await useDesignNotesStore.getState().send(ITEM, target)
    expect(post).not.toHaveBeenCalled()
  })

  it('a whole-page note has no payload', () => {
    const body = buildNotesBody('docs', [{ id: 'x', comment: 'c', intent: 'question', url: 'http://x/', title: 't' }])
    expect(body.notes).toEqual([{ comment: 'c', intent: 'question' }])
    expect(() => buildNotesBody('docs', [])).toThrow('no notes')
  })
})
