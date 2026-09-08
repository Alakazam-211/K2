// @vitest-environment jsdom
//
// View-only Inbox browser (prd-view-only-inbox-browser-v1 + vs-live I11–I24).
// Tab titles Chat/Inbox vs Work Board stay locked in stores/tabs.test.ts.

import { describe, it, expect, beforeEach, vi } from 'vitest'
import { render, screen, fireEvent, waitFor, cleanup } from '@testing-library/react'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import {
  EXTERNAL_EMAIL_BEGIN,
  EXTERNAL_EMAIL_END,
  MAIL_PAGE_LIMIT,
} from './inbox-browser'

const dir = dirname(fileURLToPath(import.meta.url))

const h = vi.hoisted(() => {
  const openFileAsTab = vi.fn()
  const invoke = vi.fn()
  const daemonCliGet = vi.fn<(route: string, params?: Record<string, unknown>) => Promise<unknown>>()
  const daemonCliPost = vi.fn()
  return { openFileAsTab, invoke, daemonCliGet, daemonCliPost }
})

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: h.daemonCliGet,
  daemonCliPost: h.daemonCliPost,
  isHostSwitchedError: () => false,
}))

vi.mock('@/lib/workspace-agent', () => ({
  agentDisplayName: vi.fn(async () => 'Sales'),
}))

vi.mock('@tauri-apps/api/event', () => ({
  listen: vi.fn(async () => () => {}),
}))

vi.mock('@tauri-apps/api/core', () => ({
  invoke: h.invoke,
}))

vi.mock('@/stores/tabs', () => ({
  useTabsStore: { getState: () => ({ openFileAsTab: h.openFileAsTab }) },
}))

vi.mock('@/components/Markdown/Markdown', () => ({
  default: ({ children }: { children: string }) => (
    <div data-testid="markdown">{children}</div>
  ),
}))

import { AgentInboxPane } from './AgentInboxPane'

const TRAY_ITEM = {
  id: 'pkg-1',
  filename: 'pkg-1.md',
  folder: '',
  title: 'Wake package',
  priority: 'normal',
  created: '2026-09-08',
  source: 'cli',
  from: 'user',
  bodyPreview: 'please look',
}

const HOSTED = { address: 'you@host.k2', source: 'hosted' }
const LINKED = { address: 'you@gmail.com', source: 'linked' }
const POSTAL = { address: 'postal-bot', source: 'postal' }

const MAIL_ROW = {
  id: 'mail-1',
  address: 'you@host.k2',
  from: { name: 'Ada', address: 'ada@ex.com' },
  subject: 'Hello host',
  date: '2026-09-08T12:00:00Z',
  unread: true,
}

function mockDaemon(opts?: {
  inboxes?: unknown
  catalogThrow?: Error
  trayList?: unknown
  trayThrow?: Error
  trayContent?: unknown
  bodyThrow?: Error
  mailMessages?: unknown
  mailThrow?: Error
  mailRead?: unknown
  folders?: unknown
}) {
  h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, unknown>) => {
    if (route === 'mail/inboxes') {
      if (opts?.catalogThrow) throw opts.catalogThrow
      return opts?.inboxes ?? { ok: true, count: 0, inboxes: [] }
    }
    if (route === 'inbox/list') {
      if (opts?.trayThrow) throw opts.trayThrow
      return opts?.trayList ?? [TRAY_ITEM]
    }
    if (route === 'inbox/folders') return opts?.folders ?? ['active', 'done']
    if (route === 'inbox/read') {
      if (opts?.bodyThrow) throw opts.bodyThrow
      return opts?.trayContent ?? { id: params?.id, content: '# tray body' }
    }
    if (route === 'mail/messages') {
      if (opts?.mailThrow) throw opts.mailThrow
      return opts?.mailMessages ?? { ok: true, count: 1, messages: [MAIL_ROW] }
    }
    if (route === 'mail/read') {
      if (opts?.bodyThrow) throw opts.bodyThrow
      return opts?.mailRead ?? {
        ok: true,
        message: {
          id: MAIL_ROW.id,
          subject: MAIL_ROW.subject,
          from: MAIL_ROW.from,
          date: MAIL_ROW.date,
          text: `${EXTERNAL_EMAIL_BEGIN}\nplain body\n${EXTERNAL_EMAIL_END}`,
          html: `${EXTERNAL_EMAIL_BEGIN}\n<p>html body</p>\n${EXTERNAL_EMAIL_END}`,
        },
      }
    }
    throw new Error(`unexpected route ${route}`)
  })
}

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openFileAsTab.mockReset()
  h.invoke.mockReset()
  mockDaemon()
})

