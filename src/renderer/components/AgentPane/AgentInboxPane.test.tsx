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
  const activateProject = vi.fn()
  const projects = { list: [{ id: 'ws-1', path: '/ws' }] as { id: string; path: string }[] }
  return { openFileAsTab, invoke, daemonCliGet, daemonCliPost, activateProject, projects }
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

vi.mock('@/stores/projects', () => ({
  useProjectsStore: (selector: (s: { projects: { id: string; path: string }[] }) => unknown) =>
    selector({ projects: h.projects.list }),
  activateProject: h.activateProject,
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

function mockPosts(opts?: {
  ensure?: unknown
  ensureThrow?: Error
  send?: unknown
  sendThrow?: Error
}) {
  h.daemonCliPost.mockImplementation(async (route: string) => {
    if (route === 'workspace/ensure-pinned-chat') {
      if (opts?.ensureThrow) throw opts.ensureThrow
      return opts?.ensure ?? {
        sessionId: 'sess-1',
        claudeSessionId: 'c1',
        resumedExisting: true,
        command: 'claude',
        args: [],
        cols: 80,
        rows: 24,
        reused: true,
      }
    }
    if (route === 'terminal/send-message') {
      if (opts?.sendThrow) throw opts.sendThrow
      return opts?.send ?? {
        success: true,
        target_session_id: 'sess-1',
        attempts: 1,
        reason: null,
        hint: null,
      }
    }
    throw new Error(`unexpected POST ${route}`)
  })
}

beforeEach(() => {
  cleanup()
  h.daemonCliGet.mockReset()
  h.daemonCliPost.mockReset()
  h.openFileAsTab.mockReset()
  h.invoke.mockReset()
  h.activateProject.mockReset()
  h.projects.list = [{ id: 'ws-1', path: '/ws' }]
  mockDaemon()
  mockPosts()
})

function calls(route: string): unknown[][] {
  return h.daemonCliGet.mock.calls.filter((c) => c[0] === route)
}

describe('AgentInboxPane module locks', () => {
  it('does not call Tauri inbox shims, openFileAsTab, unsandboxed HTML, or Settings owner catalog', () => {
    const pane = readFileSync(join(dir, 'AgentInboxPane.tsx'), 'utf8')
    const helpers = readFileSync(join(dir, 'inbox-browser.ts'), 'utf8')
    const dialog = readFileSync(join(dir, 'InboxChatAboutDialog.tsx'), 'utf8')
    for (const src of [pane, helpers, dialog]) {
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
    expect(pane).not.toMatch(/setActiveTab/)
    expect(dialog).not.toMatch(/from '\.\/AgentChatPane'/)
    expect(dialog).not.toMatch(/composerPermitted/)
    expect(dialog).not.toMatch(/forceRespawn/)
    expect(dialog).not.toMatch(/\[from /)
  })
})

describe('AgentInboxPane sources + tray', () => {
  it('always shows K2 Inbox, loads workspace catalog with project=, omits Postal', async () => {
    mockDaemon({ inboxes: { ok: true, inboxes: [HOSTED, LINKED, POSTAL] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)

    await waitFor(() => {
      expect(screen.getByTestId('inbox-source-tray').textContent).toContain('K2 Inbox')
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
    expect(calls('inbox/list')[0][1]).toEqual({ project: '/ws' })
    expect(calls('inbox/list')[0][1]).not.toHaveProperty('folder')
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

  it('lists leftover active items with no folder chips and no folder query', async () => {
    mockDaemon({
      trayList: [
        TRAY_ITEM,
        {
          ...TRAY_ITEM,
          id: 'pkg-active',
          filename: 'pkg-active.md',
          folder: 'active',
          title: 'Leftover active',
        },
      ],
    })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-message-pkg-1')).toBeTruthy())
    expect(screen.getByTestId('inbox-message-pkg-active')).toBeTruthy()
    expect(screen.queryByTestId('inbox-folder-filter')).toBeNull()
    expect(screen.queryByTestId('inbox-folder-active')).toBeNull()
    expect(screen.queryByText('Unassigned')).toBeNull()
    expect(screen.queryByText('In Progress')).toBeNull()
    expect(calls('inbox/folders')).toEqual([])
    for (const c of calls('inbox/list')) {
      expect(c[1]).toEqual({ project: '/ws' })
      expect(c[1]).not.toHaveProperty('folder')
    }
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
  it('catalog failure is a loud one-line error, still shows K2 Inbox, not silent empty mail', async () => {
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

const VIEW_ONLY = /\b(Compose|Reply|Send|Draft|Archive|Move|Respond)\b/

async function openTrayBody(): Promise<void> {
  await waitFor(() => expect(screen.getByTestId('inbox-message-pkg-1')).toBeTruthy())
  fireEvent.click(screen.getByTestId('inbox-message-pkg-1'))
  await waitFor(() => expect(screen.getByTestId('markdown').textContent).toContain('# tray body'))
}

async function openMailSource(address: string): Promise<void> {
  const testId = `inbox-source-mail-${address}`
  await waitFor(() => expect(screen.getByTestId(testId)).toBeTruthy())
  fireEvent.click(screen.getByTestId(testId))
  await waitFor(() => expect(screen.getByTestId('inbox-message-mail-1')).toBeTruthy())
  fireEvent.click(screen.getByTestId('inbox-message-mail-1'))
  await waitFor(() => expect(screen.getByTestId('inbox-mail-text')).toBeTruthy())
}

async function typeChatAboutNote(note: string): Promise<HTMLElement> {
  fireEvent.click(screen.getByTestId('inbox-chat-about'))
  const textarea = await screen.findByTestId('inbox-chat-about-note')
  fireEvent.change(textarea, { target: { value: note } })
  return textarea
}

function postCalls(route: string): unknown[][] {
  return h.daemonCliPost.mock.calls.filter((c) => c[0] === route)
}

describe('AgentInboxPane Chat about this', () => {
  it('hides the button until a tray or mail body is loaded, including loading and bodyError', async () => {
    let resolveRead: ((value: unknown) => void) | undefined
    mockDaemon()
    h.daemonCliGet.mockImplementation(async (route: string, params?: Record<string, unknown>) => {
      if (route === 'mail/inboxes') return { ok: true, inboxes: [] }
      if (route === 'inbox/list') return [TRAY_ITEM]
      if (route === 'inbox/folders') return ['active', 'done']
      if (route === 'inbox/read') {
        return new Promise((resolve) => { resolveRead = resolve })
      }
      throw new Error(`unexpected route ${route} ${JSON.stringify(params)}`)
    })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-message-pkg-1')).toBeTruthy())
    expect(screen.queryByTestId('inbox-chat-about')).toBeNull()

    fireEvent.click(screen.getByTestId('inbox-message-pkg-1'))
    await waitFor(() => expect(screen.getByText('Loading…')).toBeTruthy())
    expect(screen.queryByTestId('inbox-chat-about')).toBeNull()
    expect(typeof resolveRead).toBe('function')
    resolveRead!({ id: 'pkg-1', content: '# tray body' })
    await waitFor(() => expect(screen.getByTestId('inbox-chat-about')).toBeTruthy())
    expect(screen.getByTestId('inbox-chat-about').textContent).toBe('Chat about this')
    cleanup()

    mockDaemon({ bodyThrow: new Error('read failed') })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-message-pkg-1')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-message-pkg-1'))
    await waitFor(() => expect(screen.getByTestId('inbox-body-error').textContent).toContain('read failed'))
    expect(screen.queryByTestId('inbox-chat-about')).toBeNull()
  })

  it('keeps Send outside inbox-browser while the pane still matches I16', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()
    const root = screen.getByTestId('inbox-browser')
    expect(root.textContent).not.toMatch(VIEW_ONLY)
    fireEvent.click(screen.getByTestId('inbox-chat-about'))
    const send = await screen.findByTestId('inbox-chat-about-send')
    expect(send.textContent).toBe('Send')
    expect(root.contains(send)).toBe(false)
    expect(root.textContent).not.toMatch(VIEW_ONLY)
    expect(screen.getByTestId('inbox-chat-about-dialog').contains(send)).toBe(true)
  })

  it('Esc / Cancel / scrim close without ensure or send-message', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()

    fireEvent.click(screen.getByTestId('inbox-chat-about'))
    await screen.findByTestId('inbox-chat-about-send')
    fireEvent.keyDown(window, { key: 'Escape' })
    await waitFor(() => expect(screen.queryByTestId('inbox-chat-about-send')).toBeNull())

    fireEvent.click(screen.getByTestId('inbox-chat-about'))
    await screen.findByTestId('inbox-chat-about-cancel')
    fireEvent.click(screen.getByTestId('inbox-chat-about-cancel'))
    await waitFor(() => expect(screen.queryByTestId('inbox-chat-about-send')).toBeNull())

    fireEvent.click(screen.getByTestId('inbox-chat-about'))
    await screen.findByTestId('inbox-chat-about-scrim')
    fireEvent.mouseDown(screen.getByTestId('inbox-chat-about-scrim'))
    await waitFor(() => expect(screen.queryByTestId('inbox-chat-about-send')).toBeNull())

    expect(h.activateProject).not.toHaveBeenCalled()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('Enter sends and Shift+Enter stays a newline without POST', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()
    const textarea = await typeChatAboutNote('enter note')
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: true })
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    fireEvent.keyDown(textarea, { key: 'Enter', shiftKey: false })
    await waitFor(() => expect(postCalls('terminal/send-message').length).toBe(1))
    const sendBody = postCalls('terminal/send-message')[0][1] as { text: string }
    expect(sendBody.text).toContain('enter note')
    expect(h.activateProject).toHaveBeenCalledWith('ws-1')
  })

  it('disables Send on empty or whitespace notes and does not POST', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()
    fireEvent.click(screen.getByTestId('inbox-chat-about'))
    const send = await screen.findByTestId('inbox-chat-about-send')
    expect((send as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(send)
    fireEvent.change(screen.getByTestId('inbox-chat-about-note'), { target: { value: '   \n' } })
    expect((screen.getByTestId('inbox-chat-about-send') as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    expect(h.activateProject).not.toHaveBeenCalled()
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('tray send activates then ensures then injects the k2 stamp without body or [from', async () => {
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()
    await typeChatAboutNote('please look at this')
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(h.daemonCliPost).toHaveBeenCalled())

    expect(h.activateProject).toHaveBeenCalledTimes(1)
    expect(h.activateProject).toHaveBeenCalledWith('ws-1')
    expect(h.activateProject.mock.invocationCallOrder[0])
      .toBeLessThan(h.daemonCliPost.mock.invocationCallOrder[0])

    expect(h.daemonCliPost.mock.calls.map((c) => c[0])).toEqual([
      'workspace/ensure-pinned-chat',
      'terminal/send-message',
    ])
    expect(h.daemonCliPost.mock.calls[0][1]).toEqual({ project: '/ws' })
    const sendBody = h.daemonCliPost.mock.calls[1][1] as { session_id: string; text: string }
    expect(sendBody.session_id).toBe('sess-1')
    expect(sendBody.text).toBe(
      '[k2 inboxID:pkg-1] Wake package\nOpen: k2 inbox read pkg-1\n\nplease look at this',
    )
    expect(sendBody.text).not.toMatch(/\[from /)
    expect(sendBody.text).not.toContain('# tray body')
    expect(sendBody.text).not.toMatch(/\[inbox:/)
    await waitFor(() => expect(screen.queryByTestId('inbox-chat-about-send')).toBeNull())
    expect(screen.getByTestId('inbox-browser')).toBeTruthy()
    expect(screen.getByTestId('inbox-chat-about')).toBeTruthy()
  })

  it('stamps hosted and linked from the source kind, not the mail id', async () => {
    mockDaemon({ inboxes: { ok: true, inboxes: [HOSTED, LINKED] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openMailSource('you@host.k2')
    await typeChatAboutNote('hosted note')
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(postCalls('terminal/send-message').length).toBe(1))
    const hostedText = (postCalls('terminal/send-message')[0][1] as { text: string }).text
    expect(hostedText).toContain('[hosted inboxID:mail-1] Hello host')
    expect(hostedText).toContain('Open: k2 mail read mail-1')
    expect(hostedText).toContain('hosted note')
    expect(hostedText).not.toMatch(/\[k2 inboxID:/)
    expect(hostedText).not.toMatch(/\[linked inboxID:/)

    fireEvent.click(screen.getByTestId('inbox-source-mail-you@gmail.com'))
    await waitFor(() => expect(screen.getByTestId('inbox-message-mail-1')).toBeTruthy())
    fireEvent.click(screen.getByTestId('inbox-message-mail-1'))
    await waitFor(() => expect(screen.getByTestId('inbox-mail-text')).toBeTruthy())
    await typeChatAboutNote('linked note')
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(postCalls('terminal/send-message').length).toBe(2))
    const linkedText = (postCalls('terminal/send-message')[1][1] as { text: string }).text
    expect(linkedText).toContain('[linked inboxID:mail-1] Hello host')
    expect(linkedText).toContain('Open: k2 mail read mail-1')
    expect(linkedText).toContain('linked note')
    expect(linkedText).not.toMatch(/\[hosted inboxID:/)
    expect(h.daemonCliPost.mock.calls.every((c) => !String(c[0]).startsWith('mail/') && !String(c[0]).startsWith('inbox/'))).toBe(true)
  })

  it('Work Board still ensures { project: path } and stays on Inbox', async () => {
    render(<AgentInboxPane agentName="__workspace__" projectPath="/ws" />)
    expect(screen.getByText('Work Board')).toBeTruthy()
    await openTrayBody()
    await typeChatAboutNote('board note')
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(postCalls('workspace/ensure-pinned-chat').length).toBe(1))
    expect(postCalls('workspace/ensure-pinned-chat')[0][1]).toEqual({ project: '/ws' })
    expect(h.activateProject).toHaveBeenCalledWith('ws-1')
    expect(screen.getByText('Work Board')).toBeTruthy()
    expect(screen.queryByText('__workspace__')).toBeNull()
    await waitFor(() => expect(screen.queryByTestId('inbox-chat-about-send')).toBeNull())
    expect(screen.getByTestId('inbox-browser')).toBeTruthy()
  })

  it('ensure throw / missing sessionId / pty_died / 403 fail in the dialog and restore the note', async () => {
    const note = 'keep this note'
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()

    mockPosts({ ensureThrow: new Error('project not registered: /ws') })
    await typeChatAboutNote(note)
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(screen.getByTestId('inbox-chat-about-error').textContent).toContain('project not registered: /ws'))
    expect((screen.getByTestId('inbox-chat-about-note') as HTMLTextAreaElement).value).toBe(note)
    expect(postCalls('terminal/send-message')).toEqual([])
    fireEvent.click(screen.getByTestId('inbox-chat-about-cancel'))

    mockPosts({ ensure: { session_id: 'snake-only' } })
    await typeChatAboutNote(note)
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(screen.getByTestId('inbox-chat-about-error').textContent).toMatch(/sessionId/))
    expect((screen.getByTestId('inbox-chat-about-note') as HTMLTextAreaElement).value).toBe(note)
    expect(postCalls('terminal/send-message')).toEqual([])
    fireEvent.click(screen.getByTestId('inbox-chat-about-cancel'))

    mockPosts({
      send: { success: false, target_session_id: null, attempts: 1, reason: 'pty_died', hint: 'session gone' },
    })
    await typeChatAboutNote(note)
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(screen.getByTestId('inbox-chat-about-error').textContent).toContain('session gone'))
    expect((screen.getByTestId('inbox-chat-about-note') as HTMLTextAreaElement).value).toBe(note)
    fireEvent.click(screen.getByTestId('inbox-chat-about-cancel'))

    mockPosts({ sendThrow: new Error('403 invalid or missing token') })
    await typeChatAboutNote(note)
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(screen.getByTestId('inbox-chat-about-error').textContent).toContain('403'))
    expect((screen.getByTestId('inbox-chat-about-note') as HTMLTextAreaElement).value).toBe(note)
    expect(screen.getByTestId('inbox-browser')).toBeTruthy()
    expect(screen.getByRole('alert').getAttribute('data-testid')).toBe('inbox-chat-about-error')
  })

  it('skips activate when workspace id is missing and still ensures+sends', async () => {
    h.projects.list = []
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await openTrayBody()
    await typeChatAboutNote('no id')
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(postCalls('terminal/send-message').length).toBe(1))
    expect(h.activateProject).not.toHaveBeenCalled()
    expect(postCalls('workspace/ensure-pinned-chat')[0][1]).toEqual({ project: '/ws' })
    const sendBody = postCalls('terminal/send-message')[0][1] as { session_id: string; text: string }
    expect(sendBody.session_id).toBe('sess-1')
    expect(sendBody.text).toContain('[k2 inboxID:pkg-1]')
    expect(sendBody.text).toContain('no id')
  })

  it('load still does not POST; dialog never hits mail/inbox mutate routes', async () => {
    mockDaemon({ inboxes: { ok: true, inboxes: [HOSTED] } })
    render(<AgentInboxPane agentName="sales" projectPath="/ws" />)
    await waitFor(() => expect(screen.getByTestId('inbox-browser')).toBeTruthy())
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    await openMailSource('you@host.k2')
    expect(h.daemonCliPost).not.toHaveBeenCalled()
    await typeChatAboutNote('inject only')
    fireEvent.click(screen.getByTestId('inbox-chat-about-send'))
    await waitFor(() => expect(h.daemonCliPost.mock.calls.length).toBe(2))
    for (const call of h.daemonCliPost.mock.calls) {
      expect(String(call[0])).not.toMatch(/^(inbox|mail)\//)
      expect(['workspace/ensure-pinned-chat', 'terminal/send-message']).toContain(call[0])
    }
  })
})
