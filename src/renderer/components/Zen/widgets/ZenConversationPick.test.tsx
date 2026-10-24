// @vitest-environment jsdom
//
// Fleet Ops 2 (Rosson 2026-10-24) — a Conversation with `pick`: the header
// is the drop-down, one layer. Through the real widget with a fake bridge
// that answers like the K2 side (`conversation.open` selects the row and
// re-emits the rows):
//   - the header shows the first agent found, a chevron, and a badge with
//     the OTHER agents' unread and waiting (a muted Thread's unread left out);
//   - a click opens a listbox over the Thread inside the widget's box, one
//     card per agent in `pick` order, each drawn like the header, the shown
//     one selected; an agent that isn't found is a disabled card;
//   - picking a card switches the Thread (`conversation.open`) and closes;
//     picking the shown one, Esc and a click outside only close;
//   - ↓ on the header opens it, ↑/↓ skip the disabled card, Enter picks;
//   - the pick is kept per Garden and widget on this computer and comes
//     back after a remount.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import {
  ZEN_CONVERSATION_PICKS_KEY,
  useZenConversationPicksStore,
  zenConversationPick,
  zenFindPickRow,
} from '@/lib/zen/zen-conversation-pick'
import { ZenConversationWidget, zenPickBadge, zenPickMissingText } from './ZenConversationWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected POST ${route}`)
  }),
}))

const WEB = 'k2-dev-web::web.k2.dev'
const K2 = 'k2::local'

function row(address: string, label: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address,
    label,
    index: 0,
    hostKey: address.split('::')[1] ?? 'local',
    server: address.endsWith('::local') ? null : 'web box',
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

interface Fake {
  bridge: ZenWidgetBridge
  opened: string[]
}

/** The K2 side, faked: `pick` rows (only the found ones), and
 *  `conversation.open` selects one and re-emits. */
function fakeBridge(start: ZenAgentRow[]): Fake {
  let rows = start
  const listeners = new Set<(r: ZenAgentRow[]) => void>()
  const opened: string[] = []
  const bridge = {
    widgetId: 'talk',
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
          rows = rows.map((r) => ({ ...r, selected: r.address === address }))
          for (const l of listeners) l(rows)
          return Promise.resolve(rows.find((r) => r.address === address))
        }
        default:
          throw new Error(`unexpected bridge verb ${verb}`)
      }
    },
    gardens: { list: () => [], current: () => ({ id: 'g-ops', name: 'Fleet Ops 2' }), switch: () => {}, create: () => Promise.reject(new Error('no')) },
    homes: { list: () => [] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return { bridge, opened }
}

function decl(pick: unknown): ZenWidgetDecl {
  return {
    id: 'talk',
    kind: 'conversation',
    column: 0,
    props: { pick, compose: false, attachments: false, 'load-older': false, 'activity-drawer': true, mute: false },
    caps: ['agents:read', 'thread:read', 'thread:post'],
    source: 'builtin',
  } as ZenWidgetDecl
}

