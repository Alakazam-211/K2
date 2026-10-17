// @vitest-environment jsdom
// The Design tab's pane (prd-browser-design-mode-v1 D-A6–D-A12), fail loud:
//  - the page is a design view (`browser_create { design: true }`); a plain
//    Browser pane never sends the flag;
//  - its own Chat | page split (a design slot), the "Chat is open beside"
//    rule, and the split width saved per window per room;
//  - Select on when the tab opens; the chrome-bar palette toggles it
//    (accent while on); pick → the note pop-up (an overlay claim, which
//    freezes the page) → tray → badges → one Send to `browser/notes`;
//    ⌘↩ in the pop-up sends the typed note plus the tray at once;
//  - closing the tab drops its notes; a workspace switch does not.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, screen } from '@testing-library/react'

const h = vi.hoisted(() => ({ retainedId: { value: null as string | null } }))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => {}),
  listen: vi.fn(async () => () => {}),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'main' }) }))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
      if (route === 'browser/notes') {
        return { ok: true, threadItemId: 'th-1', delivered: true, targetSessionId: 's-1', addr: 'docs', notes: 1 }
      }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})
// The retained-Chat gate needs the daemon capability + the Active set; the
// slot itself is the real store wiring's job (PinnedChatRetainer tests).
vi.mock('@/components/AgentPane/PinnedChatRetainer', async () => {
  const { useLayoutEffect, useRef } = await import('react')
  const { useRetainedChatStore } = await import('@/stores/retained-chat')
  return {
    useRetainedChatProjectId: () => h.retainedId.value,
    PinnedChatSlot: (props: { projectId: string; kind?: 'chat' | 'design'; visible?: boolean }) => {
      const ref = useRef<HTMLDivElement | null>(null)
      useLayoutEffect(() => {
        const el = ref.current
        if (!el) return
        useRetainedChatStore.getState().registerSlot(props.projectId, el, props.kind)
        return () => useRetainedChatStore.getState().unregisterSlot(props.projectId, el)
      }, [props.projectId, props.kind])
      useLayoutEffect(() => {
        const el = ref.current
        if (el) useRetainedChatStore.getState().setSlotVisible(props.projectId, props.visible ?? true, el)
      }, [props.projectId, props.visible])
      return <div ref={ref} data-slot-kind={props.kind} data-slot-visible={String(props.visible)} />
    },
  }
})

