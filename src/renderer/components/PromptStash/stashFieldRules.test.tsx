// @vitest-environment jsdom
// Prompt stash field rules settled after the K2 Companion 4.1.4 review
// (spec-prompt-stash-v1 "Field rules settled after the 4.1.4 review"), one
// block per rule. A fake IndexedDB stands in for the webview's. Fail loud.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { act, cleanup, fireEvent, screen, waitFor } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => {
  const tunnel = { publicUrl: 'https://rosson.k2.dev' as string | null }
  const daemonCliGet = vi.fn(async (_scope: unknown, route: string): Promise<unknown> => {
    if (route === 'terminal/compose-history') return { items: [] }
    if (route === 'tunnel/status') return tunnel.publicUrl ? { public_url: tunnel.publicUrl, running: true } : { running: false }
    return {}
  })
  const daemonCliPost = vi.fn(async () => ({ ok: true }))
  return { tunnel, daemonCliGet, daemonCliPost }
})
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: h.daemonCliGet, daemonCliPost: h.daemonCliPost }))

import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { ThreadBubble } from '@/components/Thread/ThreadZenKit'
import { ThreadAttachmentSourceProvider } from '@/components/Thread/ThreadImageObjects'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { primaryScope } from '@/kessel/server-scope'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { resetStashServerIdsForTests, stashServerId, stashSourceFor, stashUserFor } from '@/lib/prompt-stash/compose'
import { closeStashDbForTests, openStashDb, putRawStashRowForTests, readRawStashRowForTests } from '@/lib/prompt-stash/idb'
import {
  newStashId,
  STASH_CAP,
  STASH_OFFER_TEXT_MAX,
  STASH_OFFER_TITLE_MAX,
  STASH_TOMBSTONE_TTL_MS,
  stashOfferView,
} from '@/lib/prompt-stash/record'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { usePromptStashStore } from '@/stores/prompt-stash'
import { useToastStore } from '@/stores/toast'
import { resetSavedOffersForTests, stashThreadText } from './StashFromThread'

const UUID_V4 = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/
const room = testRoom({ tabs: {}, scope: primaryScope(), projects: [] })

beforeEach(async () => {
  cleanup()
  await closeStashDbForTests()
  ;(globalThis as { indexedDB: IDBFactory | undefined }).indexedDB = new IDBFactory()
  localStorage.clear()
  h.tunnel.publicUrl = 'https://rosson.k2.dev'
  h.daemonCliPost.mockClear()
  resetStashServerIdsForTests()
  resetSavedOffersForTests()
  resetThreadViewersForTests()
  useToastStore.setState({ toasts: [] })
  usePromptStashStore.setState({ items: [], loaded: false, removeAfterUse: false })
})
afterEach(async () => {
  cleanup()
  vi.unstubAllGlobals()
  await closeStashDbForTests()
})

describe('source.server_id is the server\'s host key, the same on every device', () => {
  it('a saved server: its host key', async () => {
    expect(await stashServerId(fakeScope('box.k2.dev'))).toBe('box.k2.dev')
    expect(await stashServerId(fakeScope('192.168.1.20:38471'))).toBe('192.168.1.20:38471')
  })

  it('this computer: its Connect host, never `local`', async () => {
    const src = await stashSourceFor(primaryScope(), 'sales', 'Sales')
    expect(src.server_id).toBe('rosson.k2.dev')
    expect(src.server_label).toBe('rosson.k2.dev')
  })

  it('this computer with no tunnel: null, not `local` and not ""', async () => {
    h.tunnel.publicUrl = null
    const src = await stashSourceFor(primaryScope(), 'sales', 'Sales')
    expect(src.server_id).toBeNull()
    expect(src.server_label).toBeNull()
  })
})

describe('source.user is the login the server reports', () => {
  it('the owner: the linked username where known, else `owner`; a login: its username', () => {
    expect(stashUserFor({ principal: 'owner', aliases: ['rosson'] })).toBe('rosson')
    expect(stashUserFor({ principal: 'owner', aliases: [] })).toBe('owner')
    expect(stashUserFor({ principal: 'ana', aliases: [] })).toBe('ana')
    expect(stashUserFor(null)).toBeNull()
  })

  it('reads the viewer of that server', async () => {
    setThreadViewerForTests(primaryScope(), { principal: 'owner', aliases: ['rosson'] })
    expect((await stashSourceFor(primaryScope(), 'sales', null)).user).toBe('rosson')
  })
})

