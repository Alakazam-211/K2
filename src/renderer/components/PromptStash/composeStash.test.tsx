// @vitest-environment jsdom
// Prompt stash in the composers (spec-prompt-stash-v1 Acceptance 1–7): Cmd+S
// saves then clears, Cmd+S on an empty box opens the drawer, the button is
// always there, the activity bar slides away while the drawer is open and
// comes back, a pick goes below existing text, and a server switch keeps
// the list. A fake IndexedDB stands in for the webview's. Fail loud.
import 'fake-indexeddb/auto'
import { IDBFactory } from 'fake-indexeddb'
import { act, cleanup, fireEvent, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ServerScope } from '@/kessel/server-scope'

const h = vi.hoisted(() => {
  const daemonCliGet = vi.fn(async (_scope: unknown, route: string) => {
    if (route === 'terminal/compose-history') return { items: [] }
    // This computer's Connect host: its stable `server_id`.
    if (route === 'tunnel/status') return { public_url: 'https://rosson.k2.dev' }
    return []
  })
  const daemonCliPost = vi.fn(async () => ({ ok: true }))
  return { daemonCliGet, daemonCliPost }
})

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
}))

import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import { ThreadComposeDrawerContext } from '@/components/SessionView/threadComposeDrawer'
import { coerceThreadTurn, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { closeStashDbForTests } from '@/lib/prompt-stash/idb'
import { resetStashServerIdsForTests } from '@/lib/prompt-stash/compose'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { primaryScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { STASH_CAP } from '@/lib/prompt-stash/record'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useToastStore } from '@/stores/toast'
import { usePromptStashStore, type StashSaveResult } from '@/stores/prompt-stash'
import { useContextMenuStore } from '@/stores/context-menu'

const chrome: SessionViewChromeValue = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: 'c',
  agentName: 'sales',
  displayName: 'Sales',
}

function turn(): ThreadTurn {
  const now = Date.now()
  const t = coerceThreadTurn(
    { turnId: 't1', state: 'working', phase: 'tool', phaseSince: now, line: 'Reading `a.rs`', startedAt: now - 5_000, subagents: 0, background: 0, tally: { read: 1 }, end: null, serverNow: now, rev: 1 },
    now,
  )
  if (!t) throw new Error('fixture is not a turn')
  return t
}

function renderThreadBar(scope: ServerScope = primaryScope(), withTurn = false): void {
  const room = testRoom({ tabs: {}, scope, isPrimary: scope === primaryScope(), projects: [] })
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <ThreadComposeDrawerContext.Provider value={withTurn ? { turn: turn() } : null}>
        <TerminalComposeBar sessionId="sess-1" workspacePath="/ws/sales" sendDestination="thread" />
      </ThreadComposeDrawerContext.Provider>
    </SessionViewChromeContext.Provider>,
  )
}

function renderPtyBar(): void {
  const room = testRoom({ tabs: {}, scope: primaryScope(), projects: [] })
  renderInRoom(room, <TerminalComposeBar sessionId="sess-2" workspacePath="/ws/sales" sendDestination="pty" />)
}

function box(): HTMLTextAreaElement {
  return screen.getByRole('textbox') as HTMLTextAreaElement
}

function type(text: string): void {
  fireEvent.change(box(), { target: { value: text } })
}

function pressStash(target: Element = box()): void {
  const mac = isMacPlatform()
  fireEvent.keyDown(target, { key: 's', metaKey: mac, ctrlKey: !mac })
}

function activityPhase(): string | null {
  return document.querySelector('[data-zen-drawer]')?.getAttribute('data-phase') ?? null
}

async function reloadStore(): Promise<void> {
  await closeStashDbForTests()
  usePromptStashStore.setState({ items: [], loaded: false })
  await usePromptStashStore.getState().load()
}

const realSave = usePromptStashStore.getState().save

beforeEach(async () => {
  cleanup()
  await closeStashDbForTests()
  ;(globalThis as { indexedDB: IDBFactory }).indexedDB = new IDBFactory()
  localStorage.clear()
  resetStashServerIdsForTests()
  usePromptStashStore.setState({ items: [], loaded: false, removeAfterUse: false, save: realSave })
})
afterEach(async () => {
  cleanup()
  await closeStashDbForTests()
})

