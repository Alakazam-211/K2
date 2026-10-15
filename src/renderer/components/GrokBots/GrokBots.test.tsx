// @vitest-environment jsdom
//
// Local Grok Bots (0.45.7) in the renderer: the Agents sidebar group shows
// each Grok Bot with the "Grok Bot" tag; right-click → Add to Home writes a
// `grokbot-<id>::<host>` row through the existing Add to Home flow; a Home
// row opens the chat; the chat is read-only (with a "Sending is off" note)
// unless the server turned Send on, then it posts to `grokbots/send`.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ label: 'main', listen: async () => () => undefined }),
}))

const ID = '0e5f5de8-7619-4ba4-9753-32c5470b2346'
const ID2 = '7a1b2c3d-0000-4000-8000-000000000002'

const cli = vi.hoisted(() => ({
  list: {} as Record<string, unknown>,
  thread: {} as Record<string, unknown>,
  gets: [] as Array<{ route: string; params: unknown }>,
  posts: [] as Array<{ route: string; body: unknown }>,
}))
vi.mock('@/lib/daemon-cli', async (orig) => ({
  ...(await orig<typeof import('@/lib/daemon-cli')>()),
  daemonCliGet: vi.fn(async (_scope: unknown, route: string, params?: unknown) => {
    cli.gets.push({ route, params })
    if (route === 'grokbots/list') return cli.list
    if (route === 'grokbots/thread') return cli.thread
    throw new Error(`unexpected GET ${route}`)
  }),
  daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
    cli.posts.push({ route, body })
    if (route === 'grokbots/send') return { ok: true, clientNonce: 'n' }
    throw new Error(`unexpected POST ${route}`)
  }),
}))
const menu = vi.hoisted(() => ({ pick: null as string | null, items: [] as Array<{ id: string; label: string }> }))
vi.mock('@/lib/context-menu', () => ({
  showContextMenu: vi.fn(async (items: Array<{ id: string; label: string }>) => {
    menu.items = items
    return menu.pick
  }),
}))
vi.mock('@/lib/home-avatars', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-avatars')>()),
  putHomeAvatarOnAdd: vi.fn(),
}))

import { GrokBotSidebarSection, GrokBotHomeRow } from './GrokBotRows'
import GrokBotChatView from './GrokBotChatView'
import { useGrokBotsStore } from '@/stores/grokbots'
import { useHomesStore } from '@/stores/homes'
import { useConnectHostStore } from '@/stores/connect-host'
import { openHomeRow } from '@/lib/home-open'
import { ADD_TO_HOME_MENU_ID } from '@/stores/add-to-home-dialog'

const item = (id: string, name: string, over: Record<string, unknown> = {}): Record<string, unknown> => ({
  id,
  kind: 'grokbot',
  name,
  title: 'Assistant',
  source: 'gateway',
  working: false,
  needsYou: false,
  unreadCount: 0,
  ...over,
})

function listWith(over: Record<string, unknown> = {}): Record<string, unknown> {
  return {
    ok: true,
    enabled: true,
    sendEnabled: false,
    installed: true,
    status: { reachable: true, busy: false },
    agents: [item(ID, 'Cortana', { working: true }), item(ID2, 'Jarvis')],
    ...over,
  }
}

const THREAD = {
  ok: true,
  supported: true,
  nextSince: 7,
  hasMoreBefore: false,
  messages: [
    { seq: 1, author: 'human', text: 'Can you check the build?', atMs: 1_791_990_000_000 },
    { seq: 2, author: 'agent', text: 'Build is green.', atMs: 1_791_990_060_000 },
    {
      seq: 3,
      author: 'agent',
      text: '',
      openInApp: 'widget',
      card: {
        type: 'choice',
        prompt: 'Which environment?',
        options: [{ label: 'Staging', chosen: true }, { label: 'Production' }],
        state: 'answered',
      },
    },
    {
      seq: 4,
      author: 'agent',
      text: '',
      openInApp: 'secret request',
      card: { type: 'secret-request', label: 'Bot token', provided: true },
    },
  ],
}

