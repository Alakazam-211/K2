// @vitest-environment jsdom
//
// 0.46.1 Zen defaults for thread-read-v1, through the real widgets with a
// fake bridge and the real unread store (daemon calls mocked):
//   - Agents: unread / waiting counts on list rows AND on the pin grid's
//     pictures (`unread`, on by default); `unread = false` hides both.
//   - Conversation: marks its Thread read while it is looked at (window
//     visible and focused, a ready conversation on the window's server),
//     never on another server or when the window isn't focused.
//   - "N waiting": the cards waiting on you; a click scrolls to the first.
//   - `mute` (Garden 1): the bell mutes / unmutes the Thread
//     (`thread/mute`); hidden without the prop or on another server.
//   - The nav rail: Agents and Projects carry the unread badges
//     (`app.badges`), and `zenAppBadges` derives them from the store.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act } from 'react'
import { cleanup, fireEvent, render } from '@testing-library/react'
import type { ZenWidgetBridge } from '@/lib/zen/zen-bridge'
import type { ZenAgentRow, ZenThreadView } from '@/lib/zen/zen-data'
import type { ZenWidgetDecl } from '@/lib/zen/zen-page'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { __resetUnreadForTests, useUnreadStore, type UnreadEntry } from '@/stores/unread'
import { zenAppBadges } from '@/lib/zen/zen-app-nav'
import { ZenAgentsWidget } from './ZenAgentsWidget'
import { ZenConversation, type ZenConversationOptions } from './ZenConversationWidget'
import { ZenNavRailWidget } from './ZenNavRailWidget'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const h = vi.hoisted(() => ({
  posts: [] as { route: string; body: Record<string, unknown> }[],
  supports: true,
  // The daemon's mute for conv-1: read replies carry it, as the daemon's do.
  muted: false,
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: Record<string, unknown>) => {
    h.posts.push({ route, body })
    if (route === 'thread/read') {
      return {
        kind: 'thread',
        id: body.conversationId,
        lastReadSeq: body.seq,
        lastSeq: body.seq,
        unread: 0,
        waiting: 1,
        muted: h.muted,
        total: 1,
        waitingTotal: 1,
      }
    }
    if (route === 'thread/mute') {
      h.muted = body.muted === true
      return {
        kind: 'thread',
        id: body.id,
        lastReadSeq: 5,
        lastSeq: 5,
        unread: 2,
        waiting: 0,
        muted: body.muted,
        total: 0,
        waitingTotal: 0,
      }
    }
    throw new Error(`unexpected POST ${route}`)
  }),
}))

vi.mock('@/kessel/server-scope', async (orig) => {
  const real = (await orig()) as Record<string, unknown>
  return {
    ...real,
    primaryScope: () => ({ serverSupports: (f: string) => h.supports && f === 'thread-read-v1', isWindowHost: () => true }),
  }
})

