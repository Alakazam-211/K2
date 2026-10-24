// @vitest-environment jsdom
//
// The Kanban's Thread (Rosson 2026-10-24, for 0.46.4) — a Conversation with
// `pick = "all"`: the header is the drop-down over ANY agent, with a search
// field, and it shares its pick with the custom widget it follows.
//   - the prop: "all" (any case) is its own mode, never a one-name list;
//   - zen-data: following a custom widget, the conversation and the widget
//     are one view (one key); a pick in the header (`conversation.open` from
//     the conversation) is the widget's `selected` row, as the sealed frame
//     sees it (projected) — the row the Kanban's Send board posts to; with
//     nothing to follow it is the Garden's reach under its own key;
//   - the widget: "Pick an agent" until someone is picked; the menu has a
//     search field that keeps the focus and filters by name, address or
//     server; ↑/↓ move among the matches, Enter picks, Esc closes; a pick
//     opens the conversation (the shared selection) and is saved; a pick the
//     followed widget made shows and is saved too; the saved one reopens
//     after a remount; no "others" badge.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { useHomesStore, type Home } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { useProjectsStore } from '@/stores/projects'
import { parseZenGet } from '@/lib/zen/zen-page'
import { __resetZenDataForTests, openZenConversation, zenAgentRows, zenViewFor } from '@/lib/zen/zen-data'
import { projectZenRows } from '@/lib/zen/zen-custom-projection'
import {
  ZEN_CONVERSATION_PICKS_KEY,
  useZenConversationPicksStore,
  zenConversationPick,
  zenConversationPickAll,
} from '@/lib/zen/zen-conversation-pick'
import { ZEN_PICK_NONE_LABEL, ZEN_PICK_NONE_TEXT, ZenConversationWidget, zenPickAllState, zenPickNoMatchText } from './ZenConversationWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const ALICE = 'alice::local'
const BOB = 'bob::local'
const OPS = 'ops::box.k2.dev'

// ── the prop and the view (real zen-data) ────────────────────────────────

const WORK: Home = {
  id: 'home-work',
  name: 'Work',
  rows: [
    { address: ALICE, workspaceId: 'w1', label: 'Alice' },
    { address: BOB, workspaceId: 'w2', label: 'Bob' },
  ],
}

/** kanban-5.toml's page: the board (a custom widget that reads agents) and
 *  K2's Conversation following it with `pick = "all"`. */
function kanbanPage(thread: Record<string, unknown>): ReturnType<typeof parseZenGet> {
  return parseZenGet({
    ok: true,
    page: {
      template: 'k2.kanban@5',
      layout: { split: [70, 30] },
      controls: [],
      widgets: [
        {
          id: 'board',
          kind: 'custom',
          widget: 'k2:kanban@5',
          column: 0,
          props: { config: {} },
          caps: ['agents:read', 'thread:post', 'store:read', 'store:write'],
          requested: ['agents:read', 'thread:post', 'store:read', 'store:write'],
          name: 'Kanban',
          hash: 'h',
          state: 'ok',
          origin: 'local',
          paused: null,
        },
        { id: 'thread', kind: 'conversation', column: 1, props: thread, caps: ['agents:read', 'thread:read', 'thread:post'], source: 'builtin' },
      ],
    },
  })
}