beforeEach(() => {
  cli.list = listWith()
  cli.thread = THREAD
  cli.gets = []
  cli.posts = []
  menu.pick = null
  menu.items = []
  useGrokBotsStore.setState({ byHost: {}, changeSeq: {}, open: null })
  useConnectHostStore.setState({ activeHost: 'local', connectionStatus: 'connected' })
  useHomesStore.setState({ homes: [{ id: 'h1', name: 'Home', rows: [] }], selectedId: 'h1' })
})

afterEach(() => cleanup())

describe('Agents sidebar', () => {
  it('lists Grok Bots with the Grok Bot tag and their status', async () => {
    render(<GrokBotSidebarSection />)
    await screen.findByTestId('grokbot-section')
    expect(screen.getAllByTestId('grokbot-tag')).toHaveLength(2)
    expect(screen.getByText('Cortana')).toBeTruthy()
    const states = screen.getAllByTestId('grokbot-state').map((e) => e.getAttribute('data-state'))
    expect(states).toEqual(['working', 'idle'])
  })

  it('shows nothing while Show is off on the server', async () => {
    cli.list = { ok: true, enabled: false, agents: [] }
    render(<GrokBotSidebarSection />)
    await waitFor(() => expect(useGrokBotsStore.getState().byHost.local?.list?.enabled).toBe(false))
    expect(screen.queryByTestId('grokbot-section')).toBeNull()
  })

  it('right-click → Add to Home adds a grokbot row; the menu has no workspace items', async () => {
    menu.pick = ADD_TO_HOME_MENU_ID
    render(<GrokBotSidebarSection />)
    const row = await screen.findByText('Cortana')
    await act(async () => {
      fireEvent.contextMenu(row)
    })
    await waitFor(() => expect(useHomesStore.getState().homes[0].rows).toHaveLength(1))
    expect(menu.items.map((i) => i.id)).toEqual([ADD_TO_HOME_MENU_ID])
    expect(useHomesStore.getState().homes[0].rows[0]).toMatchObject({
      address: `grokbot-${ID}::local`,
      label: 'Cortana',
    })
  })

  it('clicking a row opens its chat (no workspace selected)', async () => {
    render(<GrokBotSidebarSection />)
    fireEvent.click(await screen.findByText('Jarvis'))
    expect(useGrokBotsStore.getState().open).toEqual({ hostKey: 'local', agentId: ID2 })
  })
})

describe('row hover text', () => {
  it("uses Grok Bot's current preview, not this computer's last line", async () => {
    cli.list = listWith({
      agents: [item(ID, 'Grok', { stale: true, lastMessagePreview: 'Shipped the October report' })],
    })
    render(<GrokBotSidebarSection />)
    await screen.findByText('Grok')
    const row = document.querySelector(`[data-grokbot-row="${ID}"] [title]`) as HTMLElement
    expect(row.getAttribute('title')).toBe('Grok — Assistant\nShipped the October report')
  })
})

describe('My Home', () => {
  it('a Grok Bot Home row shows the tag and live status and opens the chat', async () => {
    const row = { address: `grokbot-${ID}::local`, workspaceId: ID, label: 'Cortana' }
    const home = { id: 'h1', name: 'Home', rows: [row] }
    render(<GrokBotHomeRow home={home} row={row} index={0} />)
    await waitFor(() => expect(screen.getByTestId('grokbot-state').getAttribute('data-state')).toBe('working'))
    expect(screen.getByTestId('grokbot-tag')).toBeTruthy()
    fireEvent.click(screen.getByText('Cortana'))
    expect(useGrokBotsStore.getState().open).toEqual({ hostKey: 'local', agentId: ID })
  })

  it('openHomeRow (⌘1–9, the rail, Garden) opens a Grok Bot row as its chat', () => {
    expect(openHomeRow({ address: `grokbot-${ID2}::local`, workspaceId: null, label: 'Jarvis' })).toBe('selected')
    expect(useGrokBotsStore.getState().open).toEqual({ hostKey: 'local', agentId: ID2 })
  })
})