function row(handle: string, over: Partial<ZenAgentRow> = {}): ZenAgentRow {
  return {
    address: `${handle}::local`,
    label: handle,
    index: 0,
    hostKey: 'local',
    server: null,
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
  pushThread(view: ZenThreadView): void
}

function fakeBridge(rows: ZenAgentRow[], badges: unknown = { tickets: { badge: null, stale: false, title: '' } }): Fake {
  let threadListener: ((v: ZenThreadView) => void) | null = null
  const bridge = {
    widgetId: 'w',
    caps: new Set<string>(['agents:read', 'thread:read', 'thread:post', 'app:navigate']),
    call(verb: string, ...args: unknown[]): unknown {
      switch (verb) {
        case 'agents.list':
          return rows
        case 'agents.subscribe':
        case 'app.subscribe':
        case 'app.subscribeCurrent':
          return () => {}
        case 'agents.home':
          return 'home-1'
        case 'app.badges':
          return badges
        case 'app.current':
          return 'home'
        case 'thread.subscribe':
          threadListener = args[1] as (v: ZenThreadView) => void
          return () => {
            threadListener = null
          }
        default:
          throw new Error(`unexpected bridge verb ${verb}`)
      }
    },
    gardens: { list: () => [], current: () => ({ id: 'g-1', name: 'Garden 1' }), switch: () => {}, create: () => Promise.reject(new Error('no')) },
    homes: { list: () => [{ id: 'home-1', name: 'Home', rows: rows.length }] },
    zen: { exit: () => {} },
  } as unknown as ZenWidgetBridge
  return {
    bridge,
    pushThread(view) {
      const fn = threadListener
      if (!fn) throw new Error('nobody subscribed to the thread')
      act(() => fn(view))
    },
  }
}

function agentsDecl(props: Record<string, unknown>): ZenWidgetDecl {
  return { id: 'agents', kind: 'agents', column: 0, props: { 'home-picker': false, ...props }, caps: ['agents:read'], source: 'builtin' } as ZenWidgetDecl
}

const text = (id: string, seq: number): OverlayThreadItem =>
  ({ collection: 'thread', seq, id, doc: { id, kind: 'text', from: 'alpha', body: `message ${seq}`, created_at: 900 + seq } }) as unknown as OverlayThreadItem

const choice = (id: string, seq: number, status = 'pending'): OverlayThreadItem =>
  ({
    collection: 'thread',
    seq,
    id,
    doc: {
      id,
      kind: 'choice',
      from: 'alpha',
      body: 'Ship it?',
      created_at: 900 + seq,
      choice: { prompt: 'Ship it?', options: [{ label: 'Ship' }, { label: 'Wait' }], allow_custom: false, status },
    },
  }) as unknown as OverlayThreadItem

function view(over: Partial<ZenThreadView> = {}): ZenThreadView {
  return {
    address: 'alpha::local',
    phase: 'ready',
    note: null,
    items: [text('m1', 1), text('m5', 5)],
    loaded: true,
    hasMore: false,
    loadingOlder: false,
    error: null,
    viewer: null,
    conversationId: 'conv-1',
    windowServer: true,
    ...over,
  }
}

function entry(over: Partial<UnreadEntry> = {}): UnreadEntry {
  return { kind: 'thread', id: 'conv-1', projectId: 'p1', lastReadSeq: 3, lastSeq: 5, unread: 2, waiting: 0, muted: false, mutedUntil: null, ...over }
}

function seed(entries: UnreadEntry[]): void {
  const byKey: Record<string, UnreadEntry> = {}
  for (const e of entries) byKey[`${e.kind}:${e.id}`] = e
  useUnreadStore.setState({ byKey, total: 0, waitingTotal: 0, loaded: true })
}

const OPTS: ZenConversationOptions = { compose: false, attachments: false, loadOlder: false }

function el(sel: string): HTMLElement {
  const found = document.querySelector<HTMLElement>(sel)
  if (!found) throw new Error(`missing ${sel}`)
  return found
}

async function flush(): Promise<void> {
  await act(async () => {
    await Promise.resolve()
    await Promise.resolve()
  })
}

let focus: ReturnType<typeof vi.spyOn>

beforeEach(() => {
  __resetUnreadForTests()
  h.posts.length = 0
  h.supports = true
  h.muted = false
  focus = vi.spyOn(document, 'hasFocus').mockReturnValue(true)
})

afterEach(() => {
  cleanup()
  focus.mockRestore()
  __resetUnreadForTests()
})

describe('Agents: unread on rows and pins (`unread`)', () => {
  const rows = (): ZenAgentRow[] => [
    row('alpha', { unread: { unread: 3, waiting: 0, muted: false } }),
    row('pinned', { pinned: true, unread: { unread: 120, waiting: 2, muted: false } }),
    row('quiet', { unread: { unread: 4, waiting: 0, muted: true } }),
    row('read'),
  ]

  it('on by default: list rows and pin pictures carry the counts; muted is grey', () => {
    render(<ZenAgentsWidget bridge={fakeBridge(rows()).bridge} decl={agentsDecl({ pins: true })} />)
    const alpha = el('[data-zen-agent-row="alpha::local"]')
    expect(alpha.querySelector('[data-zen-unread-count]')?.getAttribute('data-zen-unread-count')).toBe('3')
    const pin = el('[data-zen-pin="pinned::local"]')
    expect(pin.querySelector('[data-zen-pin-unread] [data-zen-unread-count]')?.getAttribute('data-zen-unread-count')).toBe('99+')
    expect(pin.querySelector('[data-zen-unread-waiting]')?.getAttribute('data-zen-unread-waiting')).toBe('2')
    const quiet = el('[data-zen-agent-row="quiet::local"]').querySelector('[data-zen-unread-count]')
    expect(quiet?.getAttribute('data-muted')).toBe('true')
    expect(el('[data-zen-agent-row="read::local"]').querySelector('[data-zen-unread]')).toBeNull()
  })

  it('`unread = false`: no counts anywhere', () => {
    render(<ZenAgentsWidget bridge={fakeBridge(rows()).bridge} decl={agentsDecl({ pins: true, unread: false })} />)
    expect(document.querySelectorAll('[data-zen-unread]').length).toBe(0)
    expect(document.querySelectorAll('[data-zen-pin-unread]').length).toBe(0)
  })
})

describe('Conversation: read while looked at', () => {
  it('posts thread/read for the newest seq past the cursor', async () => {
    seed([entry()])
    const f = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={f.bridge} row={row('alpha')} options={OPTS} />)
    f.pushThread(view())
    await flush()
    expect(h.posts).toEqual([{ route: 'thread/read', body: { conversationId: 'conv-1', seq: 5 } }])
    expect(useUnreadStore.getState().byKey['thread:conv-1']?.unread).toBe(0)
  })

  it('never on another server, before ready, without focus, or when already read', async () => {
    const f = fakeBridge([row('alpha')])
    seed([entry()])
    const r = render(<ZenConversation bridge={f.bridge} row={row('alpha')} options={OPTS} />)
    f.pushThread(view({ windowServer: false }))
    f.pushThread(view({ phase: 'opening' }))
    await flush()
    r.unmount()
    focus.mockReturnValue(false)
    const g = fakeBridge([row('alpha')])
    const r2 = render(<ZenConversation bridge={g.bridge} row={row('alpha')} options={OPTS} />)
    g.pushThread(view())
    await flush()
    r2.unmount()
    focus.mockReturnValue(true)
    seed([entry({ lastReadSeq: 5, unread: 0 })])
    const k = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={k.bridge} row={row('alpha')} options={OPTS} />)
    k.pushThread(view())
    await flush()
    expect(h.posts).toEqual([])
  })
})

