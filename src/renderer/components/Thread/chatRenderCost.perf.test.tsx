// @vitest-environment jsdom
//
// Chat render cost (0.46.2, ported from K2 Companion's phone-perf fixes).
//
// Measures, in jsdom, what the desktop chats cost with a long history:
//   - the Thread beside an agent (`ThreadOverlayColumn` + the real Thread
//     compose bar), 500 messages;
//   - a ticket's discussion (`ThreadTab`, the Thread look in the Tickets
//     rail), 300 comments;
//   - the project chat (`ProjectChatPanel`), 500 messages.
//
// Per surface: the work of one keystroke in the message box, of the things
// that re-render a chat while you read it (an activity frame, a parent
// re-render, a refetch that hands back equal data), and the DOM size.
//   - "rows": message bubbles rendered (a probe on `ThreadFileObjects`,
//     which every bubble renders once);
//   - "md": Markdown bodies rendered (each one is a full remark parse);
//   - "ms": React Profiler `actualDuration` summed over the commits.
// The numbers printed under "[perf]" are what the 0.46.2 report quotes.
// The assertions are the budgets the fixes hold; they fail loudly.

import { afterAll, afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow: () => ({ label: 'window-test' }) }))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))

const probe = vi.hoisted(() => ({ rows: 0, md: 0 }))

vi.mock('@/components/Markdown/Markdown', async (orig) => {
  const real = (await orig()) as { default: (p: unknown) => unknown }
  return {
    default: (p: unknown) => {
      probe.md++
      return real.default(p)
    },
  }
})
vi.mock('@/components/Thread/ThreadFileObjects', async (orig) => {
  const real = (await orig()) as Record<string, unknown> & { ThreadFileObjects: (p: unknown) => unknown }
  return {
    ...real,
    ThreadFileObjects: (p: unknown) => {
      probe.rows++
      return real.ThreadFileObjects(p)
    },
  }
})

const cli = vi.hoisted(() => ({
  get: vi.fn(async (_path: string, _q?: unknown): Promise<unknown> => ({})),
  post: vi.fn(async (_path: string, _body?: unknown): Promise<unknown> => ({})),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (_scope: unknown, path: string, q?: unknown) => cli.get(path, q),
  daemonCliPost: (_scope: unknown, path: string, body?: unknown) => cli.post(path, body),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/kessel/daemon-ws', () => ({
  getDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  getLocalDaemonWs: vi.fn(async () => {
    throw new Error('no daemon in test')
  }),
  invalidateDaemonWs: () => {},
  daemonWsBase: () => 'ws://127.0.0.1:1',
  daemonHttpBase: () => 'http://127.0.0.1:1',
}))
vi.mock('@/kessel-term/TerminalPane', () => ({ TerminalPane: () => <div data-testid="stub-terminal-pane" /> }))
const viewerRole = vi.hoisted(() => ({ current: 'owner' as 'owner' | 'member' | null }))
vi.mock('@/components/Presence/PresenceKickButton', () => ({ fetchViewerRole: vi.fn(async () => viewerRole.current) }))

// The Thread's data: a tiny store, so an activity frame re-renders the pane
// the way the real hook's `setTurn` does (not through a parent re-render).
const thread = vi.hoisted(() => {
  const listeners = new Set<() => void>()
  const state = {
    snap: {
      items: [] as unknown[],
      conversationId: 'c',
      error: null as string | null,
      posting: false,
      post: async () => {},
      answer: async () => {},
      voidCard: async () => {},
      hasMore: false,
      loadingOlder: false,
      loaded: true,
      loadOlder: async () => {},
      turn: null as unknown,
      turnsReported: true,
      currentAddr: 'sales',
      pastAddresses: [] as string[],
    },
  }
  return {
    state,
    set(patch: Record<string, unknown>) {
      state.snap = { ...state.snap, ...patch }
      for (const l of listeners) l()
    },
    subscribe(l: () => void) {
      listeners.add(l)
      return () => listeners.delete(l)
    },
  }
})
vi.mock('@/components/SessionView/useOverlayThread', async () => {
  const { useSyncExternalStore } = await import('react')
  return {
    useOverlayThread: () => useSyncExternalStore(thread.subscribe, () => thread.state.snap),
  }
})