describe('Cmd+S with text', () => {
  it('writes first, then clears; the draft survives a reload', async () => {
    let release: () => void = () => {}
    const gate = new Promise<void>((r) => {
      release = r
    })
    const save = vi.fn(async (...args: Parameters<typeof realSave>): Promise<StashSaveResult> => {
      await gate
      return realSave(...args)
    })
    usePromptStashStore.setState({ save })
    renderThreadBar()
    type('remember this prompt')
    pressStash()
    await waitFor(() => expect(save).toHaveBeenCalledTimes(1))
    // The write hasn't landed: the box still holds the draft.
    expect(box().value).toBe('remember this prompt')
    await act(async () => {
      release()
    })
    await waitFor(() => expect(box().value).toBe(''))
    expect(save.mock.calls[0][0]).toMatchObject({
      text: 'remember this prompt',
      source: { server_id: 'rosson.k2.dev', server_label: 'rosson.k2.dev', agent: 'sales', agent_name: 'Sales', user: 'owner' },
    })
    await reloadStore()
    expect(usePromptStashStore.getState().items.map((r) => r.text)).toEqual(['remember this prompt'])
  })

  it('a failed write keeps the draft in the box', async () => {
    usePromptStashStore.setState({
      save: vi.fn(async () => {
        throw new Error('disk full')
      }),
    })
    renderThreadBar()
    type('keep me')
    pressStash()
    await waitFor(() =>
      expect(useToastStore.getState().toasts.map((t) => t.message)).toContain("Couldn't stash: disk full. Your draft is still here."),
    )
    expect(box().value).toBe('keep me')
  })

  it('works in the terminal compose bar too', async () => {
    renderPtyBar()
    type('pty draft')
    pressStash()
    await waitFor(() => expect(box().value).toBe(''))
    expect(usePromptStashStore.getState().items.map((r) => r.text)).toEqual(['pty draft'])
    expect(screen.getByTestId('stash-button').getAttribute('aria-label')).toBe('Stash (1 item)')
  })

  it('at the cap it asks first, and a no keeps the draft and the list', async () => {
    for (let i = 0; i < STASH_CAP; i++) await realSave({ text: `p${i}`, source: { server_id: 'local', server_label: '', user: '', agent: '', agent_name: '' } }, [], { now: 1_000 + i })
    renderThreadBar()
    type('the 51st')
    pressStash()
    await waitFor(() => expect(useConfirmDialogStore.getState().isOpen).toBe(true))
    expect(useConfirmDialogStore.getState().message).toBe('Your stash is full (50). Remove the oldest?')
    act(() => useConfirmDialogStore.getState().close())
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(box().value).toBe('the 51st')
    expect(usePromptStashStore.getState().items).toHaveLength(STASH_CAP)
    expect(usePromptStashStore.getState().items.some((r) => r.text === 'p0')).toBe(true)
  })
})

describe('"Save to stash" in the "+" menu', () => {
  function openPlus(): void {
    act(() => {
      fireEvent.click(screen.getByTestId('compose-plus'))
    })
    if (!useContextMenuStore.getState().isOpen) throw new Error('"+" did not open its menu')
  }
  function row(id: string) {
    const r = useContextMenuStore.getState().items.find((i) => i.id === id)
    if (!r) throw new Error(`the "+" menu has no ${id} row`)
    return r
  }
  afterEach(() => {
    act(() => useContextMenuStore.getState().close())
  })

  it.each([
    ['the Thread box', renderThreadBar],
    ['the terminal compose bar', renderPtyBar],
  ])('in %s: attach first, then Save to stash, dimmed on an empty box', (_where, renderBar) => {
    renderBar()
    openPlus()
    const items = useContextMenuStore.getState().items
    expect(items.map((i) => i.id)).toEqual(['attach', 'sep', 'stash'])
    expect(row('stash').label).toBe('Save to stash')
    expect(row('stash').enabled).toBe(false)
    expect(row('stash').shortcut).toBe(isMacPlatform() ? '⌘S' : 'Ctrl+S')
    expect(screen.getByTestId('compose-plus').getAttribute('aria-expanded')).toBe('true')
  })

  it('with text it is enabled, and picking it saves, then clears the box', async () => {
    renderThreadBar()
    type('stash me from the menu')
    openPlus()
    expect(row('stash').enabled).toBe(true)
    act(() => useContextMenuStore.getState().selectItem('stash'))
    await waitFor(() => expect(box().value).toBe(''))
    expect(usePromptStashStore.getState().items.map((r) => r.text)).toEqual(['stash me from the menu'])
    expect(screen.getByTestId('stash-button').getAttribute('aria-label')).toBe('Stash (1 item)')
    expect(screen.getByTestId('compose-plus').getAttribute('aria-expanded')).toBe('false')
  })

  it('in the terminal compose bar it saves and clears too', async () => {
    renderPtyBar()
    type('pty from the menu')
    openPlus()
    act(() => useContextMenuStore.getState().selectItem('stash'))
    await waitFor(() => expect(box().value).toBe(''))
    expect(usePromptStashStore.getState().items.map((r) => r.text)).toEqual(['pty from the menu'])
  })

  it('a dimmed row can\'t be picked: nothing is saved', async () => {
    renderThreadBar()
    openPlus()
    act(() => useContextMenuStore.getState().selectItem('stash'))
    expect(useContextMenuStore.getState().isOpen).toBe(true)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(usePromptStashStore.getState().items).toEqual([])
  })

  it('the stash tab on the box\'s top edge is 44 px wide', () => {
    renderThreadBar()
    expect(screen.getByTestId('stash-button').style.width).toBe('44px')
  })

  it('the stash tab shows the icon, no count (the label still says how many); closed, it keeps the bookmark', async () => {
    renderThreadBar()
    type('one to count')
    pressStash()
    await waitFor(() => expect(box().value).toBe(''))
    const tab = screen.getByTestId('stash-button')
    expect(tab.getAttribute('aria-label')).toBe('Stash (1 item)')
    expect(tab.querySelector('[data-stash-count]')).toBeNull()
    expect(tab.querySelector('[data-stash-icon] svg')).not.toBeNull()
    expect(tab.textContent).toBe('')
  })
})