import { invoke } from '@tauri-apps/api/core'
import { daemonCliPost } from '@/lib/daemon-cli'
import { BrowserPane } from '@/components/BrowserPane/BrowserPane'
import { overlayClaims, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'
import { resetRetainedChatStore, useRetainedChatStore } from '@/stores/retained-chat'
import { useTabsStore, type Tab } from '@/stores/tabs'
import { renderInPrimaryRoom } from '@/test-utils/primary-room'
import { DesignPane } from './DesignPane'
import { designSplitKey } from './design-split-sizes'
import { useDesignNotesStore } from './useDesignNotes'
import type { DesignPickerIpc, PickPoll, PickResult } from './types'

class ResizeObserverStub {
  observe(): void {}
  unobserve(): void {}
  disconnect(): void {}
}

const PICK: PickResult = {
  type: 'pick',
  pickId: 'p1',
  url: 'http://localhost:5173/pricing',
  title: 'Pricing',
  viewport: { width: 640, height: 480, scrollX: 0, scrollY: 0, dpr: 2 },
  element: { selector: 'button.cta', tag: 'button', classes: ['cta'], text: 'Buy', html: '<button class="cta">Buy</button>', rect: { x: 10, y: 20, width: 80, height: 30 } },
}

function fakeIpc(polls: PickPoll[]): DesignPickerIpc & { calls: string[]; badges: unknown[] } {
  const calls: string[] = []
  const badges: unknown[] = []
  return {
    calls,
    badges,
    pickArm: vi.fn(async (id: string) => {
      calls.push(`arm:${id}`)
    }),
    pickDisarm: vi.fn(async (id: string) => {
      calls.push(`disarm:${id}`)
    }),
    pickPoll: vi.fn(async () => polls.shift() ?? null),
    setDesignBadges: vi.fn(async (_id: string, _parent: string | undefined, b: unknown) => {
      badges.push(b)
    }),
  }
}

/** A room strip holding a Chat system tab and one Design tab. */
function seedTabs(): { tab: Tab; itemId: string; pgId: string } {
  const chatPg = 'pg-chat'
  const chat: Tab = {
    id: 'tab-chat',
    title: 'Chat',
    mosaicTree: chatPg,
    isSystemAgent: true,
    paneGroups: new Map([[chatPg, { id: chatPg, activeItemIndex: 0, items: [{ id: 'i-chat', type: 'agent', data: { agentName: 'docs', projectPath: '/w/docs', section: 'chat' } }] }]]),
  }
  const pgId = 'pg-design'
  const tab: Tab = {
    id: 'tab-design',
    title: 'Design',
    mosaicTree: pgId,
    paneGroups: new Map([[pgId, { id: pgId, activeItemIndex: 0, items: [{ id: 'i-design', type: 'design', data: { url: 'http://localhost:5173/pricing' } }] }]]),
  }
  useTabsStore.setState({ tabs: [chat, tab], activeTabId: tab.id, splitCount: 1, extraGroups: [], backgroundWorkspaces: {} })
  return { tab, itemId: 'i-design', pgId }
}

function renderDesign(ipc: DesignPickerIpc): ReturnType<typeof renderInPrimaryRoom> {
  const { tab, itemId, pgId } = seedTabs()
  return renderInPrimaryRoom(
    <DesignPane itemId={itemId} tabId={tab.id} paneGroupId={pgId} data={{ url: 'http://localhost:5173/pricing' }} ipc={ipc} />,
  )
}

async function flush(): Promise<void> {
  await act(async () => {
    for (let i = 0; i < 5; i++) await Promise.resolve()
  })
}

beforeEach(() => {
  vi.mocked(invoke).mockReset()
  vi.mocked(invoke).mockImplementation(async () => null)
  vi.mocked(daemonCliPost).mockClear()
  globalThis.ResizeObserver = ResizeObserverStub as unknown as typeof ResizeObserver
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    x: 0, y: 32, width: 640, height: 480, top: 32, left: 0, right: 640, bottom: 512, toJSON: () => ({}),
  } as DOMRect)
  resetBrowserOverlaysForTest()
  resetRetainedChatStore()
  useDesignNotesStore.setState({ trays: {} })
  h.retainedId.value = null
  localStorage.clear()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  vi.restoreAllMocks()
  useTabsStore.setState({ tabs: [], activeTabId: null, extraGroups: [], backgroundWorkspaces: {} })
})

describe('the page is a design view', () => {
  it('Design creates its native view with design: true; a plain Browser pane never does', async () => {
    renderDesign(fakeIpc([]))
    await flush()
    const creates = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'browser_create')
    expect(creates).toHaveLength(1)
    expect(creates[0][1]).toMatchObject({ itemId: 'i-design', url: 'http://localhost:5173/pricing', design: true })
    cleanup()

    vi.mocked(invoke).mockClear()
    renderInPrimaryRoom(<BrowserPane itemId="b1" tabId="t" paneGroupId="pg" url="https://example.com" />)
    await flush()
    const plain = vi.mocked(invoke).mock.calls.filter((c) => c[0] === 'browser_create')
    expect(plain).toHaveLength(1)
    expect(plain[0][1]).not.toHaveProperty('design')
  })

  it('shows the toolbar above the page and no Chat column when the Chat is not retained', async () => {
    renderDesign(fakeIpc([]))
    await flush()
    expect(screen.getByTestId('design-toolbar')).toBeTruthy()
    expect(document.querySelector('[data-design-chat]')).toBeNull()
    expect(document.querySelector('[data-design-split]')).toBeNull()
  })
})