describe('chat view', () => {
  it('is read-only with a "Sending is off" note while Send is off; cards render read-only', async () => {
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    await screen.findByText('Build is green.')
    expect(screen.getByTestId('grokbot-send-off').textContent).toContain('Settings → K2 Server → Local Grok Bots')
    expect(screen.queryByTestId('grokbot-compose')).toBeNull()
    expect(screen.getAllByTestId('grokbot-message').map((e) => e.getAttribute('data-author'))).toEqual([
      'human',
      'agent',
      'agent',
      'agent',
    ])
    const choice = screen.getByTestId('grokbot-card-choice')
    expect(choice.textContent).toContain('Which environment?')
    expect(choice.querySelectorAll('button:disabled')).toHaveLength(2)
    expect(screen.getByTestId('grokbot-card-secret').textContent).toContain('Provided in Grok Bot')
    expect(screen.getByTestId('grokbot-chat-status').textContent).toContain('Working')
    expect(cli.posts).toEqual([])
  })

  it('shows a compose box when Send is on and posts as the person', async () => {
    cli.list = listWith({ sendEnabled: true })
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    const input = (await screen.findByTestId('grokbot-compose-input')) as HTMLTextAreaElement
    // Cortana is working: a gentle note, sending still allowed.
    expect(screen.getByTestId('grokbot-compose').textContent).toContain('is busy')
    fireEvent.change(input, { target: { value: 'ship it' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('grokbot-compose-send'))
    })
    expect(cli.posts).toEqual([{ route: 'grokbots/send', body: { agent: ID, text: 'ship it' } }])
  })

  it('a line sent from K2 shows its sender as the author, without the [from …] stamp', async () => {
    cli.thread = {
      ...THREAD,
      messages: [
        { seq: 1, author: 'human', text: '[from Rosson Test] ship it', atMs: 1_791_990_000_000, viaK2: true },
        { seq: 2, author: 'human', text: '[from Typed] by hand', atMs: 1_791_990_010_000 },
      ],
    }
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    await screen.findByText('ship it')
    expect(screen.queryByText('[from Rosson Test] ship it')).toBeNull()
    const [k2, typed] = screen.getAllByTestId('grokbot-message')
    expect(k2.textContent).toContain('Rosson Test')
    expect(k2.textContent).toContain('sent from K2')
    expect(k2.textContent).not.toContain('You')
    // Typed in Grok Bot itself: shown as stored, author "You".
    expect(typed.textContent).toContain('[from Typed] by hand')
    expect(typed.textContent).toContain('You')
  })

  it('a stale bot shows the banner, the old history, then the live tail under a divider, and still sends', async () => {
    cli.list = listWith({
      sendEnabled: true,
      agents: [
        item(ID, 'Grok', {
          stale: true,
          lastMessagePreview: 'Done: report sent.',
          lastActivityAt: Date.now() - 2 * 3600_000,
          localActivityAt: Date.UTC(2026, 7, 30, 12),
        }),
      ],
    })
    cli.thread = {
      ...THREAD,
      stale: true,
      liveTail: {
        sinceMs: Date.now() - 3 * 3600_000,
        entries: [
          { id: 'r1', author: 'agent', text: 'Shipped the October report.', atMs: Date.now() - 3 * 3600_000 },
          { id: 'k2-n1', author: 'human', text: '[from Rosson Test] send it to the board', atMs: Date.now() - 2.5 * 3600_000, viaK2: true },
          { id: 'r2', author: 'agent', text: 'Done: report sent.', atMs: Date.now() - 2 * 3600_000, gapBefore: true },
        ],
      },
    }
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    const banner = await screen.findByTestId('grokbot-stale-banner')
    expect(banner.textContent).toContain("Grok's latest conversation runs outside this computer.")
    expect(banner.textContent).toContain('Showing history up to')
    expect(screen.getByTestId('grokbot-stale-preview').textContent).toContain('Done: report sent.')
    // Old local history first, then the divider, then the live lines.
    await screen.findByText('Build is green.')
    const divider = screen.getByTestId('grokbot-live-divider')
    expect(divider.textContent).toMatch(/^Live since .+ — older history above may be out of date$/)
    const all = screen.getAllByTestId('grokbot-message')
    expect(all).toHaveLength(7)
    const live = screen.getAllByTestId('grokbot-live-entry')
    expect(live.map((e) => e.getAttribute('data-live-id'))).toEqual(['r1', 'k2-n1', 'r2'])
    const history = screen.getByText('Build is green.')
    expect(history.compareDocumentPosition(divider) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect(divider.compareDocumentPosition(live[0]) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    // Normal bubbles: the agent's reply, and K2's own line with its sender.
    expect(live[0].textContent).toContain('Shipped the October report.')
    expect(live[1].textContent).toContain('Rosson Test')
    expect(live[1].textContent).toContain('send it to the board')
    expect(live[1].textContent).toContain('sent from K2')
    expect(live[1].textContent).not.toContain('[from Rosson Test]')
    // A gap marker only where lines may be missing.
    const gaps = screen.getAllByTestId('grokbot-live-gap')
    expect(gaps).toHaveLength(1)
    expect(live[2].contains(gaps[0])).toBe(true)
    expect(gaps[0].textContent).toBe('… earlier messages not shown — open Grok Bot')

    // The compose box works, with a note about where replies come from.
    expect(screen.queryByTestId('grokbot-compose-stale')).toBeNull()
    expect(screen.getByTestId('grokbot-compose-stale-note').textContent).toBe(
      "Replies come from Grok Bot's live conversation; older history on this computer may be out of date.",
    )
    const input = screen.getByTestId('grokbot-compose-input') as HTMLTextAreaElement
    expect(input.disabled).toBe(false)
    fireEvent.change(input, { target: { value: 'thanks' } })
    await act(async () => {
      fireEvent.click(screen.getByTestId('grokbot-compose-send'))
    })
    expect(cli.posts).toEqual([{ route: 'grokbots/send', body: { agent: ID, text: 'thanks' } }])
  })

  it("a new live line from the server's change event shows up", async () => {
    cli.list = listWith({ agents: [item(ID, 'Grok', { stale: true })] })
    const entry = (id: string, text: string): Record<string, unknown> => ({ id, author: 'agent', text, atMs: Date.now() })
    cli.thread = { ...THREAD, stale: true, liveTail: { sinceMs: Date.now(), entries: [entry('r1', 'first')] } }
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    await screen.findByText('first')
    cli.thread = {
      ...THREAD,
      stale: true,
      liveTail: { sinceMs: Date.now(), entries: [entry('r1', 'first'), entry('r2', 'second')] },
    }
    act(() => useGrokBotsStore.getState().noteChange('local'))
    await screen.findByText('second')
    expect(screen.getAllByTestId('grokbot-live-entry')).toHaveLength(2)
  })

  it('a current bot has no stale banner', async () => {
    cli.list = listWith({ sendEnabled: true })
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    await screen.findByTestId('grokbot-compose')
    expect(screen.queryByTestId('grokbot-stale-banner')).toBeNull()
    expect(screen.queryByTestId('grokbot-compose-stale-note')).toBeNull()
    expect(screen.queryByTestId('grokbot-live-divider')).toBeNull()
  })

  it('an unknown chat layout says "Open it in Grok Bot" instead of breaking', async () => {
    cli.thread = { ok: true, supported: false, reason: 'layout K2 does not know', messages: [], nextSince: 0 }
    render(<GrokBotChatView hostKey="local" agentId={ID} />)
    expect((await screen.findByTestId('grokbot-unsupported')).textContent).toContain('Open it in Grok Bot')
  })
})
