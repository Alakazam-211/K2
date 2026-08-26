import { describe, expect, it, vi, beforeEach } from 'vitest'
import {
  chatRenamePayloadForTab,
  chatDisplayName,
  conversationIdFromArgs,
  copyableAddressFromDaemonRow,
  copyableAddressForWorkspaceHandle,
  daemonRowForTab,
  findChatSessionInTab,
  findTabByPaneGroupId,
  isAgentPtyTerminalItem,
  persistChatRenameIfSessionTab,
  pickConversationId,
  restampSessionTabs,
  restampTitle,
  tabLooksLikeChatSession,
} from './chat-session-tab'
import type { Tab, TerminalItemData, FileViewerItemData } from '@/stores/tabs'

const SID = '01920000-aaaa-7000-8000-000000000001'
const PTY = 'bbbbbbbb-cccc-4ddd-8eee-ffffffffffff'

function terminalTab(opts: {
  id?: string
  title?: string
  isSystemAgent?: boolean
  data?: Partial<TerminalItemData>
}): Tab {
  const data: TerminalItemData = {
    terminalId: 'pty-1',
    cwd: '/tmp/proj',
    command: 'claude',
    args: ['--resume', SID],
    ...opts.data,
  }
  return {
    id: opts.id ?? 'tab-1',
    title: opts.title ?? 'Chat',
    mosaicTree: 'pg-1',
    paneGroups: new Map([['pg-1', {
      id: 'pg-1',
      items: [{ id: 'item-1', type: 'terminal', data }],
      activeItemIndex: 0,
    }]]),
    isSystemAgent: opts.isSystemAgent,
  }
}

function fileTab(): Tab {
  const data: FileViewerItemData = { filePath: '/tmp/proj/README.md' }
  return {
    id: 'file-1',
    title: 'README.md',
    mosaicTree: 'pg-file',
    paneGroups: new Map([['pg-file', {
      id: 'pg-file',
      items: [{ id: 'item-f', type: 'file-viewer', data }],
      activeItemIndex: 0,
    }]]),
  }
}

const listed = [{ sessionId: SID, provider: 'claude' }]

describe('chatDisplayName', () => {
  it('prefers trimmed customName and falls back to title', () => {
    expect(chatDisplayName({ customName: '  Named  ', title: 'Transcript' })).toBe('Named')
    expect(chatDisplayName({ customName: '   ', title: 'Transcript' })).toBe('Transcript')
    expect(chatDisplayName({ customName: '', title: 'Transcript' })).toBe('Transcript')
    expect(chatDisplayName({ title: 'Transcript' })).toBe('Transcript')
  })
})

describe('findChatSessionInTab', () => {
  const ids = new Set([SID])

  it('hits a resume tab whose args contain the listed session uuid', () => {
    const hit = findChatSessionInTab(terminalTab({}), ids)
    expect(hit).toEqual({ sessionId: SID, provider: 'claude' })
  })

  it('matches conversationId first, ignoring the PTY sessionId', () => {
    const tab = terminalTab({
      data: {
        args: ['--dangerously-skip-permissions'],
        sessionId: PTY,
        conversationId: SID,
        command: 'claude',
      },
    })
    expect(findChatSessionInTab(tab)).toEqual({ sessionId: SID, provider: 'claude' })
    expect(findChatSessionInTab(tab, ids)).toEqual({ sessionId: SID, provider: 'claude' })
  })

  it('ignores TerminalItemData.sessionId (Kessel PTY id)', () => {
    const tab = terminalTab({
      data: { args: ['--help'], sessionId: SID, command: 'claude' },
    })
    expect(findChatSessionInTab(tab, ids)).toBeNull()
  })

  it('excludes system, heartbeat, fromApi, and api- attach tabs', () => {
    expect(findChatSessionInTab(terminalTab({ isSystemAgent: true }), ids)).toBeNull()
    expect(findChatSessionInTab(terminalTab({ data: { heartbeatName: 'hb' } }), ids)).toBeNull()
    expect(findChatSessionInTab(terminalTab({ data: { fromApi: true } }), ids)).toBeNull()
    expect(findChatSessionInTab(terminalTab({ data: { attachAgentName: 'api-x' } }), ids)).toBeNull()
  })

  it('does not match a file tab', () => {
    expect(findChatSessionInTab(fileTab(), ids)).toBeNull()
  })
})

describe('chatRenamePayloadForTab / persistChatRenameIfSessionTab', () => {
  it('builds chat/rename payload only for session tabs', () => {
    const payload = chatRenamePayloadForTab(terminalTab({}), listed, '  New Name  ')
    expect(payload).toEqual({
      provider: 'claude',
      session_id: SID,
      custom_name: 'New Name',
    })
  })

  it('does not fire for file or heartbeat tabs', () => {
    expect(chatRenamePayloadForTab(fileTab(), listed, 'Name')).toBeNull()
    expect(
      chatRenamePayloadForTab(terminalTab({ data: { heartbeatName: 'nightly' } }), listed, 'Name'),
    ).toBeNull()
  })

  it('uses conversationId for a fresh premint that is not on chat/list', () => {
    const tab = terminalTab({
      data: {
        args: ['--dangerously-skip-permissions'],
        conversationId: SID,
        sessionId: PTY,
        command: 'claude',
      },
    })
    const payload = chatRenamePayloadForTab(tab, [], 'Code Review')
    expect(payload).toEqual({
      provider: 'claude',
      session_id: SID,
      custom_name: 'Code Review',
    })
    expect(payload?.session_id).not.toBe(PTY)
  })
})