describe('pick = "all": the prop and the shared view', () => {
  let saved: { homes: Home[]; selected: string }
  beforeEach(() => {
    saved = { homes: useHomesStore.getState().homes, selected: useHomesStore.getState().selectedId }
    useHomesStore.setState({ homes: [WORK], selectedId: WORK.id })
    useConnectHostStore.setState({ activeHost: 'local', hosts: [] })
    useProjectsStore.setState({ projects: [] })
    __resetZenDataForTests()
  })
  afterEach(() => {
    useHomesStore.setState({ homes: saved.homes, selectedId: saved.selected })
    __resetZenDataForTests()
  })

  it('"all" (any case) is its own mode, never a one-name list', () => {
    for (const v of ['all', ' ALL ', 'All']) {
      expect(zenConversationPickAll({ pick: v })).toBe(true)
      expect(zenConversationPick({ pick: v })).toBeNull()
    }
    expect(zenConversationPickAll({ pick: ['all', 'k2'] })).toBe(false)
    expect(zenConversationPick({ pick: ['all', 'k2'] })).toEqual(['all', 'k2'])
    expect(zenConversationPickAll({ pick: 'everyone' })).toBe(false)
    expect(zenConversationPickAll({})).toBe(false)
  })

  it('following the board: one view, and a header pick is the board’s selected row (what Send board posts to)', async () => {
    const p = kanbanPage({ agents: 'board', pick: 'all' })
    const board = zenViewFor(p, 'g-kanban1', 'board')
    const thread = zenViewFor(p, 'g-kanban1', 'thread')
    expect(board?.source).toBe('reach')
    expect(thread?.key).toBe(board?.key)
    expect(zenAgentRows(thread).map((r) => r.address)).toEqual([ALICE, BOB])
    expect(zenAgentRows(board).some((r) => r.selected)).toBe(false)
    // The header picks Bob: `conversation.open` from the conversation's view.
    await openZenConversation(thread!, BOB)
    const seen = projectZenRows(zenAgentRows(board), new Set(['agents:read']))
    expect(seen.filter((r) => r.selected).map((r) => r.address)).toEqual([BOB])
    // The board switches it (its own `conversation.open`): the header follows.
    await openZenConversation(board!, ALICE)
    expect(zenAgentRows(thread).filter((r) => r.selected).map((r) => r.address)).toEqual([ALICE])
  })

  it('with nothing to follow: the Garden’s reach under its own key', async () => {
    const p = kanbanPage({ pick: 'all' })
    const thread = zenViewFor(p, 'g-kanban1', 'thread')
    const board = zenViewFor(p, 'g-kanban1', 'board')
    expect(thread?.source).toBe('reach')
    expect(thread?.reach).toBe(true)
    expect(thread?.key).toBe('g-kanban1/thread')
    expect(thread?.key).not.toBe(board?.key)
    expect(zenAgentRows(thread).map((r) => r.address)).toEqual([ALICE, BOB])
    await openZenConversation(thread!, ALICE)
    expect(zenAgentRows(board).some((r) => r.selected)).toBe(false)
  })
})

// ── the widget, with a fake bridge (the shared selection) ────────────────

function row(address: string, label: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address,
    label,
    index: 0,
    hostKey: address.split('::')[1] ?? 'local',
    server: address.endsWith('::local') ? null : 'box',
    role: null,
    reach: 'live',
    auth: null,
    activity: 'idle',
    counts: null,
    working: false,
    needsYou: false,
    state: 'ok',
    stateLabel: null,
    detail: null,
    preview: null,
    people: [],
    selected: false,
    openable: true,
    avatarUrl: null,
    color: null,
    image: null,
    pinned: false,
    ...over,
  }
}

const reach = (): ZenAgentRow[] => [
  row(ALICE, 'Alice', { unread: { unread: 4, waiting: 1, muted: false } }),
  row(BOB, 'Bob'),
  row(OPS, 'Ops desk'),
]

interface Fake {
  bridge: ZenWidgetBridge
  opened: string[]
  /** The followed widget (the board) moves the shared selection. */
  widgetOpens(address: string): void
}

/** The K2 side, faked: the board's view (the reach), one selection shared
 *  by the conversation and the board; `conversation.open` selects. */