function calls(route: string): unknown[][] {
  return h.daemonCliGet.mock.calls.filter((c) => c[0] === route)
}

describe('AgentInboxPane module locks', () => {
  it('does not call Tauri inbox shims, openFileAsTab, unsandboxed HTML, or Settings owner catalog', () => {
    const pane = readFileSync(join(dir, 'AgentInboxPane.tsx'), 'utf8')
    const helpers = readFileSync(join(dir, 'inbox-browser.ts'), 'utf8')
    for (const src of [pane, helpers]) {
      expect(src).not.toMatch(/k2so_inbox_/)
      expect(src).not.toMatch(/openFileAsTab/)
      expect(src).not.toMatch(/dangerouslySetInnerHTML/)
      expect(src).not.toMatch(/sandbox="allow-scripts"/)
      expect(src).not.toMatch(/fetchInboxes\s*\(/)
      expect(src).not.toMatch(/from '@\/components\/Feedback/)
      expect(src).not.toMatch(/from '@\/stores\/feedback/)
    }
    expect(pane).not.toMatch(/invoke\(/)
    expect(pane).toMatch(/sandbox=""/)
  })
})

describe('AgentInboxPane sources + tray', () => {
  it('always shows K2 tray, loads workspace catalog with project=, omits Postal', async () => {
    mockDaemon({ inboxes: { ok: true, inboxes: [HOSTED, LINKED, POSTAL] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)

    await waitFor(() => {
      expect(screen.getByTestId('inbox-source-tray').textContent).toContain('K2 tray')
    })
    expect(screen.getByTestId('inbox-source-mail-you@host.k2').textContent).toContain('Hosted')
    expect(screen.getByTestId('inbox-source-mail-you@gmail.com').textContent).toContain('Linked')
    expect(screen.queryByText('Postal')).toBeNull()
    expect(screen.queryByText(/postal-bot/i)).toBeNull()

    const inboxCalls = calls('mail/inboxes')
    expect(inboxCalls.length).toBeGreaterThan(0)
    for (const c of inboxCalls) {
      expect(c[1]).toEqual(expect.objectContaining({ project: '/ws' }))
    }

    await waitFor(() => {
      expect(calls('inbox/list').length).toBeGreaterThan(0)
    })
    expect(calls('inbox/list')[0][1]).toEqual(expect.objectContaining({
      project: '/ws',
      folder: '',
    }))
    expect(h.invoke).not.toHaveBeenCalled()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('keeps Work Board chrome for __workspace__ and does not retitle agents to Work Board', async () => {
    const board = render(<AgentInboxPane agentName="__workspace__" projectPath="/ws" />)
    expect(board.getByText('Work Board')).toBeTruthy()
    board.unmount()
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByText('Sales')).toBeTruthy())
    expect(screen.queryByText('Work Board')).toBeNull()
  })

  it('opens a tray package via inbox/read → Markdown content, not a file tab', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-message-pkg-1')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-message-pkg-1'))
    await waitFor(() => expect(screen.getByTestId('markdown').textContent).toContain('# tray body'))
    expect(calls('inbox/read')[0][1]).toEqual({ project: '/ws', id: 'pkg-1' })
    expect(h.openFileAsTab).not.toHaveBeenCalled()
  })

  it('filters tray folders with a single inbox/list call (no kanban columns)', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-folder-active')).toBeTruthy())
    expect(screen.queryByText('Unassigned')).toBeNull()
    expect(screen.queryByText('In Progress')).toBeNull()
    fireEvent.click(screen.getByTestId('inbox-folder-active'))
    await waitFor(() => {
      const folders = calls('inbox/list').map((c) => (c[1] as { folder?: string }).folder)
      expect(folders).toContain('active')
    })
  })

  it('empty mail catalog keeps tray and points at Settings → Email', async () => {
    mockDaemon({ inboxes: { ok: true, count: 0, inboxes: [] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByText(/Settings → Email/)).toBeTruthy())
    expect(screen.getByTestId('inbox-source-tray')).toBeTruthy()
    expect(screen.queryByTestId('inbox-source-mail-you@host.k2')).toBeNull()
  })
})

describe('AgentInboxPane mail', () => {
  it('lists mail for the selected address and opens via mail/read (marks seen)', async () => {
    mockDaemon({ inboxes: { ok: true, inboxes: [HOSTED] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-source-mail-you@host.k2')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-source-mail-you@host.k2'))
    await waitFor(() => expect(screen.getByTestId('inbox-message-mail-1')).toBeTruthy())

    const listCall = calls('mail/messages')[0]
    expect(listCall[1]).toEqual(expect.objectContaining({
      project: '/ws',
      address: 'you@host.k2',
      limit: MAIL_PAGE_LIMIT,
    }))

    fireEvent.click(screen.getByTestId('inbox-message-mail-1'))
    await waitFor(() => expect(screen.getByTestId('inbox-mail-text')).toBeTruthy())
    expect(calls('mail/read')[0][1]).toEqual(expect.objectContaining({
      project: '/ws',
      id: 'mail-1',
    }))
    expect(screen.getByText('Marks read')).toBeTruthy()
    expect(screen.getByTestId('inbox-mail-text').textContent).toContain('plain body')
    expect(screen.queryByTestId('inbox-mail-html')).toBeNull()
    expect(h.openFileAsTab).not.toHaveBeenCalled()
  })

  it('renders HTML-only mail in an iframe with empty sandbox and markers stripped', async () => {
    mockDaemon({
      inboxes: { ok: true, inboxes: [HOSTED] },
      mailRead: {
        ok: true,
        message: {
          id: 'mail-1',
          subject: 'HTML only',
          text: `${EXTERNAL_EMAIL_BEGIN}\n\n${EXTERNAL_EMAIL_END}`,
          html: `${EXTERNAL_EMAIL_BEGIN}\n<p><a href="javascript:alert(1)">x</a></p>\n${EXTERNAL_EMAIL_END}`,
        },
      },
    })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-source-mail-you@host.k2')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-source-mail-you@host.k2'))
    await waitFor(() => expect(screen.getByTestId('inbox-message-mail-1')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-message-mail-1'))
    const iframe = await screen.findByTestId('inbox-mail-html')
    expect(iframe.tagName).toBe('IFRAME')
    expect(iframe.getAttribute('sandbox')).toBe('')
    expect(iframe.getAttribute('sandbox')).not.toContain('allow-scripts')
    const srcDoc = iframe.getAttribute('srcdoc') ?? ''
    expect(srcDoc).not.toContain(EXTERNAL_EMAIL_BEGIN)
    expect(srcDoc).not.toContain(EXTERNAL_EMAIL_END)
    expect(srcDoc).not.toMatch(/javascript:/i)
    expect(srcDoc).toContain('<p>')
  })

  it('shows Load more when nextOffset is present and requests the next page', async () => {
    mockDaemon({
      inboxes: { ok: true, inboxes: [HOSTED] },
      mailMessages: { ok: true, count: 1, messages: [MAIL_ROW], nextOffset: 50 },
    })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-source-mail-you@host.k2')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-source-mail-you@host.k2'))
    const loadMore = await screen.findByTestId('inbox-load-more')
    expect(loadMore.textContent).toContain('Load more')
    fireEvent.click(loadMore)
    await waitFor(() => {
      const offsets = calls('mail/messages').map((c) => (c[1] as { offset?: number }).offset)
      expect(offsets).toContain(50)
    })
  })
})

describe('AgentInboxPane errors + view-only', () => {
  it('catalog failure is a loud one-line error, still shows K2 tray, not silent empty mail', async () => {
    mockDaemon({ catalogThrow: new Error('catalog down') })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-catalog-error').textContent).toContain('catalog down'))
    expect(screen.getByTestId('inbox-source-tray')).toBeTruthy()
    expect(screen.queryByText('No messages')).toBeNull()
  })

  it('mail list throw is a visible error, not a silent empty mailbox', async () => {
    mockDaemon({
      inboxes: { ok: true, inboxes: [HOSTED] },
      mailThrow: new Error(JSON.stringify({ error: { hint: 'IMAP auth failed' } })),
    })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-source-mail-you@host.k2')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-source-mail-you@host.k2'))
    await waitFor(() => expect(screen.getByTestId('inbox-list-error').textContent).toContain('IMAP auth failed'))
    expect(screen.queryByText('No messages')).toBeNull()
  })

  it('wrong tray list shape fails loud instead of unwrapping {items:[]}', async () => {
    mockDaemon({ trayList: { items: [TRAY_ITEM] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-list-error').textContent).toMatch(/inbox\/list/))
    expect(screen.queryByTestId('inbox-message-pkg-1')).toBeNull()
  })

  it('has no compose/reply/send/draft/archive/move/respond controls', async () => {
    mockDaemon({ inboxes: { ok: true, inboxes: [HOSTED] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-browser')).toBeTruthy())
    const root = screen.getByTestId('inbox-browser')
    expect(root.textContent).not.toMatch(/\b(Compose|Reply|Send|Draft|Archive|Move|Respond)\b/)
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })
})