const daemonMocks = vi.hoisted(() => ({
  daemonCliGet: vi.fn(),
  daemonCliPost: vi.fn(),
}))

vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: daemonMocks.daemonCliGet,
  daemonCliPost: daemonMocks.daemonCliPost,
}))

describe('persistChatRenameIfSessionTab', () => {
  beforeEach(() => {
    daemonMocks.daemonCliGet.mockReset()
    daemonMocks.daemonCliPost.mockReset()
    daemonMocks.daemonCliGet.mockResolvedValue(listed)
    daemonMocks.daemonCliPost.mockResolvedValue({ success: true })
  })

  it('POSTs chat/rename for a session tab', async () => {
    const ok = await persistChatRenameIfSessionTab(terminalTab({}), 'Renamed', '/tmp/proj')
    expect(ok).toBe(true)
    expect(daemonMocks.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(daemonMocks.daemonCliPost).toHaveBeenCalledWith('chat/rename', {
      provider: 'claude',
      session_id: SID,
      custom_name: 'Renamed',
    })
  })

  it('POSTs conversationId, not the PTY sessionId, and does not require chat/list', async () => {
    daemonMocks.daemonCliGet.mockResolvedValue([])
    const tab = terminalTab({
      data: {
        args: ['--dangerously-skip-permissions'],
        conversationId: SID,
        sessionId: PTY,
        command: 'claude',
      },
    })
    const ok = await persistChatRenameIfSessionTab(tab, 'Code Review', '/tmp/proj')
    expect(ok).toBe(true)
    expect(daemonMocks.daemonCliGet).not.toHaveBeenCalled()
    expect(daemonMocks.daemonCliPost).toHaveBeenCalledWith('chat/rename', {
      provider: 'claude',
      session_id: SID,
      custom_name: 'Code Review',
    })
  })

  it('POSTs for a split-column extra tab the same way', async () => {
    const extra = terminalTab({ id: 'extra-1', data: { conversationId: SID, args: ['--dangerously-skip-permissions'] } })
    const ok = await persistChatRenameIfSessionTab(extra, 'Reviewer', '/tmp/proj')
    expect(ok).toBe(true)
    expect(daemonMocks.daemonCliPost).toHaveBeenCalledWith('chat/rename', {
      provider: 'claude',
      session_id: SID,
      custom_name: 'Reviewer',
    })
  })

  it('does not POST chat/rename for file or heartbeat tabs', async () => {
    expect(await persistChatRenameIfSessionTab(fileTab(), 'Name', '/tmp/proj')).toBe(false)
    expect(
      await persistChatRenameIfSessionTab(
        terminalTab({ data: { heartbeatName: 'nightly' } }),
        'Name',
        '/tmp/proj',
      ),
    ).toBe(false)
    expect(daemonMocks.daemonCliPost).toHaveBeenCalledTimes(0)
  })

  it('returns false (skip-rename) when a believed session tab has no conversation id', async () => {
    const tab = terminalTab({
      data: { args: ['--dangerously-skip-permissions'], command: 'claude' },
    })
    expect(tabLooksLikeChatSession(tab)).toBe(true)
    expect(await persistChatRenameIfSessionTab(tab, 'Name', '/tmp/proj')).toBe(false)
    expect(daemonMocks.daemonCliPost).toHaveBeenCalledTimes(0)
  })

  it('does not treat file tabs as believed sessions', () => {
    expect(tabLooksLikeChatSession(fileTab())).toBe(false)
    expect(tabLooksLikeChatSession(terminalTab({ data: { heartbeatName: 'nightly' } }))).toBe(false)
  })
})

describe('findTabByPaneGroupId / restampSessionTabs', () => {
  it('looks up the tab that owns a pane group id', () => {
    const tab = terminalTab({ id: 'real-tab' })
    expect(findTabByPaneGroupId([tab], 'pg-1')?.id).toBe('real-tab')
    expect(findTabByPaneGroupId([tab], 'missing')).toBeUndefined()
  })

  it('restamps matching session tabs via setTabTitle locked', () => {
    const setTabTitle = vi.fn()
    const session = terminalTab({ id: 'sess' })
    const hb = terminalTab({ id: 'hb', data: { heartbeatName: 'nightly' } })
    restampSessionTabs([session, hb, fileTab()], SID, 'New', setTabTitle)
    expect(setTabTitle).toHaveBeenCalledTimes(1)
    expect(setTabTitle).toHaveBeenCalledWith('sess', 'New', { locked: true })
  })

  it('preserves a (from <branch>) suffix when restamping', () => {
    const setTabTitle = vi.fn()
    const session = terminalTab({ id: 'sess', title: 'Old Name (from feature/x)' })
    restampSessionTabs([session], SID, 'Code Review', setTabTitle)
    expect(setTabTitle).toHaveBeenCalledWith('sess', 'Code Review (from feature/x)', { locked: true })
    expect(restampTitle('Claude (from main)', 'Named')).toBe('Named (from main)')
    expect(restampTitle('Claude', 'Named')).toBe('Named')
  })
})

describe('pickConversationId', () => {
  it('copies daemon conversationId and never writes the PTY sessionId', () => {
    expect(pickConversationId(undefined, SID, PTY)).toBe(SID)
    expect(pickConversationId('keep', PTY, PTY)).toBe('keep')
    expect(pickConversationId('keep', undefined, PTY)).toBe('keep')
    expect(pickConversationId(undefined, PTY, PTY)).toBeUndefined()
  })
})

describe('conversationIdFromArgs', () => {
  it('reads --session-id / --resume / resume subcommand', () => {
    expect(conversationIdFromArgs('claude', ['--dangerously-skip-permissions', '--session-id', SID])).toBe(SID)
    expect(conversationIdFromArgs('claude', ['--resume', SID])).toBe(SID)
    expect(conversationIdFromArgs('codex', ['resume', SID])).toBe(SID)
  })
})

describe('copy address', () => {
  it('sidecar clipboard is sales/… and the menu label is the segment only', () => {
    const addr = copyableAddressFromDaemonRow({
      kind: 'sidecar',
      handle: 'sales/code-review',
    })
    expect(addr).toEqual({ label: 'code-review', clipboard: 'sales/code-review' })
    expect(addr?.label.includes('sales/')).toBe(false)
    expect(addr?.clipboard.startsWith('sales/')).toBe(true)
  })

  it('canonical copy is the workspace handle only', () => {
    const addr = copyableAddressFromDaemonRow({ kind: 'canonical', handle: 'sales' })
    expect(addr).toEqual({ label: 'sales', clipboard: 'sales' })
    expect(copyableAddressForWorkspaceHandle('sales')).toEqual({
      label: 'sales',
      clipboard: 'sales',
    })
  })

  it('omits unresolved / api / empty handles (never a PTY uuid)', () => {
    expect(copyableAddressFromDaemonRow({ kind: 'sidecar', handle: '' })).toBeNull()
    expect(copyableAddressFromDaemonRow({ kind: 'api', handle: PTY })).toBeNull()
    expect(copyableAddressFromDaemonRow({ kind: 'other', handle: 'sales/x' })).toBeNull()
    expect(copyableAddressForWorkspaceHandle('   ')).toBeNull()
  })

  it('matches a session tab to the daemon tab-<pane> row', () => {
    const tab = terminalTab({})
    const row = daemonRowForTab(tab, [
      { agentName: 'tab-other', kind: 'sidecar', handle: 'sales/nope' },
      { agentName: 'tab-pg-1', kind: 'sidecar', handle: 'sales/reviewer' },
    ])
    expect(row?.handle).toBe('sales/reviewer')
    expect(copyableAddressFromDaemonRow(row!)).toEqual({
      label: 'reviewer',
      clipboard: 'sales/reviewer',
    })
  })
})

describe('isAgentPtyTerminalItem (C9)', () => {
  it('true for harness command and sidecar conversation', () => {
    expect(
      isAgentPtyTerminalItem({
        type: 'terminal',
        data: { terminalId: 't', cwd: '/ws', command: 'claude' },
      }),
    ).toBe(true)
    expect(
      isAgentPtyTerminalItem({
        type: 'terminal',
        data: { terminalId: 't', cwd: '/ws', command: 'grok', conversationId: SID },
      }),
    ).toBe(true)
  })

  it('false for empty bash, file viewer, and API cells', () => {
    expect(
      isAgentPtyTerminalItem({
        type: 'terminal',
        data: { terminalId: 't', cwd: '/ws' },
      }),
    ).toBe(false)
    expect(
      isAgentPtyTerminalItem({
        type: 'file-viewer',
        data: { filePath: '/ws/README.md' },
      }),
    ).toBe(false)
    expect(
      isAgentPtyTerminalItem({
        type: 'terminal',
        data: { terminalId: 't', cwd: '/ws', command: 'claude', fromApi: true },
      }),
    ).toBe(false)
    expect(
      isAgentPtyTerminalItem({
        type: 'browser',
        data: { url: 'https://example.com' },
      }),
    ).toBe(false)
  })

  it('true for heartbeat-surfaced agent PTY (still an agent session)', () => {
    expect(
      isAgentPtyTerminalItem({
        type: 'terminal',
        data: {
          terminalId: 't',
          cwd: '/ws',
          command: 'claude',
          heartbeatName: 'daily',
        },
      }),
    ).toBe(true)
  })
})