describe('the button and the drawer', () => {
  it('the button is always there, with no activity bar', async () => {
    renderThreadBar()
    const btn = screen.getByTestId('stash-button')
    expect(btn.getAttribute('aria-label')).toBe('Stash (0 items)')
    expect(document.querySelector('[data-zen-drawer]')).toBeNull()
  })

  it('sits just right of the activity bar on the box\'s top edge', () => {
    renderThreadBar(primaryScope(), true)
    const row = document.querySelector('[data-zen-stash-row]') as HTMLElement
    const activity = row.querySelector('[data-zen-drawer]') as HTMLElement
    const btn = within(row).getByTestId('stash-button')
    expect(activity.compareDocumentPosition(btn) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
    const field = document.querySelector('[data-zen-compose-box]') as HTMLElement
    expect(row.compareDocumentPosition(field) & Node.DOCUMENT_POSITION_FOLLOWING).toBe(Node.DOCUMENT_POSITION_FOLLOWING)
  })

  it('Cmd+S on an empty box opens the drawer, newest first, with source, age and files', async () => {
    await realSave({ text: 'older', source: { server_id: 'local', server_label: 'This computer', user: 'owner', agent: 'sales', agent_name: 'Sales' } }, [], { now: Date.now() - 3 * 3600_000 })
    await realSave(
      { text: 'newer', source: { server_id: 'box.k2.dev', server_label: 'Box', user: 'rosson', agent: 'ops', agent_name: 'Ops' } },
      [{ name: 'a.png', type: 'image/png', bytes: new Blob([new Uint8Array([1])]) }],
      { now: Date.now() - 5 * 60_000 },
    )
    renderThreadBar()
    await waitFor(() => expect(screen.getByTestId('stash-button').getAttribute('aria-label')).toBe('Stash (2 items)'))
    pressStash()
    const drawer = await screen.findByTestId('stash-drawer')
    const rows = drawer.querySelectorAll('[data-stash-row]')
    expect([...rows].map((r) => r.querySelector('[data-stash-preview]')?.textContent)).toEqual(['newer', 'older'])
    expect(rows[0].querySelector('[data-stash-source]')?.textContent).toBe('Ops on Box')
    expect(rows[0].querySelector('[data-stash-age]')?.textContent).toBe('5m')
    expect(rows[0].querySelector('[data-stash-files]')?.getAttribute('data-stash-files')).toBe('1')
    expect(rows[1].querySelector('[data-stash-age]')?.textContent).toBe('3h')
    expect(rows[1].querySelector('[data-stash-files]')).toBeNull()
    // The list has the keys.
    expect(document.activeElement).toBe(screen.getByTestId('stash-list'))
  })

  it('the activity bar slides away while the drawer is open and comes back on close', async () => {
    renderThreadBar(primaryScope(), true)
    expect(activityPhase()).toBe('in')
    fireEvent.click(screen.getByTestId('stash-button'))
    await screen.findByTestId('stash-drawer')
    expect(activityPhase()).toBe('out')
    // Esc closes it; the bar slides back in.
    fireEvent.keyDown(screen.getByTestId('stash-list'), { key: 'Escape' })
    // It slides back out, then goes; the bar comes back at once.
    expect(screen.getByTestId('stash-drawer').getAttribute('data-closing')).toBe('true')
    expect(activityPhase()).toBe('in')
    await waitFor(() => expect(screen.queryByTestId('stash-drawer')).toBeNull())
  })

  it('arrow keys and Enter pick; the pick goes below existing text, the bar comes back, the item stays', async () => {
    await realSave({ text: 'second stash', source: { server_id: 'local', server_label: '', user: '', agent: '', agent_name: '' } }, [], { now: 1_000 })
    await realSave({ text: 'first stash', source: { server_id: 'local', server_label: '', user: '', agent: '', agent_name: '' } }, [], { now: 2_000 })
    renderThreadBar(primaryScope(), true)
    type('already typed')
    fireEvent.click(screen.getByTestId('stash-button'))
    const list = await screen.findByTestId('stash-list')
    await waitFor(() => expect(list.querySelectorAll('[data-stash-row]')).toHaveLength(2))
    expect(activityPhase()).toBe('out')
    fireEvent.keyDown(list, { key: 'ArrowDown' })
    fireEvent.keyDown(list, { key: 'Enter' })
    expect(box().value).toBe('already typed\n\nsecond stash')
    expect(activityPhase()).toBe('in')
    await waitFor(() => expect(screen.queryByTestId('stash-drawer')).toBeNull())
    await act(async () => {
      await new Promise((r) => setTimeout(r, 20))
    })
    expect(usePromptStashStore.getState().items.map((r) => r.text)).toEqual(['first stash', 'second stash'])
  })

  it('with "Remove after use" on, a pick takes the item out', async () => {
    await realSave({ text: 'one shot', source: { server_id: 'local', server_label: '', user: '', agent: '', agent_name: '' } })
    renderThreadBar()
    fireEvent.click(screen.getByTestId('stash-button'))
    const toggle = (await screen.findByTestId('stash-remove-after-use')) as HTMLInputElement
    expect(toggle.checked).toBe(false)
    fireEvent.click(toggle)
    expect(usePromptStashStore.getState().removeAfterUse).toBe(true)
    const row = await waitFor(() => {
      const r = document.querySelector('[data-stash-row]')
      if (!r) throw new Error('no row yet')
      return r
    })
    fireEvent.click(row)
    expect(box().value).toBe('one shot')
    await waitFor(() => expect(usePromptStashStore.getState().items).toEqual([]))
  })

  it('✕ removes a row', async () => {
    await realSave({ text: 'drop me', source: { server_id: 'local', server_label: '', user: '', agent: '', agent_name: '' } })
    renderThreadBar()
    fireEvent.click(screen.getByTestId('stash-button'))
    const remove = await screen.findByRole('button', { name: /Remove “drop me” from the stash/ })
    fireEvent.click(remove)
    await waitFor(() => expect(document.querySelector('[data-stash-row]')).toBeNull())
    expect(usePromptStashStore.getState().items).toEqual([])
  })
})

describe('per person on this device', () => {
  it('a stash made on one server shows in a composer on another', async () => {
    const remote = fakeScope('box.k2.dev', { remote: true })
    renderThreadBar(remote)
    type('from the box')
    pressStash()
    await waitFor(() => expect(box().value).toBe(''))
    expect(usePromptStashStore.getState().items[0].source.server_id).toBe('box.k2.dev')
    cleanup()
    // Another server's composer (this computer): same list.
    renderThreadBar(primaryScope())
    expect(screen.getByTestId('stash-button').getAttribute('aria-label')).toBe('Stash (1 item)')
    type('')
    pressStash()
    const drawer = await screen.findByTestId('stash-drawer')
    expect(drawer.querySelector('[data-stash-preview]')?.textContent).toBe('from the box')
  })
})

describe('closing the drawer', () => {
  it('a press outside the bar closes it and the activity bar comes back', async () => {
    renderThreadBar(primaryScope(), true)
    fireEvent.click(screen.getByTestId('stash-button'))
    await screen.findByTestId('stash-drawer')
    expect(activityPhase()).toBe('out')
    fireEvent.mouseDown(document.body)
    expect(activityPhase()).toBe('in')
    await waitFor(() => expect(screen.queryByTestId('stash-drawer')).toBeNull())
  })
})

describe('the drawer and its tab (Rosson 2026-10-30)', () => {
  it('opens with a slide-in, closes with the reverse, then unmounts; reopening while it leaves keeps it', async () => {
    renderThreadBar()
    fireEvent.click(screen.getByTestId('stash-button'))
    const drawer = await screen.findByTestId('stash-drawer')
    expect(drawer.hasAttribute('data-closing')).toBe(false)
    const css = drawer.querySelector('style[data-stash-styles]')?.textContent ?? ''
    expect(css).toContain('[data-stash-drawer] { animation: k2-stash-in 200ms')
    expect(css).toContain('[data-stash-drawer][data-closing="true"] { animation: k2-stash-out 180ms')
    expect(css).toMatch(/prefers-reduced-motion: reduce\)[^}]*\[data-stash-drawer\]/)
    // The handle closes it: it slides out...
    fireEvent.click(screen.getByTestId('stash-handle'))
    expect(screen.getByTestId('stash-drawer').getAttribute('data-closing')).toBe('true')
    // ...and the tab opens it again before it is gone: it stays.
    fireEvent.click(screen.getByTestId('stash-button'))
    expect(screen.getByTestId('stash-drawer').hasAttribute('data-closing')).toBe(false)
    await act(async () => {
      await new Promise((r) => setTimeout(r, 260))
    })
    expect(screen.queryByTestId('stash-drawer')).not.toBeNull()
    fireEvent.keyDown(screen.getByTestId('stash-list'), { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('stash-drawer')).toBeNull())
  })

  it('open, the tab rides on the drawer\'s top edge as its handle, one shape; the edge tab steps aside', async () => {
    renderThreadBar()
    const edgeTab = screen.getByTestId('stash-button')
    expect(edgeTab.style.visibility).toBe('')
    fireEvent.click(edgeTab)
    const drawer = await screen.findByTestId('stash-drawer')
    const handle = within(drawer).getByTestId('stash-handle')
    expect(handle.getAttribute('aria-expanded')).toBe('true')
    // One control on the open drawer: the handle is ✕; no bookmark, no
    // second ✕ in the header.
    expect(handle.getAttribute('aria-label')).toBe('Close stash')
    expect(handle.querySelector('[data-stash-close]')?.textContent?.trim()).toBe('✕')
    expect(handle.querySelector('[data-stash-icon]')).toBeNull()
    expect(within(drawer).getAllByRole('button', { name: 'Close stash' })).toEqual([handle])
    expect(drawer.querySelectorAll('svg path[d^="M19 21l-7-5-7 5V5"]')).toHaveLength(0)
    expect(handle.style.width).toBe('44px')
    const slot = handle.parentElement as HTMLElement
    expect(slot.style.position).toBe('absolute')
    expect(slot.style.bottom).toBe('calc(100% - 1px)')
    expect(slot.style.right).toBe('-1px')
    // The drawer's top-right corner runs straight into the handle.
    expect(drawer.style.borderRadius).toBe('calc(var(--zen-radius) + 2px) 0 0 0')
    expect(drawer.style.getPropertyValue('--ch-tr')).toBe('0px')
    expect(drawer.style.getPropertyValue('--ch-tl')).toBe('5px')
    expect(drawer.style.overflow).toBe('visible')
    // The edge tab keeps its place, unseen and out of the tab order.
    expect(edgeTab.style.visibility).toBe('hidden')
    expect(edgeTab.getAttribute('aria-hidden')).toBe('true')
    expect(edgeTab.tabIndex).toBe(-1)
  })

  it('chamfered in the square Style: the tab, the drawer and its rows; the checkbox is the themed one', async () => {
    await realSave({ text: 'a row', source: { server_id: 'local', server_label: '', user: '', agent: '', agent_name: '' } }, [], { now: 1_000 })
    renderThreadBar()
    const edgeTab = screen.getByTestId('stash-button')
    expect(edgeTab.hasAttribute('data-chamfer')).toBe(true)
    expect(['tl', 'tr', 'br', 'bl'].map((k) => edgeTab.style.getPropertyValue(`--ch-${k}`))).toEqual(['5px', '5px', '0px', '0px'])
    fireEvent.click(edgeTab)
    const drawer = await screen.findByTestId('stash-drawer')
    expect(drawer.hasAttribute('data-chamfer')).toBe(true)
    await waitFor(() => expect(drawer.querySelectorAll('[data-stash-row]')).toHaveLength(1))
    const rowEl = drawer.querySelector('[data-stash-row]') as HTMLElement
    expect(rowEl.style.getPropertyValue('--ch-tl')).toBe('3px')
    const css = drawer.querySelector('style[data-stash-styles]')?.textContent ?? ''
    expect(css).toContain('html[data-style="square"] [data-k2-thread-zen] [data-stash-row] { border-radius: 0 !important; clip-path: polygon(')
    const check = screen.getByTestId('stash-remove-after-use')
    expect(check.className).toContain('k2-square-check')
  })
})