describe('Conversation: cards waiting on you', () => {
  it('"1 waiting" counts the pending card in view; a click scrolls to it', async () => {
    const scroll = vi.fn()
    Element.prototype.scrollIntoView = scroll
    const f = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={f.bridge} row={row('alpha')} options={OPTS} />)
    f.pushThread(view({ items: [text('m1', 1), choice('c2', 2), choice('c3', 3, 'answered')], windowServer: false }))
    const chip = el('[data-zen-waiting]')
    expect(chip.getAttribute('data-zen-waiting')).toBe('1')
    expect(chip.textContent).toBe('1 waiting')
    fireEvent.click(chip)
    expect(scroll).toHaveBeenCalledTimes(1)
    const target = scroll.mock.contexts[0] as Element
    expect(target.closest('[data-zen-item]')?.getAttribute('data-zen-item')).toBe('c2')
  })

  it('the daemon’s waiting wins when it knows of more (older cards); none, no chip', async () => {
    seed([entry({ waiting: 3, unread: 0, lastReadSeq: 5 })])
    const f = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={f.bridge} row={row('alpha')} options={OPTS} />)
    f.pushThread(view({ items: [choice('c2', 2)] }))
    expect(el('[data-zen-waiting]').getAttribute('data-zen-waiting')).toBe('3')
    cleanup()
    seed([])
    const g = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={g.bridge} row={row('alpha')} options={OPTS} />)
    g.pushThread(view())
    expect(document.querySelector('[data-zen-waiting]')).toBeNull()
  })
})

