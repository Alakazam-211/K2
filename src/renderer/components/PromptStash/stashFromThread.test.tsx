// @vitest-environment jsdom
// Saving into the prompt stash from outside a composer (spec-prompt-stash-v1):
// "Save to stash" in an agent message's menu, the `stash.offer` card, and
// "Save for later" in the Inbox "Chat about this" dialog. None of them sends
// anything to the daemon. Fail loud — no skip.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string): Promise<unknown> => {
    // This computer's Connect host: its stable `server_id`.
    if (route === 'tunnel/status') return { public_url: 'https://rosson.k2.dev' }
    return {}
  }),
  daemonCliPost: vi.fn(async () => ({ ok: true })),
}))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.daemonCliGet, daemonCliPost: h.daemonCliPost }))

import { ThreadBubble } from '@/components/Thread/ThreadZenKit'
import { ThreadAttachmentSourceProvider } from '@/components/Thread/ThreadImageObjects'
import { coerceStashOffer, type OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { InboxChatAboutDialog } from '@/components/AgentPane/InboxChatAboutDialog'
import { closeStashDbForTests } from '@/lib/prompt-stash/idb'
import { resetStashServerIdsForTests } from '@/lib/prompt-stash/compose'
import { resetSavedOffersForTests } from './StashFromThread'
import { primaryScope } from '@/kessel/server-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { useContextMenuStore } from '@/stores/context-menu'
import { usePromptStashStore } from '@/stores/prompt-stash'
import { isMacPlatform } from '@/lib/desktop-chrome'

const room = testRoom({ tabs: {}, scope: primaryScope(), projects: [] })
const OWNER = { principal: 'owner', aliases: [] as string[] }

function agentItem(doc: Partial<OverlayThreadItem['doc']> = {}): OverlayThreadItem {
  return { collection: 'thread', seq: 1, id: 'a1', doc: { id: 'a1', kind: 'text', from: 'k2', body: 'Try: refactor the parser', created_at: 1, ...doc } }
}

function renderBubble(item: OverlayThreadItem, opts: { mine?: boolean; source?: boolean } = {}): void {
  const bubble = (
    <ThreadBubble item={item} mine={opts.mine ?? false} author={opts.mine ? 'You' : 'k2'} timeLabel="now" viewer={OWNER} />
  )
  renderInRoom(
    room,
    opts.source === false ? bubble : <ThreadAttachmentSourceProvider value={{ scope: primaryScope(), addr: 'k2' }}>{bubble}</ThreadAttachmentSourceProvider>,
  )
}

beforeEach(async () => {
  cleanup()
  await closeStashDbForTests()
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  localStorage.clear()
  resetStashServerIdsForTests()
  resetSavedOffersForTests()
  usePromptStashStore.setState({ items: [], loaded: false, removeAfterUse: false })
  useContextMenuStore.getState().close()
  h.daemonCliPost.mockClear()
})
afterEach(async () => {
  cleanup()
  await closeStashDbForTests()
})

describe('"Save to stash" in a message\'s menu', () => {
  it('the message menu (right-click or ⋯) saves the text with the agent as its source', async () => {
    renderBubble(agentItem())
    const bubble = document.querySelector('[data-zen-bubble="agent"]') as HTMLElement
    fireEvent.contextMenu(bubble, { clientX: 10, clientY: 20 })
    const menu = useContextMenuStore.getState()
    expect(menu.isOpen).toBe(true)
    // No message box open for this Thread here: no Reply (prd-thread-message-menu-v1).
    expect(menu.items.filter((i) => i.type !== 'separator').map((i) => i.label)).toEqual(['Copy text', 'Save to stash'])
    menu.selectItem('stash')
    await waitFor(() => expect(usePromptStashStore.getState().items).toHaveLength(1))
    const rec = usePromptStashStore.getState().items[0]
    expect(rec.text).toBe('Try: refactor the parser')
    expect(rec.source).toMatchObject({ server_id: 'rosson.k2.dev', agent: 'k2', agent_name: 'k2', user: 'owner' })
    expect(h.daemonCliPost).not.toHaveBeenCalled()

    fireEvent.click(screen.getByRole('button', { name: 'Message menu' }))
    expect(useContextMenuStore.getState().isOpen).toBe(true)
  })

  it('my own message saves with the Thread as its source (prd-thread-message-menu-v1)', async () => {
    renderBubble(agentItem({ from: 'owner', via: 'compose', author: { kind: 'owner', login: 'owner' }, body: 'Ship the parser' }), { mine: true })
    fireEvent.click(screen.getByRole('button', { name: 'Message menu' }))
    useContextMenuStore.getState().selectItem('stash')
    await waitFor(() => expect(usePromptStashStore.getState().items).toHaveLength(1))
    const rec = usePromptStashStore.getState().items[0]
    expect(rec.text).toBe('Ship the parser')
    expect(rec.source).toMatchObject({ agent: 'k2', agent_name: null })
  })

  it('a person\'s post has the menu; a view with no server and a secret card get none', () => {
    renderBubble(agentItem({ author: { kind: 'user', login: 'ana' } }))
    expect(screen.getByRole('button', { name: 'Message menu' })).toBeTruthy()
    cleanup()
    renderBubble(agentItem({ kind: 'secret', body: 'Paste the key' }))
    expect(screen.queryByRole('button', { name: 'Message menu' })).toBeNull()
    cleanup()
    renderBubble(agentItem(), { source: false })
    expect(screen.queryByRole('button', { name: 'Message menu' })).toBeNull()
    fireEvent.contextMenu(document.querySelector('[data-zen-bubble]') as HTMLElement)
    expect(useContextMenuStore.getState().isOpen).toBe(false)
  })
})

describe('the stash.offer card', () => {
  it('coerces doc.stash: text required, title optional', () => {
    expect(coerceStashOffer({ text: 'hi', title: ' T ' })).toEqual({ text: 'hi', title: 'T' })
    expect(coerceStashOffer({ text: 'hi' })).toEqual({ text: 'hi' })
    expect(coerceStashOffer({ text: '  ' })).toBeNull()
    expect(coerceStashOffer({ title: 'x' })).toBeNull()
  })

  it('shows the offer and saves it on this device on tap; nothing goes to the daemon', async () => {
    renderBubble(
      agentItem({
        kind: 'stash.offer',
        body: 'Weekly report\n\nWrite the weekly report from the tickets board.',
        stash: { title: 'Weekly report', text: 'Write the weekly report from the tickets board.' },
      }),
    )
    const card = screen.getByTestId('stash-offer-card')
    expect(card.querySelector('[data-stash-offer-title]')?.textContent).toBe('Weekly report')
    expect(card.querySelector('[data-stash-offer-text]')?.textContent).toBe('Write the weekly report from the tickets board.')
    // The fallback body isn't drawn twice.
    expect(document.querySelector('[data-zen-bubble]')?.textContent).not.toContain('Weekly report\n\n')
    fireEvent.click(screen.getByTestId('stash-offer-save'))
    await waitFor(() => expect(screen.getByTestId('stash-offer-save').textContent).toBe('Saved'))
    const items = usePromptStashStore.getState().items
    // Field rules: title, blank line, text.
    expect(items.map((r) => [r.text, r.source.agent])).toEqual([['Weekly report\n\nWrite the weekly report from the tickets board.', 'k2']])
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('an old doc with no stash payload shows its body', () => {
    renderBubble(agentItem({ kind: 'stash.offer', body: 'Fallback text' }))
    expect(screen.queryByTestId('stash-offer-card')).toBeNull()
    expect(document.querySelector('[data-zen-bubble]')?.textContent).toContain('Fallback text')
  })
})

describe('"Save for later" in the Inbox "Chat about this" dialog', () => {
  it('stashes the note with the inbox stamp as a quote, closes, sends nothing', async () => {
    const onClose = vi.fn()
    renderInRoom(
      room,
      <InboxChatAboutDialog stamp={{ kind: 'tray', id: 'pkg-1', title: 'Build failed' }} projectPath="/ws/sales" workspaceId={null} onClose={onClose} />,
    )
    fireEvent.change(screen.getByTestId('inbox-chat-about-note'), { target: { value: 'look at this tomorrow' } })
    fireEvent.click(screen.getByTestId('inbox-chat-about-save-later'))
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
    const rec = usePromptStashStore.getState().items[0]
    expect(rec.text).toBe('look at this tomorrow')
    expect(rec.quotes).toEqual([{ text: '[k2 inboxID:pkg-1] Build failed\nOpen: k2 inbox read pkg-1', from: 'Inbox' }])
    expect(rec.source).toMatchObject({ server_id: 'rosson.k2.dev', agent: 'sales' })
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('Cmd+S in the note does the same (the Thread box\'s stash chord); empty, it does nothing', async () => {
    const onClose = vi.fn()
    renderInRoom(
      room,
      <InboxChatAboutDialog stamp={{ kind: 'tray', id: 'pkg-2', title: 'Deploy done' }} projectPath="/ws/sales" workspaceId={null} onClose={onClose} />,
    )
    const note = screen.getByTestId('inbox-chat-about-note')
    const mac = isMacPlatform()
    fireEvent.keyDown(note, { key: 's', metaKey: mac, ctrlKey: !mac })
    await new Promise((r) => setTimeout(r, 20))
    expect(onClose).not.toHaveBeenCalled()
    expect(usePromptStashStore.getState().items).toHaveLength(0)
    fireEvent.change(note, { target: { value: 'chord save' } })
    fireEvent.keyDown(note, { key: 's', metaKey: mac, ctrlKey: !mac })
    await waitFor(() => expect(onClose).toHaveBeenCalledTimes(1))
    expect(usePromptStashStore.getState().items[0].text).toBe('chord save')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })
})
