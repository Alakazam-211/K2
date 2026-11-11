// @vitest-environment jsdom
// Delete message (thread-message-delete-v1, 0.46.11). Rosson: "If a user
// enters something sensitive into the threads, we need to give them a way
// to delete the sensitive message."
//
//   1. A deleted post draws as "You deleted this message" for its sender and
//      "Message deleted" for everyone else: muted, no words, no files, no menu.
//   2. Your own post's menu has Delete message where the Thread's server has
//      the feature; someone else's post, or a server without it, has none.
//   3. Delete asks first (the agreed words), then posts
//      `thread/message/delete {addr, id}` to the Thread's server; Cancel
//      sends nothing.
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({ ok: true, deleted: true })),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.daemonCliGet, daemonCliPost: h.daemonCliPost }))

import { act, cleanup, fireEvent, render, screen } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { fakeScope } from '@/test-utils/fake-scope'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useContextMenuStore } from '@/stores/context-menu'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'
import { DELETE_MESSAGE_CONFIRM, deleteThreadMessage, threadMessageMenuItems } from './ThreadMessageMenu'
import { ThreadBubble } from './ThreadZenKit'

const SECRET = 'my ledger password is hunter2'

function scopeWith(features: string[]): ServerScope {
  return { ...fakeScope('box.example.k2.dev'), serverSupports: (f: string) => features.includes(f) } as ServerScope
}

function post(id: string, extra: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: 4,
    id,
    doc: { id, kind: 'text', from: 'ada', via: 'compose', author: { kind: 'user', login: 'ada' }, created_at: 1_762_700_000, body: SECRET, ...extra },
  }
}

function tombstone(id: string): OverlayThreadItem {
  return post(id, { body: null, deleted: true, deletedAt: 1_762_700_100, deletedBy: { kind: 'user', login: 'ada' } })
}

function draw(item: OverlayThreadItem, mine: boolean, scope: ServerScope = scopeWith(['thread-message-delete-v1'])) {
  return render(
    <ThreadAttachmentSourceProvider value={{ scope, addr: 'ledger' }}>
      <ThreadBubble item={item} mine={mine} author={mine ? 'You' : 'ada'} timeLabel="now" viewer={null} />
    </ThreadAttachmentSourceProvider>,
  )
}

function menuLabels(): string[] {
  return useContextMenuStore
    .getState()
    .items.filter((i) => i.type !== 'separator')
    .map((i) => i.label)
}

afterEach(() => {
  cleanup()
  act(() => useContextMenuStore.getState().close())
  useConfirmDialogStore.getState().close()
  h.daemonCliPost.mockClear()
})

describe('a deleted post', () => {
  it('reads "You deleted this message" to its sender and "Message deleted" to others, with no words', () => {
    const mine = draw(tombstone('d1'), true)
    const note = mine.container.querySelector('[data-thread-deleted]')
    expect(note?.textContent).toBe('You deleted this message')
    expect(mine.container.textContent).not.toContain('hunter2')
    expect(mine.container.querySelector('[data-zen-bubble]')).toBeNull()
    expect(mine.container.querySelector('[data-zen-message-more]')).toBeNull()
    cleanup()

    const theirs = draw(tombstone('d2'), false)
    expect(theirs.container.querySelector('[data-thread-deleted]')?.textContent).toBe('Message deleted')
    expect(theirs.container.textContent).not.toContain('hunter2')
  })

  it('replaces the bubble when the frame turns the post into a tombstone', () => {
    const scope = scopeWith(['thread-message-delete-v1'])
    const view = draw(post('d3'), true, scope)
    expect(view.container.textContent).toContain(SECRET)
    view.rerender(
      <ThreadAttachmentSourceProvider value={{ scope, addr: 'ledger' }}>
        <ThreadBubble item={tombstone('d3')} mine author="You" timeLabel="now" viewer={null} />
      </ThreadAttachmentSourceProvider>,
    )
    expect(view.container.textContent).not.toContain('hunter2')
    expect(view.container.querySelector('[data-thread-deleted]')?.textContent).toBe('You deleted this message')
  })
})

describe('Delete message in the menu', () => {
  it('is on your own post where the server has the feature, and nowhere else', () => {
    draw(post('m1'), true)
    fireEvent.contextMenu(document.querySelector('[data-zen-bubble]') as HTMLElement)
    expect(menuLabels()).toContain('Delete message')
    act(() => useContextMenuStore.getState().close())
    cleanup()

    draw(post('m2'), false)
    fireEvent.contextMenu(document.querySelector('[data-zen-bubble]') as HTMLElement)
    expect(menuLabels()).not.toContain('Delete message')
    act(() => useContextMenuStore.getState().close())
    cleanup()

    draw(post('m3'), true, scopeWith([]))
    fireEvent.contextMenu(document.querySelector('[data-zen-bubble]') as HTMLElement)
    expect(menuLabels()).not.toContain('Delete message')
  })

  it('has no row without a delete target', () => {
    const base = { scope: null, text: 'x', agent: null, agentName: null, reply: null }
    expect(threadMessageMenuItems(base, null).some((i) => i.id === 'delete')).toBe(false)
    const del = { scope: scopeWith([]), addr: 'ledger', id: 'm4' }
    expect(threadMessageMenuItems({ ...base, del }, null).some((i) => i.id === 'delete' && i.label === 'Delete message')).toBe(true)
  })
})

describe('deleting', () => {
  it('asks with the agreed words, then posts the delete to the Thread’s server', async () => {
    const scope = scopeWith(['thread-message-delete-v1'])
    const done = deleteThreadMessage({ scope, addr: 'ledger', id: 'x1' })
    const dialog = useConfirmDialogStore.getState()
    expect(dialog.isOpen).toBe(true)
    expect(dialog.title).toBe('Delete this message?')
    expect(dialog.message).toBe("It's removed from this Thread and from the agent's server. The agent is told you deleted it; it can't be undone.")
    expect(dialog.confirmLabel).toBe(DELETE_MESSAGE_CONFIRM.confirmLabel)
    expect(dialog.confirmDestructive).toBe(true)
    act(() => dialog.onResolve?.(true))
    await expect(done).resolves.toBe(true)
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith(scope, 'thread/message/delete', { addr: 'ledger', id: 'x1' })
  })

  it('sends nothing on Cancel', async () => {
    const done = deleteThreadMessage({ scope: scopeWith(['thread-message-delete-v1']), addr: 'ledger', id: 'x2' })
    act(() => useConfirmDialogStore.getState().close())
    await expect(done).resolves.toBe(false)
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    expect(screen.queryByText('Delete this message?')).toBeNull()
  })
})