describe('Chat | page split (DD5, DD6, D-A7)', () => {
  it('draws the retained Chat in a design slot beside the page', async () => {
    h.retainedId.value = 'p-docs'
    renderDesign(fakeIpc([]))
    await flush()
    const col = document.querySelector('[data-design-chat]') as HTMLElement | null
    if (!col) throw new Error('no Chat column')
    expect(col.style.display).toBe('block')
    expect(col.querySelector('[data-slot-kind="design"]')?.getAttribute('data-slot-visible')).toBe('true')
    // The design slot holds the instance (no Chat tab slot is visible).
    expect(useRetainedChatStore.getState().slots.get('p-docs')?.el).toBe(col.firstElementChild)
  })

  it('the Chat tab visible beside it wins: the page goes full width with a note', async () => {
    h.retainedId.value = 'p-docs'
    renderDesign(fakeIpc([]))
    await flush()
    const chatEl = document.createElement('div')
    act(() => {
      useRetainedChatStore.getState().registerSlot('p-docs', chatEl, 'chat')
      useRetainedChatStore.getState().setSlotVisible('p-docs', true, chatEl)
    })
    const col = document.querySelector('[data-design-chat]') as HTMLElement
    expect(col.style.display).toBe('none')
    expect(screen.getByTestId('design-chat-beside').textContent).toBe('Chat is open beside')
    expect(useRetainedChatStore.getState().slots.get('p-docs')?.el).toBe(chatEl)
  })

  it('dragging the split saves the width for this window and room', async () => {
    h.retainedId.value = 'p-docs'
    renderDesign(fakeIpc([]))
    await flush()
    const sep = document.querySelector('[data-design-split]')
    if (!sep) throw new Error('no split handle')
    act(() => {
      fireEvent.mouseDown(sep, { clientX: 224 })
    })
    // The drag claims the window so the native page freezes, not eats the mouse.
    expect(overlayClaims().some((c) => c.kind === 'drag')).toBe(true)
    act(() => {
      fireEvent.mouseMove(window, { clientX: 320 })
      fireEvent.mouseUp(window)
    })
    expect(overlayClaims().some((c) => c.kind === 'drag')).toBe(false)
    const saved = JSON.parse(localStorage.getItem(designSplitKey('main')) ?? 'null') as Record<string, number[]> | null
    if (!saved) throw new Error('split not saved')
    expect(Object.values(saved)).toEqual([[50, 50]])
    // Nothing shared moved.
    expect(useTabsStore.getState().splitCount).toBe(1)
    expect(useTabsStore.getState().extraGroups).toEqual([])
  })
})

