// @vitest-environment jsdom
//
// 0.46.3 r10 (prd-garden-column-collapse-and-agent-list-v1 A2, CA11–CA13):
// the Agents widget in list mode (`list-from`), drawn from the rows the
// bridge gives it (zen-data's list view: the listed rows, then off-list
// rows that need you, flagged `offList`).
//   - "Nothing listed yet" before the first show, never the whole reach;
//   - the list in its order; ⌘↑ / ⌘↓ walk it; off-list rows never drawn;
//   - the footer counts the off-list agents that need you;
//   - no Home picker, no Add agent, Recent agents first off;
//   - the same widget without list-from is unchanged.
// Fixtures are made up (example.test, g-test0001).
import { afterEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import { useZenRecentFirstStore } from '@/lib/zen/zen-recent-first'
import { ZEN_EMPTY_HOME, ZEN_EMPTY_LIST, ZenAgentsWidget, zenOthersNeedYou } from './ZenAgentsWidget'

function row(address: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address,
    label: address.split('::')[0],
    index: 0,
    hostKey: address.split('::')[1],
    server: null,
    role: null,
    reach: 'live',
    auth: 'ok',
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
    ...over,
  }
}

interface Fake {
  bridge: ZenWidgetBridge
  push(rows: ZenAgentRow[]): void
  opened: string[]
}

function fake(initial: ZenAgentRow[], caps = ['agents:read', 'agents:add', 'presence:read']): Fake {
  let rows = initial
  let cb: ((r: ZenAgentRow[]) => void) | null = null
  const opened: string[] = []
  const call = (verb: string, ...args: unknown[]): unknown => {
    switch (verb) {
      case 'agents.list':
        return rows
      case 'agents.subscribe':
        cb = args[0] as (r: ZenAgentRow[]) => void
        return () => {
          cb = null
        }
      case 'agents.home':
        return null
      case 'gardens.current':
        return { id: 'g-test0001', name: 'Board', index: 1 }
      case 'homes.list':
        return [{ id: 'home-work', name: 'Work' }]
      case 'conversation.open':
        opened.push(String(args[0]))
        return Promise.resolve(null)
      default:
        throw new Error(`unexpected verb ${verb}`)
    }
  }
  const bridge = {
    widgetId: 'crew',
    caps: new Set(caps),
    call,
    gardens: { current: () => call('gardens.current'), list: () => [] },
    homes: { list: () => call('homes.list') },
  } as unknown as ZenWidgetBridge
  return {
    bridge,
    opened,
    push: (next) => {
      rows = next
      act(() => cb?.(next))
    },
  }
}

function decl(props: Record<string, unknown>): ZenWidgetDecl {
  return { id: 'crew', kind: 'agents', column: 1, props, caps: ['agents:read', 'agents:add'], source: 'builtin' }
}

const LIST = { mode: 'list', 'list-from': 'board', 'home-picker': false, preview: true, unread: true, 'server-tag': true }

afterEach(() => {
  cleanup()
  useZenRecentFirstStore.setState({ stored: null })
})

describe('the Agents widget in list mode', () => {
  it('before the first show: "Nothing listed yet", no Add agent, no footer', () => {
    const f = fake([])
    const { container } = render(<ZenAgentsWidget bridge={f.bridge} decl={decl(LIST)} />)
    const root = container.querySelector('[data-zen-widget="agents"]')!
    expect(root.getAttribute('data-zen-agents-mode')).toBe('list')
    expect(container.querySelector('[data-zen-agents-empty]')?.textContent).toBe(ZEN_EMPTY_LIST)
    expect(container.querySelector('[data-zen-add-agent]')).toBeNull()
    expect(container.querySelector('[data-zen-agents-others-need-you]')).toBeNull()
  })

  it('draws the list in its order; off-list rows are counted, never drawn; ⌘↓ walks the list', async () => {
    const f = fake([
      row('carol::local', { index: 0 }),
      row('dave::box.example.test', { index: 1, server: 'Box', state: 'sign-in', stateLabel: 'Sign in', openable: false }),
      row('bob::local', { index: 2, needsYou: true, activity: 'needs-you', offList: true }),
      row('erin::local', { index: 3, unread: { unread: 0, waiting: 1, muted: false }, offList: true }),
    ])
    const { container } = render(<ZenAgentsWidget bridge={f.bridge} decl={decl(LIST)} />)
    const drawn = [...container.querySelectorAll('[data-zen-agents-order] [role="option"]')].map((b) =>
      b.querySelector('[data-zen-agent-name]')?.textContent,
    )
    expect(drawn).toEqual(['carol', 'dave'])
    expect(container.querySelector('[data-zen-agents-order]')?.getAttribute('data-zen-agents-order')).toBe('home')
    const footer = container.querySelector('[data-zen-agents-others-need-you]')!
    expect(footer.textContent).toBe('2 others need you')
    expect(footer.getAttribute('role')).toBe('status')
    // ⌘↓ from nothing picked opens the list's first row, never an off-list one.
    fireEvent.keyDown(window, { key: 'ArrowDown', metaKey: true })
    await vi.waitFor(() => expect(f.opened).toEqual(['carol::local']))
    // One hidden agent left: the singular.
    f.push([row('carol::local'), row('bob::local', { needsYou: true, offList: true })])
    expect(container.querySelector('[data-zen-agents-others-need-you]')?.textContent).toBe(zenOthersNeedYou(1))
    expect(zenOthersNeedYou(1)).toBe('1 other needs you')
    // Nobody off the list needs you: no footer.
    f.push([row('carol::local')])
    expect(container.querySelector('[data-zen-agents-others-need-you]')).toBeNull()
  })

  it('no Home picker even with home-picker on; Recent agents first is off', () => {
    useZenRecentFirstStore.setState({ stored: true })
    const f = fake([row('carol::local')])
    const { container } = render(<ZenAgentsWidget bridge={f.bridge} decl={decl({ ...LIST, 'home-picker': true, 'recent-first': true })} />)
    expect(container.querySelector('[data-zen-home-picker]')).toBeNull()
    expect(container.querySelector('[data-zen-agents-order]')?.getAttribute('data-zen-agents-order')).toBe('home')
  })

  it('the Agents rail view (source: workspaces) ignores list-from', () => {
    const f = fake([], ['agents:read'])
    const groups = { enabled: false, groups: [], active: null }
    const bridge = {
      ...f.bridge,
      call: (verb: string, ...args: unknown[]) =>
        verb === 'focusGroups.get' ? groups : verb === 'focusGroups.subscribe' ? () => undefined : f.bridge.call(verb as never, ...args),
    } as unknown as ZenWidgetBridge
    const { container } = render(<ZenAgentsWidget bridge={bridge} decl={decl({ ...LIST, source: 'workspaces' })} />)
    expect(container.querySelector('[data-zen-widget="agents"]')?.getAttribute('data-zen-agents-mode')).toBe('home')
    expect(container.querySelector('[data-zen-agents-empty]')?.textContent).not.toBe(ZEN_EMPTY_LIST)
  })

  it('without list-from: today’s whole-Home widget (Add agent, the Home empty state)', () => {
    const f = fake([])
    const { container } = render(<ZenAgentsWidget bridge={f.bridge} decl={decl({ mode: 'home' })} />)
    expect(container.querySelector('[data-zen-widget="agents"]')?.getAttribute('data-zen-agents-mode')).toBe('home')
    expect(container.querySelector('[data-zen-agents-empty]')?.textContent).toBe(ZEN_EMPTY_HOME)
    expect(container.querySelector('[data-zen-add-agent]')).not.toBeNull()
  })
})