describe('Conversation: the mute bell (`mute`)', () => {
  it('Garden 1: mutes and unmutes the shown Thread', async () => {
    seed([entry({ lastReadSeq: 5, unread: 2 })])
    const f = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={f.bridge} row={row('alpha')} options={{ ...OPTS, mute: true }} />)
    f.pushThread(view())
    const bell = el('[data-zen-mute]')
    expect(bell.getAttribute('data-zen-mute')).toBe('off')
    expect(bell.getAttribute('aria-pressed')).toBe('false')
    await act(async () => {
      fireEvent.click(bell)
    })
    await flush()
    expect(h.posts.filter((p) => p.route === 'thread/mute')).toEqual([{ route: 'thread/mute', body: { kind: 'thread', id: 'conv-1', muted: true } }])
    expect(el('[data-zen-mute]').getAttribute('data-zen-mute')).toBe('on')
    await act(async () => {
      fireEvent.click(el('[data-zen-mute]'))
    })
    await flush()
    expect(h.posts.filter((p) => p.route === 'thread/mute').map((p) => p.body.muted)).toEqual([true, false])
  })

  it('no bell without the prop, on another server, or on a server without thread-read-v1', () => {
    const f = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={f.bridge} row={row('alpha')} options={OPTS} />)
    f.pushThread(view())
    expect(document.querySelector('[data-zen-mute]')).toBeNull()
    cleanup()
    const g = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={g.bridge} row={row('alpha')} options={{ ...OPTS, mute: true }} />)
    g.pushThread(view({ windowServer: false }))
    expect(document.querySelector('[data-zen-mute]')).toBeNull()
    cleanup()
    h.supports = false
    const k = fakeBridge([row('alpha')])
    render(<ZenConversation bridge={k.bridge} row={row('alpha')} options={{ ...OPTS, mute: true }} />)
    k.pushThread(view())
    expect(document.querySelector('[data-zen-mute]')).toBeNull()
  })
})

describe('Nav rail: Agents and Projects badges', () => {
  it('draws the counts app.badges gives; waiting uses the needs-you colour', () => {
    const badges = {
      tickets: { badge: null, stale: false, title: '' },
      agents: { badge: 4, waiting: true },
      projects: { badge: 120, waiting: false },
    }
    const decl = { id: 'nav', kind: 'nav-rail', column: 0, props: {}, caps: ['app:navigate'], source: 'builtin' } as unknown as ZenWidgetDecl
    render(<ZenNavRailWidget bridge={fakeBridge([], badges).bridge} decl={decl} />)
    const agents = el('[data-zen-nav-count="agents"]')
    expect(agents.textContent).toBe('4')
    expect(agents.getAttribute('data-waiting')).toBe('true')
    expect(el('[data-zen-nav-count="projects"]').textContent).toBe('99+')
  })

  it('zenAppBadges: Threads (unmuted unread + waiting) on Agents, project chats on Projects', () => {
    expect(zenAppBadges().agents).toBeUndefined()
    seed([
      entry({ id: 't1', unread: 2, waiting: 1 }),
      entry({ id: 't2', unread: 5, muted: true }),
      { kind: 'project', id: 'grp', projectId: null, lastReadSeq: 0, lastSeq: 3, unread: 3, waiting: 0, muted: false, mutedUntil: null },
    ])
    const b = zenAppBadges()
    expect(b.agents).toEqual({ badge: 3, waiting: true })
    expect(b.projects).toEqual({ badge: 1, waiting: false })
  })
})