function fakeBridge(start: ZenAgentRow[]): Fake {
  let rows = start
  const listeners = new Set<(r: ZenAgentRow[]) => void>()
  const opened: string[] = []
  const select = (address: string): void => {
    rows = rows.map((r) => ({ ...r, selected: r.address === address }))
    for (const l of listeners) l(rows)
  }
  const bridge = {
    widgetId: 'thread',
    caps: new Set<string>(['agents:read', 'thread:read', 'thread:post']),
    call(verb: string, ...args: unknown[]): unknown {
      switch (verb) {
        case 'agents.list':
          return rows
        case 'agents.subscribe': {
          const cb = args[0] as (r: ZenAgentRow[]) => void
          listeners.add(cb)
          return () => listeners.delete(cb)
        }
        case 'thread.subscribe':
          return () => {}
        case 'conversation.open': {
          const address = args[0] as string
          opened.push(address)
          select(address)
          return Promise.resolve(rows.find((r) => r.address === address))
        }
        default:
          throw new Error(`unexpected bridge verb ${verb}`)
      }
    },
    gardens: { list: () => [], current: () => ({ id: 'g-kanban1', name: 'Kanban' }), switch: () => {}, create: () => Promise.reject(new Error('no')) },
    homes: { list: () => [] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return { bridge, opened, widgetOpens: select }
}

function decl(): ZenWidgetDecl {
  return {
    id: 'thread',
    kind: 'conversation',
    column: 1,
    props: { agents: 'board', pick: 'all', compose: false, attachments: false, 'load-older': false, 'activity-drawer': true, mute: false },
    caps: ['agents:read', 'thread:read', 'thread:post'],
    source: 'builtin',
  } as ZenWidgetDecl
}

function el(sel: string, root: ParentNode = document): HTMLElement {
  const found = root.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}

async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

async function mount(fake: Fake): Promise<ReturnType<typeof render>> {
  const r = render(<ZenConversationWidget bridge={fake.bridge} decl={decl()} />)
  await flush()
  return r
}

const shown = (): string | null => document.querySelector('[data-zen-conversation]')?.getAttribute('data-zen-conversation') ?? null
const button = (): HTMLButtonElement => el('[data-zen-pick-button]') as HTMLButtonElement
const options = (): HTMLElement[] => Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))
const search = (): HTMLInputElement => el('[data-zen-pick-search-input]') as HTMLInputElement
const savedPick = (): string | undefined => useZenConversationPicksStore.getState().picks['g-kanban1/thread']

async function openMenu(): Promise<void> {
  await act(async () => {
    fireEvent.click(button())
  })
}

async function typeSearch(text: string): Promise<void> {
  await act(async () => {
    fireEvent.change(search(), { target: { value: text } })
  })
}

async function key(k: string): Promise<void> {
  await act(async () => {
    fireEvent.keyDown(search(), { key: k })
  })
}

/** The card the keyboard is on (the combobox's active descendant). */
function active(): string | null {
  const id = search().getAttribute('aria-activedescendant')
  return id ? (document.getElementById(id)?.getAttribute('data-zen-pick-option') ?? null) : null
}