import { act, Profiler, type ReactNode } from 'react'
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import { ThreadOverlayColumn } from '@/components/SessionView/ThreadOverlayColumn'
import { coerceThreadTurn, type OverlayThreadItem, type ThreadTurn } from '@/components/SessionView/overlayThread'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { ThreadTab } from '@/components/Feedback/TicketAgentRail'
import type { FeedbackShow } from '@/components/Feedback/feedback-api'
import ProjectChatPanel from '@/components/Projects/ProjectChatPanel'
import type { ProjectGroupMessage, ProjectGroupShow } from '@/components/Projects/projects-api'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { noteServerVersion, primaryScope } from '@/kessel/server-scope'
import { resetThreadViewersForTests, setThreadViewerForTests } from '@/lib/thread-viewer'
import { useSettingsStore } from '@/stores/settings'
import { usePageViewStore } from '@/stores/page-view'
import { useProjectGroupsStore } from '@/stores/project-groups'
import { __resetUnreadForTests } from '@/stores/unread'
import { __resetActivityForTests } from '@/stores/activity'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const NOW = Math.floor(Date.now() / 1000)

/** A chat's worth of bodies: plain lines, markdown, lists, code. */
function bodyFor(i: number): string {
  switch (i % 5) {
    case 0:
      return `Message ${i}: plain text, a sentence or two long, the way people write in a chat.`
    case 1:
      return `**Step ${i}**: ran \`cargo test -p k2-core\`\n\n- one\n- two\n- three`
    case 2:
      return `Reply ${i}. Looks good, ship it.`
    case 3:
      return `A longer one (${i}) with a [link](https://k2.dev) and some _emphasis_ across\nseveral lines\nof text.`
    default:
      return `\`\`\`\nfn main() { println!("${i}") }\n\`\`\``
  }
}

function threadItems(n: number): OverlayThreadItem[] {
  return Array.from({ length: n }, (_, i) => ({
    collection: 'thread',
    seq: i + 1,
    id: `t${i}`,
    doc:
      i % 2 === 0
        ? { id: `t${i}`, kind: 'text', from: 'sales', created_at: NOW - (n - i) * 60, body: bodyFor(i) }
        : { id: `t${i}`, kind: 'text', from: 'owner', via: 'compose', created_at: NOW - (n - i) * 60, body: bodyFor(i) },
  }))
}

function turnOf(rev: number): ThreadTurn {
  const server = Date.now()
  const t = coerceThreadTurn(
    {
      turnId: 'turn-1',
      state: 'working',
      phase: 'tool',
      phaseSince: server - 1_000,
      line: `Running step ${rev}`,
      startedAt: server - 10_000,
      since: server - 10_000,
      subagents: 0,
      subagentsDone: 0,
      background: 0,
      tally: { read: rev, search: 0, cmd: 1, edit: 0 },
      end: null,
      serverNow: server,
      rev,
    },
    server,
  )
  if (!t) throw new Error('turn fixture did not coerce')
  return t
}

// ── Measuring ──────────────────────────────────────────────────────────────

const cost = { ms: 0, commits: 0 }
function Measured({ children }: { children: ReactNode }): React.JSX.Element {
  return (
    <Profiler
      id="chat"
      onRender={(_id, _phase, actual) => {
        cost.ms += actual
        cost.commits++
      }}
    >
      {children}
    </Profiler>
  )
}

interface Sample {
  rows: number
  md: number
  ms: number
}

async function measure(fn: () => void | Promise<void>): Promise<Sample> {
  probe.rows = 0
  probe.md = 0
  cost.ms = 0
  cost.commits = 0
  await act(async () => {
    await fn()
  })
  return { rows: probe.rows, md: probe.md, ms: Math.round(cost.ms * 10) / 10 }
}

function dom(root: ParentNode): { rows: number; nodes: number } {
  return {
    rows: root.querySelectorAll('[data-zen-message]').length,
    nodes: root.querySelectorAll('*').length,
  }
}

const report: string[] = []
function note(surface: string, what: string, v: Record<string, number>): void {
  const line = `[perf] ${surface} · ${what}: ${Object.entries(v)
    .map(([k, n]) => `${k}=${n}`)
    .join(' ')}`
  report.push(line)
  console.log(line)
}

afterAll(() => {
  console.log(['', '[perf] summary', ...report].join('\n'))
})

async function flush(): Promise<void> {
  for (let i = 0; i < 5; i++) await act(async () => new Promise((r) => setTimeout(r, 0)))
}