describe('source.agent is the sender\'s address, group chats too', () => {
  it('a group chat message saves its sender, not the chat', async () => {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'g1',
      doc: { id: 'g1', kind: 'text', from: 'ops/reviewer', body: 'Ship it after the soak', created_at: 1 },
    }
    renderInRoom(
      room,
      <ThreadAttachmentSourceProvider value={{ scope: primaryScope(), addr: 'launch-room' }}>
        <ThreadBubble item={item} mine={false} author="Reviewer" timeLabel="now" viewer={{ principal: 'owner', aliases: [] }} />
      </ThreadAttachmentSourceProvider>,
    )
    fireEvent.click(screen.getByRole('button', { name: 'Message menu' }))
    const { useContextMenuStore } = await import('@/stores/context-menu')
    useContextMenuStore.getState().selectItem('stash')
    await waitFor(() => expect(usePromptStashStore.getState().items).toHaveLength(1))
    expect(usePromptStashStore.getState().items[0].source).toMatchObject({ agent: 'ops/reviewer', agent_name: 'Reviewer' })
  })

  it('no sender: null', async () => {
    expect(await stashThreadText({ scope: null, text: 'x', agent: '', agentName: '' })).toBe(true)
    expect(usePromptStashStore.getState().items[0].source).toEqual({ server_id: null, server_label: null, user: null, agent: null, agent_name: null })
  })
})

describe('stash.offer saves title, blank line, text', () => {
  it('with a title', () => {
    expect(stashOfferView({ title: 'Weekly report', text: 'Write it.' }).saved).toBe('Weekly report\n\nWrite it.')
  })
  it('without a title: the text alone', () => {
    expect(stashOfferView({ text: 'Write it.' }).saved).toBe('Write it.')
    expect(stashOfferView({ text: 'Write it.', title: '   ' }).saved).toBe('Write it.')
  })
})

describe('unknown `v`: kept as is, never rewritten, never shown', () => {
  const future = {
    v: 2,
    id: '6f1c2c1e-7d1f-4c4e-9a7e-2f3b8a1d0c55',
    text: 'from a newer client',
    blocks: [{ kind: 'rich', value: 1 }],
    source: { server_id: 'box.k2.dev' },
    created_at: 1,
    updated_at: 1,
    deleted_at: 1,
  }

  it('is hidden, survives saves, deletes and the tombstone sweep untouched, and isn\'t counted', async () => {
    await putRawStashRowForTests(future)
    const s = usePromptStashStore.getState()
    await s.load()
    expect(usePromptStashStore.getState().items).toEqual([])
    const r = await s.save({ text: 'mine', source: {} }, [], { now: 1 + STASH_TOMBSTONE_TTL_MS * 2 })
    if (r.status !== 'saved') throw new Error('not saved')
    await s.remove(r.record.id)
    await s.save({ text: 'again', source: {} }, [], { now: 10 + STASH_TOMBSTONE_TTL_MS * 4 })
    expect(usePromptStashStore.getState().items.map((x) => x.text)).toEqual(['again'])
    expect(await readRawStashRowForTests(future.id)).toEqual(future)
    // The cap counts our live items only.
    for (let i = 0; i < STASH_CAP - 1; i++) {
      const ok = await s.save({ text: `p${i}`, source: {} })
      if (ok.status !== 'saved') throw new Error(`save ${i} refused`)
    }
    expect(usePromptStashStore.getState().items).toHaveLength(STASH_CAP)
    expect(await readRawStashRowForTests(future.id)).toEqual(future)
  })
})

describe('absent optional fields are null, never ""', () => {
  it('source fields and a quote\'s `from`', async () => {
    const r = await usePromptStashStore.getState().save({
      text: 'x',
      quotes: [{ text: 'q' }, { text: 'r', from: '' }],
      source: { server_id: 'box.k2.dev', server_label: '', user: '', agent: 'sales' },
    })
    if (r.status !== 'saved') throw new Error('not saved')
    const raw = (await readRawStashRowForTests(r.record.id)) as Record<string, unknown>
    expect(raw.source).toEqual({ server_id: 'box.k2.dev', server_label: null, user: null, agent: 'sales', agent_name: null })
    expect(raw.quotes).toEqual([{ text: 'q', from: null }, { text: 'r', from: null }])
    expect(raw.deleted_at).toBeNull()
    expect(JSON.stringify(raw)).not.toContain('""')
  })
})

describe('ids are uuid v4', () => {
  it('from crypto.randomUUID', async () => {
    const r = await usePromptStashStore.getState().save({ text: 'x', source: {} }, [{ name: 'a', type: '', bytes: new Blob(['a']) }])
    if (r.status !== 'saved') throw new Error('not saved')
    expect(r.record.id).toMatch(UUID_V4)
    expect(r.record.attachments[0].id).toMatch(UUID_V4)
  })

  it('the fallback without randomUUID is still v4', () => {
    const real = globalThis.crypto
    vi.stubGlobal('crypto', { getRandomValues: (b: Uint8Array<ArrayBuffer>) => real.getRandomValues(b) })
    const ids = Array.from({ length: 50 }, () => newStashId())
    for (const id of ids) expect(id).toMatch(UUID_V4)
    expect(new Set(ids).size).toBe(50)
  })
})