describe('Conversation with pick = "all": any agent from its header', () => {
  beforeEach(() => {
    localStorage.clear()
    useZenConversationPicksStore.setState({ picks: {} })
  })
  afterEach(() => {
    cleanup()
  })

  it('before a pick: "Pick an agent" in the header, nothing opened', async () => {
    const fake = await (async () => {
      const f = fakeBridge(reach())
      await mount(f)
      return f
    })()
    expect(shown()).toBeNull()
    expect(fake.opened).toEqual([])
    expect(el('[data-zen-conversation-title]', button()).textContent).toBe(ZEN_PICK_NONE_LABEL)
    expect(el('[data-zen-conversation-none]').textContent).toBe(ZEN_PICK_NONE_TEXT)
    expect(button().getAttribute('aria-haspopup')).toBe('listbox')
    expect(button().getAttribute('aria-expanded')).toBe('false')
    el('[data-zen-pick-chevron]', button())
  })

  it('the menu: a search field that keeps the focus and filters by name, address or server', async () => {
    const fake = fakeBridge(reach())
    await mount(fake)
    await openMenu()
    const list = el('[role="listbox"]')
    expect(button().getAttribute('aria-expanded')).toBe('true')
    expect(document.activeElement).toBe(search())
    expect(search().getAttribute('role')).toBe('combobox')
    expect(search().getAttribute('aria-controls')).toBe(list.id)
    expect(options().map((o) => o.getAttribute('data-zen-pick-option'))).toEqual([ALICE, BOB, OPS])
    expect(active()).toBe(ALICE)
    // By name, case aside.
    await typeSearch('BoB')
    expect(options().map((o) => o.getAttribute('data-zen-pick-option'))).toEqual([BOB])
    expect(active()).toBe(BOB)
    // By server ("box") and by address.
    await typeSearch('box')
    expect(options().map((o) => o.getAttribute('data-zen-pick-option'))).toEqual([OPS])
    await typeSearch('alice::')
    expect(options().map((o) => o.getAttribute('data-zen-pick-option'))).toEqual([ALICE])
    // No match says so; Enter then picks nothing.
    await typeSearch('zzz')
    expect(options()).toEqual([])
    expect(el('[data-zen-pick-empty]').textContent).toBe(zenPickNoMatchText('zzz'))
    expect(search().getAttribute('aria-activedescendant')).toBeNull()
    await key('Enter')
    expect(fake.opened).toEqual([])
    expect(document.querySelector('[role="listbox"]')).not.toBeNull()
    // Esc closes and hands the focus back to the header.
    await key('Escape')
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    expect(document.activeElement).toBe(button())
  })

  it('keyboard: ↓/↑ move among the matches and wrap, Space types, Enter picks; the pick is the shared selection and is saved', async () => {
    const fake = fakeBridge(reach())
    await mount(fake)
    await act(async () => {
      button().focus()
      fireEvent.keyDown(button(), { key: 'ArrowDown' })
    })
    expect(document.activeElement).toBe(search())
    await key('ArrowDown')
    expect(active()).toBe(BOB)
    await key('ArrowDown')
    expect(active()).toBe(OPS)
    await key('ArrowDown')
    expect(active()).toBe(ALICE)
    await key('ArrowUp')
    expect(active()).toBe(OPS)
    // Space is the field's (a search can hold one), not a pick.
    await key(' ')
    expect(fake.opened).toEqual([])
    expect(document.querySelector('[role="listbox"]')).not.toBeNull()
    await key('Enter')
    await flush()
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    // conversation.open: the conversation and the board it follows both see Ops.
    expect(fake.opened).toEqual([OPS])
    expect(shown()).toBe(OPS)
    expect(savedPick()).toBe(OPS)
    expect(JSON.parse(localStorage.getItem(ZEN_CONVERSATION_PICKS_KEY) ?? 'null')).toEqual({ version: 1, picks: { 'g-kanban1/thread': OPS } })
    // The keyboard stays on the (new) header; it badges no "others".
    expect(document.activeElement).toBe(button())
    expect(document.querySelector('[data-zen-pick-badge]')).toBeNull()
    expect(el('[data-zen-conversation-title]', button()).textContent).toBe('Ops desk')

    // A click on a card in the open menu switches again.
    await openMenu()
    expect(options().find((o) => o.getAttribute('aria-selected') === 'true')?.getAttribute('data-zen-pick-option')).toBe(OPS)
    await act(async () => {
      fireEvent.click(options()[1])
    })
    await flush()
    expect(fake.opened).toEqual([OPS, BOB])
    expect(shown()).toBe(BOB)
    expect(savedPick()).toBe(BOB)
  })

  it('a pick the followed widget makes shows in the header and is saved; the saved one reopens after a remount', async () => {
    const fake = fakeBridge(reach())
    const r = await mount(fake)
    // The board (or K2's own carry-over) moves the shared selection.
    await act(async () => {
      fake.widgetOpens(ALICE)
    })
    await flush()
    expect(shown()).toBe(ALICE)
    expect(savedPick()).toBe(ALICE)
    expect(fake.opened).toEqual([])
    r.unmount()
    // Next time (nothing selected yet): the saved pick opens itself.
    const again = fakeBridge(reach())
    await mount(again)
    expect(again.opened).toEqual([ALICE])
    expect(shown()).toBe(ALICE)
  })

  it('zenPickAllState: the selection wins, then the saved pick while listed; a list’s off-list rows are no cards', () => {
    const rows = [row(ALICE, 'Alice'), row(BOB, 'Bob', { selected: true }), row(OPS, 'Ops desk', { offList: true })]
    const s = zenPickAllState(rows, ALICE)
    expect(s.entries.map((e) => e.row?.address)).toEqual([ALICE, BOB])
    expect(s.shown?.address).toBe(BOB)
    expect(zenPickAllState(rows.map((r) => ({ ...r, selected: false })), ALICE).shown?.address).toBe(ALICE)
    expect(zenPickAllState(rows.map((r) => ({ ...r, selected: false })), 'gone::local').shown).toBeNull()
    expect(zenPickAllState(rows.map((r) => ({ ...r, selected: false })), OPS).shown).toBeNull()
  })
})