beforeEach(() => {
  localStorage.clear()
  __resetActivityForTests()
  __resetUnreadForTests()
  cli.get.mockReset()
  cli.post.mockReset()
  cli.get.mockImplementation(async (path: string) => (path === 'terminal/compose-history' ? { items: [] } : {}))
  cli.post.mockImplementation(async () => ({}))
  useSettingsStore.setState({ allowRemoteInstruct: true })
})

afterEach(() => {
  cleanup()
})

// ── The Thread ─────────────────────────────────────────────────────────────

describe('the Thread beside an agent (500 messages)', () => {
  const room = testRoom({
    tabs: {},
    scope: primaryScope(),
    projects: [{ id: 'p-sales', name: 'sales', path: '/ws/sales', allowRemoteInstruct: 1 } as never],
    activeProjectId: 'p-sales',
  })

  beforeEach(() => {
    setThreadViewerForTests(room.scope, { principal: 'owner', aliases: [] })
    thread.set({ items: threadItems(500), turn: null, hasMore: true })
  })
  afterEach(() => {
    resetThreadViewersForTests()
  })

  function column(onStop: () => void): React.JSX.Element {
    return (
      <Measured>
        <ThreadOverlayColumn
          addr="sales"
          conversationId="c"
          active
          composeBar={<TerminalComposeBar sessionId="sess-1" workspacePath="/ws/sales" sendDestination="thread" />}
          agentName="Sales"
          onStop={onStop}
        />
      </Measured>
    )
  }

  it('a keystroke, an activity frame and a parent re-render cost no message rows', async () => {
    let view!: ReturnType<typeof renderInRoom>
    const open = await measure(() => {
      view = renderInRoom(room, column(() => {}))
    })
    await flush()
    const d = dom(view.container)
    note('Thread 500', 'open', { ...open, domRows: d.rows, domNodes: d.nodes })

    const input = view.container.querySelector('textarea') as HTMLTextAreaElement
    expect(input).not.toBeNull()
    const key = await measure(() => {
      fireEvent.change(input, { target: { value: 'h' } })
    })
    note('Thread 500', 'one keystroke', key)

    const frames = await measure(() => {
      thread.set({ turn: turnOf(1) })
      thread.set({ turn: turnOf(2) })
    })
    note('Thread 500', 'two activity frames', frames)

    // TerminalPane re-renders hand the column a new Stop and a new bar.
    const parent = await measure(() => {
      view.rerender(column(() => {}))
    })
    note('Thread 500', 'parent re-render', parent)

    // A re-read (the socket reconnecting) hands back equal, new objects.
    const reread = await measure(() => {
      thread.set({ items: threadItems(500) })
    })
    note('Thread 500', 'equal re-read', reread)

    expect(key.rows).toBe(0)
    expect(key.md).toBe(0)
    expect(frames.rows).toBe(0)
    expect(frames.md).toBe(0)
    expect(parent.rows).toBe(0)
    expect(parent.md).toBe(0)
    expect(reread.rows).toBe(0)
    expect(reread.md).toBe(0)
    expect(d.rows).toBeLessThanOrEqual(50)
  })
})

// ── A ticket's discussion ──────────────────────────────────────────────────

function ticket(n: number): FeedbackShow {
  return {
    id: 'aaaaaaaa-0000-4000-8000-000000000001',
    projectId: 'p1',
    sessionId: null,
    sessionKind: null,
    agentName: 'scout',
    kind: 'question',
    title: 'Which DB?',
    body: 'Pick one.',
    options: [],
    priority: 3,
    status: 'waiting',
    answer: null,
    createdAt: NOW - 120,
    updatedAt: NOW - 120,
    answeredAt: null,
    commentCount: n,
    assignees: ['owner'],
    workspace: 'Alpha',
    projectPath: '/ws',
    canonicalSessionId: null,
    comments: Array.from({ length: n }, (_, i) => ({
      author: i % 2 === 0 ? 'scout' : 'owner',
      body: bodyFor(i),
      at: NOW - (n - i) * 60,
    })),
  } as FeedbackShow
}