function rows(): ZenAgentRow[] {
  return [
    row(WEB, 'k2-dev-web', { unread: { unread: 2, waiting: 0, muted: false } }),
    row(K2, 'k2', { activity: 'working', working: true, unread: { unread: 3, waiting: 1, muted: false } }),
  ]
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

async function mount(fake: Fake, pick: unknown = ['k2-dev-web', 'k2', 'ghost']): Promise<ReturnType<typeof render>> {
  const r = render(<ZenConversationWidget bridge={fake.bridge} decl={decl(pick)} />)
  await flush()
  return r
}

const shown = (): string | null => document.querySelector('[data-zen-conversation]')?.getAttribute('data-zen-conversation') ?? null
const button = (): HTMLButtonElement => el('[data-zen-pick-button]') as HTMLButtonElement
const options = (): HTMLElement[] => Array.from(document.querySelectorAll<HTMLElement>('[role="option"]'))

async function openMenu(): Promise<HTMLElement> {
  await act(async () => {
    fireEvent.click(button())
  })
  return el('[role="listbox"]')
}

beforeEach(() => {
  localStorage.clear()
  useZenConversationPicksStore.setState({ picks: {} })
})

afterEach(() => {
  cleanup()
})

describe('the `pick` prop', () => {
  it('reads a TOML list or a comma-separated string; matches like the pinned `agent` prop', () => {
    expect(zenConversationPick({ pick: [' k2-dev-web ', 'k2', ''] })).toEqual(['k2-dev-web', 'k2'])
    expect(zenConversationPick({ pick: 'k2-dev-web, k2' })).toEqual(['k2-dev-web', 'k2'])
    expect(zenConversationPick({ pick: [] })).toBeNull()
    expect(zenConversationPick({ agent: 'k2' })).toBeNull()
    const local = { address: 'k2::local', label: 'K2' }
    const remote = { address: 'k2::other.k2.dev', label: 'k2' }
    const named = { address: 'ops-desk::local', label: 'Ops' }
    // Address, then handle, then name; first in order (this computer first).
    expect(zenFindPickRow([local, remote], 'k2::other.k2.dev')).toBe(remote)
    expect(zenFindPickRow([named, remote, local], 'K2')).toBe(remote)
    expect(zenFindPickRow([named, local], 'ops')).toBe(named)
    expect(zenFindPickRow([named, local], 'ghost')).toBeNull()
  })

  it('the badge sums the other agents only; a muted Thread’s unread is left out, its waiting counts', () => {
    const entries = [
      { name: 'a', row: row('a::local', 'a', { unread: { unread: 5, waiting: 1, muted: false } }) },
      { name: 'b', row: row('b::local', 'b', { unread: { unread: 2, waiting: 0, muted: false } }) },
      { name: 'c', row: row('c::local', 'c', { unread: { unread: 9, waiting: 2, muted: true } }) },
      { name: 'ghost', row: null },
    ]
    expect(zenPickBadge(entries, 'a::local')).toEqual({ unread: 2, waiting: 2, muted: false })
  })
})

describe('Conversation with `pick`: the header is the drop-down', () => {
  it('shows the first agent with a chevron and the others’ counts, and opens it', async () => {
    const fake = fakeBridge(rows())
    await mount(fake)
    expect(fake.opened).toEqual([WEB])
    expect(shown()).toBe(WEB)
    const b = button()
    expect(b.tagName).toBe('BUTTON')
    expect(b.getAttribute('type')).toBe('button')
    expect(b.getAttribute('aria-haspopup')).toBe('listbox')
    expect(b.getAttribute('aria-expanded')).toBe('false')
    expect(el('[data-zen-conversation-title]', b).textContent).toBe('k2-dev-web')
    expect(el('[data-zen-conversation-status]', b).textContent).toBe('web box · idle')
    el('[data-zen-pick-chevron]', b)
    // k2's 3 unread and 1 waiting; never the shown agent's own 2.
    const badge = el('[data-zen-pick-badge]', b)
    expect(el('[data-zen-unread-count]', badge).getAttribute('data-zen-unread-count')).toBe('3')
    expect(el('[data-zen-unread-waiting]', badge).getAttribute('data-zen-unread-waiting')).toBe('1')
    expect(document.querySelector('[role="listbox"]')).toBeNull()
  })

  it('a click opens one card per agent over the Thread, drawn like the header; the unknown one is disabled', async () => {
    const fake = fakeBridge(rows())
    await mount(fake)
    const list = await openMenu()
    expect(button().getAttribute('aria-expanded')).toBe('true')
    expect(button().getAttribute('aria-controls')).toBe(list.id)
    expect(document.activeElement).toBe(list)
    // Inside the widget's own box, from the header's top edge.
    const panel = el('[data-zen-pick-menu]')
    expect(el('[data-zen-conversation]').contains(panel)).toBe(true)
    expect(panel.style.position === 'absolute' || panel.className.includes('absolute')).toBe(true)
    expect(panel.style.top).toBe('0px')
    expect(panel.getAttribute('data-zen-material')).toBe('tile')
    const opts = options()
    expect(opts.map((o) => o.getAttribute('data-zen-pick-option'))).toEqual([WEB, K2, ''])
    expect(opts.map((o) => o.getAttribute('aria-selected'))).toEqual(['true', 'false', 'false'])
    expect(list.getAttribute('aria-activedescendant')).toBe(opts[0].id)
    el('[data-zen-pick-check]', opts[0])
    expect(opts[1].querySelector('[data-zen-pick-check]')).toBeNull()
    // Each card: the header's picture, name, dot and line, plus its own counts.
    const k2 = opts[1]
    expect(el('[data-zen-avatar]', k2).style.width).toBe('34px')
    expect(el('[data-zen-pick-title]', k2).textContent).toBe('k2')
    expect(el('[data-zen-pick-status]', k2).textContent).toBe('this computer · working')
    expect(el('[data-zen-status-dot]', k2).getAttribute('data-zen-status-dot')).toBe('working')
    expect(el('[data-zen-unread-count]', k2).getAttribute('data-zen-unread-count')).toBe('3')
    expect(el('[data-zen-unread-count]', opts[0]).getAttribute('data-zen-unread-count')).toBe('2')
    // Not found: a disabled card that says so, and the rest still work.
    const ghost = opts[2]
    expect(ghost.getAttribute('data-zen-pick-missing')).toBe('ghost')
    expect(ghost.getAttribute('aria-disabled')).toBe('true')
    expect(ghost.textContent).toContain(zenPickMissingText('ghost'))
    await act(async () => {
      fireEvent.click(ghost)
    })
    expect(document.querySelector('[role="listbox"]')).not.toBeNull()
    expect(fake.opened).toEqual([WEB])
  })

  it('picking a card switches the Thread and closes; the pick is kept per Garden and widget', async () => {
    const fake = fakeBridge(rows())
    const r = await mount(fake)
    await openMenu()
    await act(async () => {
      fireEvent.click(options()[1])
    })
    await flush()
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    expect(fake.opened).toEqual([WEB, K2])
    expect(shown()).toBe(K2)
    // The keyboard stays on the (new) header.
    expect(document.activeElement).toBe(button())
    // Now the badge counts k2-dev-web, not k2.
    expect(el('[data-zen-unread-count]', el('[data-zen-pick-badge]')).getAttribute('data-zen-unread-count')).toBe('2')
    expect(document.querySelector('[data-zen-pick-badge] [data-zen-unread-waiting]')).toBeNull()
    expect(useZenConversationPicksStore.getState().picks['g-ops/talk']).toBe(K2)
    expect(JSON.parse(localStorage.getItem(ZEN_CONVERSATION_PICKS_KEY) ?? 'null')).toEqual({ version: 1, picks: { 'g-ops/talk': K2 } })
    // A remount (Zen again, another window) comes back to it.
    r.unmount()
    const again = fakeBridge(rows())
    await mount(again)
    expect(shown()).toBe(K2)
    expect(again.opened).toEqual([K2])
  })

  it('picking the shown card, Esc and a click outside only close', async () => {
    const fake = fakeBridge(rows())
    await mount(fake)
    await openMenu()
    await act(async () => {
      fireEvent.click(options()[0])
    })
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    expect(document.activeElement).toBe(button())
    const list = await openMenu()
    await act(async () => {
      fireEvent.keyDown(list, { key: 'Escape' })
    })
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    expect(document.activeElement).toBe(button())
    await openMenu()
    // A click inside the panel keeps it; one outside closes it.
    await act(async () => {
      fireEvent.mouseDown(el('[data-zen-pick-menu]'))
    })
    expect(document.querySelector('[role="listbox"]')).not.toBeNull()
    await act(async () => {
      fireEvent.mouseDown(document.body)
    })
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    expect(button().getAttribute('aria-expanded')).toBe('false')
    expect(fake.opened).toEqual([WEB])
    expect(shown()).toBe(WEB)
  })

  it('keyboard: ↓ on the header opens it, ↑/↓ skip the disabled card and wrap, Enter picks', async () => {
    const fake = fakeBridge(rows())
    await mount(fake, ['k2-dev-web', 'ghost', 'k2'])
    await act(async () => {
      button().focus()
      fireEvent.keyDown(button(), { key: 'ArrowDown' })
    })
    const list = el('[role="listbox"]')
    const active = (): string | null => {
      const id = list.getAttribute('aria-activedescendant')
      return id ? (document.getElementById(id)?.getAttribute('data-zen-pick-option') ?? null) : null
    }
    expect(active()).toBe(WEB)
    await act(async () => {
      fireEvent.keyDown(list, { key: 'ArrowDown' })
    })
    // The ghost (second) is skipped.
    expect(active()).toBe(K2)
    await act(async () => {
      fireEvent.keyDown(list, { key: 'ArrowDown' })
    })
    expect(active()).toBe(WEB)
    await act(async () => {
      fireEvent.keyDown(list, { key: 'ArrowUp' })
    })
    expect(active()).toBe(K2)
    expect(el('[data-active]', list).getAttribute('data-zen-pick-option')).toBe(K2)
    await act(async () => {
      fireEvent.keyDown(list, { key: 'Enter' })
    })
    await flush()
    expect(document.querySelector('[role="listbox"]')).toBeNull()
    expect(fake.opened).toEqual([WEB, K2])
    expect(shown()).toBe(K2)
  })

  it('a first agent that isn’t found: the next one shows; none found says so', async () => {
    const fake = fakeBridge(rows())
    await mount(fake, ['ghost', 'k2'])
    expect(shown()).toBe(K2)
    expect(fake.opened).toEqual([K2])
    await openMenu()
    expect(options().map((o) => o.getAttribute('aria-disabled'))).toEqual(['true', null])
    cleanup()
    const none = fakeBridge([])
    await mount(none, ['ghost', 'phantom'])
    expect(shown()).toBeNull()
    expect(el('[data-zen-conversation-none]').textContent).toBe('None of ghost, phantom is on this Home.')
    expect(none.opened).toEqual([])
  })
})
