import { describe, expect, it, vi, beforeEach } from 'vitest'
import {
  chatRenamePayloadForTab,
  chatDisplayName,
  findChatSessionInTab,
  findTabByPaneGroupId,
  persistChatRenameIfSessionTab,
  restampSessionTabs,
} from './chat-session-tab'
import type { Tab, TerminalItemData, FileViewerItemData } from '@/stores/tabs'

const SID = '01920000-aaaa-7000-8000-000000000001'

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
})