describe("a ticket's discussion (300 comments)", () => {
  function Rail({ item, nowSec }: { item: FeedbackShow; nowSec: number }): React.JSX.Element {
    return (
      <Measured>
        <ThreadTab
          item={item}
          error={null}
          nowSec={nowSec}
          ticketId={item.id}
          options={[]}
          optionsLive
          onChanged={() => {}}
        />
      </Measured>
    )
  }

  it('a keystroke and an equal refetch cost no message rows', async () => {
    let view!: ReturnType<typeof render>
    const open = await measure(() => {
      view = render(<Rail item={ticket(300)} nowSec={NOW} />)
    })
    await flush()
    const d = dom(view.container)
    note('Ticket 300', 'open', { ...open, domRows: d.rows, domNodes: d.nodes })

    const input = screen.getByTestId('ticket-compose-input') as HTMLTextAreaElement
    const key = await measure(() => {
      fireEvent.change(input, { target: { value: 'h' } })
    })
    note('Ticket 300', 'one keystroke', key)

    // FeedbackItemView refetches on every ticket event: new, equal objects.
    const refetch = await measure(async () => {
      view.rerender(<Rail item={ticket(300)} nowSec={NOW} />)
      await Promise.resolve()
    })
    const second = await measure(() => {
      view.rerender(<Rail item={ticket(300)} nowSec={NOW} />)
    })
    refetch.rows += second.rows
    refetch.md += second.md
    refetch.ms = Math.round((refetch.ms + second.ms) * 10) / 10
    note('Ticket 300', 'two equal refetches', refetch)

    expect(key.rows).toBe(0)
    expect(key.md).toBe(0)
    expect(refetch.rows).toBe(0)
    expect(refetch.md).toBe(0)
    expect(d.rows).toBeLessThanOrEqual(50)
  })
})

// ── The project chat ───────────────────────────────────────────────────────

function projectShow(): ProjectGroupShow {
  return {
    id: 'g1',
    name: 'Launch',
    pocWorkspaceId: 'poc-ws',
    pinned: false,
    color: null,
    sortOrder: 0,
    createdAt: 1,
    updatedAt: 1,
    memberCount: 1,
    memberWorkspaceIds: ['poc-ws'],
    members: [{ workspaceId: 'poc-ws', name: 'ada-ws', path: '/ws/ada', agentName: 'Ada', createdAt: 1 }],
    dashboards: [],
  } as unknown as ProjectGroupShow
}

function projectMessages(n: number): ProjectGroupMessage[] {
  return Array.from({ length: n }, (_, i) => ({
    id: `m${i}`,
    groupId: 'g1',
    author: i % 2 === 0 ? 'Ada' : 'owner',
    body: bodyFor(i),
    createdAt: NOW - (n - i) * 60,
    seq: i + 1,
    authorKind: i % 2 === 0 ? 'agent' : 'person',
  })) as ProjectGroupMessage[]
}

describe('the project chat (500 messages)', () => {
  beforeEach(() => {
    noteServerVersion('local', '0.46.1', ['thread-read-v1', 'daemon-activity'])
    viewerRole.current = 'owner'
    const msgs = projectMessages(500)
    cli.get.mockImplementation(async (path: string) =>
      path === 'project-group/messages' ? { ok: true, messages: msgs, truncated: false } : {},
    )
    usePageViewStore.setState({ page: 'projects' })
    useProjectGroupsStore.setState({ selectedGroupId: 'g1', chatCollapsed: false })
  })

  it('a keystroke costs no message rows', async () => {
    let view!: ReturnType<typeof render>
    const open = await measure(async () => {
      view = render(
        <Measured>
          <ProjectChatPanel show={projectShow()} />
        </Measured>,
      )
    })
    await waitFor(() => expect(view.container.querySelectorAll('[data-zen-message]').length).toBeGreaterThan(0))
    await flush()
    const d = dom(view.container)
    note('Project 500', 'open', { ...open, domRows: d.rows, domNodes: d.nodes })

    const input = await waitFor(() => screen.getByTestId('project-chat-input') as HTMLTextAreaElement)
    const key = await measure(() => {
      fireEvent.change(input, { target: { value: 'h' } })
    })
    note('Project 500', 'one keystroke', key)

    // A chat event elsewhere bumps the revision: the panel refetches the
    // same page (new, equal objects) after its 300ms trailing window.
    const refetch = await measure(async () => {
      useProjectGroupsStore.setState((s) => ({ messageRevision: s.messageRevision + 1 }))
      await new Promise((r) => setTimeout(r, 400))
    })
    expect(cli.get.mock.calls.filter(([p]) => p === 'project-group/messages').length).toBeGreaterThanOrEqual(2)
    note('Project 500', 'equal refetch', refetch)

    expect(key.rows).toBe(0)
    expect(key.md).toBe(0)
    expect(refetch.rows).toBe(0)
    expect(refetch.md).toBe(0)
    expect(d.rows).toBeLessThanOrEqual(50)
  })
})