describe('the palette is the Select toggle', () => {
  it('sits in the chrome bar after Back / Forward, before the URL, on and accent-coloured at open', async () => {
    const ipc = fakeIpc([])
    renderDesign(ipc)
    await flush()
    const palette = screen.getByRole('button', { name: 'Select an element' })
    expect(palette.getAttribute('aria-pressed')).toBe('true')
    expect(palette.className).toContain('text-[var(--color-accent)]')
    expect(ipc.calls).toContain('arm:i-design')
    // Order in the chrome bar: Back, Forward, palette, then the URL field.
    const forward = screen.getByRole('button', { name: 'Forward' })
    const url = document.querySelector('[data-browser-address]') as HTMLElement
    expect(forward.compareDocumentPosition(palette) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(palette.compareDocumentPosition(url) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // No Select button, no S hint anywhere in the pane.
    expect(screen.queryByRole('button', { name: /^select$/i })).toBeNull()
    expect(document.querySelector('kbd')).toBeNull()

    act(() => {
      fireEvent.click(palette)
    })
    await flush()
    expect(palette.getAttribute('aria-pressed')).toBe('false')
    expect(palette.className).not.toContain('text-[var(--color-accent)]')
    expect(ipc.calls.at(-1)).toBe('disarm:i-design')
    act(() => {
      fireEvent.click(palette)
    })
    await flush()
    expect(palette.getAttribute('aria-pressed')).toBe('true')
    expect(ipc.calls.at(-1)).toBe('arm:i-design')
  })
})

describe('pick → note → tray → Send', () => {
  it('⌘↩ in the pop-up sends the typed note plus the tray in one post', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] })
    const res = useDesignNotesStore.getState().add('i-design', { comment: 'pinned earlier', url: 'http://localhost:5173/pricing', title: 'Pricing' })
    if (!res.ok) throw new Error(res.reason)
    renderDesign(fakeIpc([{ events: [PICK], navSeq: 1 }]))
    await flush()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(450)
    })
    const field = screen.getByRole('dialog', { name: 'Note for the agent' }).querySelector('textarea') as HTMLTextAreaElement
    act(() => {
      fireEvent.change(field, { target: { value: 'bigger' } })
    })
    await act(async () => {
      fireEvent.keyDown(field, { key: 'Enter', metaKey: true })
    })
    await flush()
    expect(screen.queryByRole('dialog', { name: 'Note for the agent' })).toBeNull()
    const posts = vi.mocked(daemonCliPost).mock.calls.filter((c) => c[1] === 'browser/notes')
    expect(posts).toHaveLength(1)
    expect((posts[0][2] as { notes: { comment: string }[] }).notes.map((n) => n.comment)).toEqual(['pinned earlier', 'bigger'])
    expect(useDesignNotesStore.getState().trays['i-design'].notes).toEqual([])
  })


  it('a pick opens the pop-up (an overlay claim), Add pins a note, badges follow, Send posts once', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout', 'setInterval', 'clearInterval'] })
    const ipc = fakeIpc([{ events: [PICK], navSeq: 1 }])
    renderDesign(ipc)
    await flush()

    await act(async () => {
      await vi.advanceTimersByTimeAsync(450)
    })
    const dialog = screen.getByRole('dialog', { name: 'Note for the agent' })
    expect(dialog).toBeTruthy()
    expect(screen.getByTestId('design-element-chip').textContent).toBe('<button.cta>')
    expect(overlayClaims().some((c) => c.kind === 'popover')).toBe(true)

    const field = dialog.querySelector('textarea') as HTMLTextAreaElement
    act(() => {
      fireEvent.change(field, { target: { value: 'make this primary' } })
    })
    act(() => {
      fireEvent.click(screen.getByRole('button', { name: 'Add' }))
    })
    expect(screen.queryByRole('dialog', { name: 'Note for the agent' })).toBeNull()
    expect(overlayClaims().some((c) => c.kind === 'popover')).toBe(false)
    const tray = useDesignNotesStore.getState().trays['i-design']
    expect(tray.notes.map((n) => n.comment)).toEqual(['make this primary'])
    await flush()
    expect(ipc.badges.at(-1)).toEqual([{ n: 1, selector: 'button.cta' }])
    // Select comes back after Add (pin many, send once).
    expect(ipc.calls.at(-1)).toBe('arm:i-design')

    await act(async () => {
      fireEvent.click(screen.getAllByRole('button', { name: 'Send' })[0])
    })
    await flush()
    const posts = vi.mocked(daemonCliPost).mock.calls.filter((c) => c[1] === 'browser/notes')
    expect(posts).toHaveLength(1)
    expect(posts[0][2]).toMatchObject({
      workspace: '/w/docs',
      page: { url: 'http://localhost:5173/pricing', title: 'Pricing' },
      notes: [{ comment: 'make this primary', intent: 'change', payload: { selector: 'button.cta' } }],
      itemId: 'i-design',
    })
    expect(useDesignNotesStore.getState().trays['i-design'].notes).toEqual([])
  })
})

describe('notes lifecycle', () => {
  function addNote(): void {
    const res = useDesignNotesStore.getState().add('i-design', { comment: 'x', url: 'http://localhost:5173/', title: 'T' })
    if (!res.ok) throw new Error(res.reason)
  }

  it('closing the tab drops its notes', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const view = renderDesign(fakeIpc([]))
    addNote()
    act(() => {
      useTabsStore.setState({ tabs: useTabsStore.getState().tabs.filter((t) => t.id !== 'tab-design') })
    })
    view.unmount()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
    })
    expect(useDesignNotesStore.getState().trays['i-design']).toBeUndefined()
  })

  it('a workspace switch (the tab parked in the background) keeps them', async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    const view = renderDesign(fakeIpc([]))
    addNote()
    act(() => {
      const s = useTabsStore.getState()
      useTabsStore.setState({
        tabs: [],
        backgroundWorkspaces: { 'p:w': { tabs: s.tabs, extraGroups: [], splitCount: 1, activeGroupIndex: 0, activeTabId: null } },
      })
    })
    view.unmount()
    await act(async () => {
      await vi.advanceTimersByTimeAsync(1)
    })
    expect(useDesignNotesStore.getState().trays['i-design'].notes).toHaveLength(1)
  })
})