describe('offer limits: title ≤ 200, text ≤ 20,000, saved = shown, one tap one save', () => {
  it('cuts long parts with an ellipsis, never splitting a character, and says so', () => {
    const v = stashOfferView({ title: 'T'.repeat(250), text: `${'x'.repeat(STASH_OFFER_TEXT_MAX - 1)}😀😀` })
    expect(Array.from(v.title ?? '').length).toBe(STASH_OFFER_TITLE_MAX)
    expect(v.title?.endsWith('…')).toBe(true)
    expect(Array.from(v.text).length).toBe(STASH_OFFER_TEXT_MAX)
    expect(v.text.endsWith('…')).toBe(true)
    expect(v.text).not.toMatch(/[\uD800-\uDBFF]…$/)
    expect(v.truncated).toBe(true)
    expect(v.saved).toBe(`${v.title}\n\n${v.text}`)
    const short = stashOfferView({ title: 'ok', text: 'fine' })
    expect(short.truncated).toBe(false)
  })

  function renderOffer(stash: { text: string; title?: string }): void {
    const item: OverlayThreadItem = {
      collection: 'thread',
      seq: 1,
      id: 'offer-1',
      doc: { id: 'offer-1', kind: 'stash.offer', from: 'k2', body: 'fallback', stash, created_at: 1 },
    }
    renderInRoom(
      room,
      <ThreadAttachmentSourceProvider value={{ scope: primaryScope(), addr: 'k2' }}>
        <ThreadBubble item={item} mine={false} author="k2" timeLabel="now" viewer={{ principal: 'owner', aliases: [] }} />
      </ThreadAttachmentSourceProvider>,
    )
  }

  it('the card shows the cut parts and a note; the saved text is exactly what it shows', async () => {
    renderOffer({ title: 'T'.repeat(300), text: 'y'.repeat(STASH_OFFER_TEXT_MAX + 50) })
    const card = screen.getByTestId('stash-offer-card')
    const shownTitle = card.querySelector('[data-stash-offer-title]')?.textContent ?? ''
    const shownText = card.querySelector('[data-stash-offer-text]')?.textContent ?? ''
    expect(shownTitle.length).toBe(STASH_OFFER_TITLE_MAX)
    expect(shownText.length).toBe(STASH_OFFER_TEXT_MAX)
    expect(card.querySelector('[data-stash-offer-truncated]')).not.toBeNull()
    fireEvent.click(screen.getByTestId('stash-offer-save'))
    await waitFor(() => expect(usePromptStashStore.getState().items).toHaveLength(1))
    expect(usePromptStashStore.getState().items[0].text).toBe(`${shownTitle}\n\n${shownText}`)
  })

  it('two quick taps save one copy; the button then says "Saved", also when drawn again', async () => {
    renderOffer({ title: 'Plan', text: 'Draft the plan.' })
    const btn = screen.getByTestId('stash-offer-save')
    fireEvent.click(btn)
    fireEvent.click(btn)
    await waitFor(() => expect(screen.getByTestId('stash-offer-save').textContent).toBe('Saved'))
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(usePromptStashStore.getState().items).toHaveLength(1)
    cleanup()
    renderOffer({ title: 'Plan', text: 'Draft the plan.' })
    expect(screen.getByTestId('stash-offer-save').textContent).toBe('Saved')
    expect((screen.getByTestId('stash-offer-save') as HTMLButtonElement).disabled).toBe(true)
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })
})

describe('IndexedDB failure: an error, the draft stays; a dropped connection reopens', () => {
  const chrome: SessionViewChromeValue = {
    viewTab: 'thread',
    splitLeft: 'terminal',
    splitRight: 'thread',
    overlayAddr: 'sales',
    conversationId: 'c',
    agentName: 'sales',
    displayName: 'Sales',
  }

  function renderBar(): HTMLTextAreaElement {
    renderInRoom(
      room,
      <SessionViewChromeContext.Provider value={chrome}>
        <TerminalComposeBar sessionId="sess-f" workspacePath="/ws/sales" sendDestination="thread" />
      </SessionViewChromeContext.Provider>,
    )
    return screen.getByRole('textbox') as HTMLTextAreaElement
  }

  it('no IndexedDB: Cmd+S shows an error and the box keeps the draft', async () => {
    await closeStashDbForTests()
    ;(globalThis as { indexedDB: IDBFactory | undefined }).indexedDB = undefined
    const box = renderBar()
    fireEvent.change(box, { target: { value: 'do not lose me' } })
    const mac = isMacPlatform()
    fireEvent.keyDown(box, { key: 's', metaKey: mac, ctrlKey: !mac })
    await waitFor(() =>
      expect(useToastStore.getState().toasts.some((t) => t.type === 'error' && t.message.startsWith("Couldn't stash:"))).toBe(true),
    )
    expect(box.value).toBe('do not lose me')
  })

  it('a dropped connection is opened again and the save lands', async () => {
    const s = usePromptStashStore.getState()
    const first = await s.save({ text: 'before', source: {} }, [], { now: 1_000 })
    if (first.status !== 'saved') throw new Error('not saved')
    // The webview drops the connection under us.
    ;(await openStashDb()).close()
    const second = await s.save({ text: 'after', source: {} }, [], { now: 2_000 })
    expect(second.status).toBe('saved')
    expect(usePromptStashStore.getState().items.map((x) => x.text)).toEqual(['after', 'before'])
  })
})
