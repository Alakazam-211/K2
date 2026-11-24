// @vitest-environment jsdom
// One attach for every box (Rosson 2026-11-23: "when you add the UI with the
// + we just keep it identical to what we have elsewhere"). The agent chat's
// compose bar and a Home DM or channel's box draw the SAME components from
// `ComposeAttach`: the "+" (`ComposeAttachButton`, the same markup in both)
// and the chips (`ComposeAttachmentChip`). Fail loud.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import type { ComponentProps } from 'react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  bytes: vi.fn(async (): Promise<unknown> => ({ ok: true, upload: { id: '5001', committed: 4, size: 4, complete: true } })),
  picked: [] as string[][],
  buttons: [] as string[],
  chips: [] as string[],
}))

vi.mock('@/components/Thread/ComposeAttach', async (orig) => {
  const real = await orig<typeof import('./ComposeAttach')>()
  return {
    ...real,
    ComposeAttachButton: (p: ComponentProps<typeof real.ComposeAttachButton>) => {
      h.buttons.push(p.testId ?? '')
      return real.ComposeAttachButton(p)
    },
    ComposeAttachmentChip: (p: ComponentProps<typeof real.ComposeAttachmentChip>) => {
      h.chips.push(p.name)
      return real.ComposeAttachmentChip(p)
    },
  }
})
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  daemonCliPostBytes: () => h.bytes(),
  daemonCliGetBlob: async () => new Blob([]),
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
vi.mock('@/lib/pick-compose-files', () => ({
  composeAttachPlan: () => ({ kind: 'native' }),
  pickLocalComposeFiles: vi.fn(async () => h.picked.shift() ?? null),
  pickRemoteComposeFile: vi.fn(async () => null),
}))
vi.mock('@/lib/handle-remote-drop', () => ({ executeRemoteDrop: vi.fn(), executeBrowserFileDrop: vi.fn() }))
vi.mock('@/lib/load-host-binary', () => ({
  loadHostImageObjectUrl: vi.fn(async (_s: unknown, path: string) => ({ url: `blob:${path}` })),
  revokeObjectUrl: () => {},
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: () => () => undefined,
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { ServerScope } from '@/kessel/server-scope'
import { noteServerVersion } from '@/kessel/server-scope'
import { TerminalComposeBar } from '@/components/Terminal/TerminalComposeBar'
import { SessionViewChromeContext, type SessionViewChromeValue } from '@/components/SessionView/sessionViewChrome'
import HomeChatView from '@/components/HomeChat/HomeChatView'
import { resetHomeChatForTests } from '@/stores/home-chat'
import { resetHomeChatViewCacheForTests } from '@/lib/home-chat/chat'
import { useContextMenuStore } from '@/stores/context-menu'
import { fakeScope } from '@/test-utils/fake-scope'
import { renderInRoom, testRoom } from '@/test-utils/room'
import type { ProjectWithWorkspaces } from '@/stores/projects'
import { ComposeAttachButton } from './ComposeAttach'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const WS = '/Users/me/ai/sales'
const chrome = {
  viewTab: 'thread',
  splitLeft: 'terminal',
  splitRight: 'thread',
  overlayAddr: 'sales',
  conversationId: null,
  agentName: 'sales',
} as unknown as SessionViewChromeValue

function agentChat(): void {
  const scope = { ...fakeScope('local', { remote: false }), serverSupports: (f: string) => f === 'thread-attachments-v1' } as ServerScope
  const room = testRoom({
    tabs: {},
    scope,
    isPrimary: true,
    projects: [{ id: 'p', name: 'sales', path: WS } as unknown as ProjectWithWorkspaces],
    activeProjectId: 'p',
  })
  renderInRoom(
    room,
    <SessionViewChromeContext.Provider value={chrome}>
      <TerminalComposeBar sessionId="sess-1" workspacePath={WS} sendDestination="thread" />
    </SessionViewChromeContext.Provider>,
  )
}

async function homeChat(): Promise<void> {
  render(<HomeChatView target={{ kind: 'channel', hostKey: 'local', conv: '600', address: '600::local' }} />)
  await screen.findByTestId('home-chat-plus')
  await waitFor(() => expect(screen.getByTestId('home-chat-input').hasAttribute('disabled')).toBe(false))
}

/** The "+" as markup, its test id aside (each box names its own). */
function plusMarkup(el: Element): string {
  const c = el.cloneNode(true) as Element
  c.removeAttribute('data-testid')
  return c.outerHTML
}

beforeEach(() => {
  cleanup()
  localStorage.clear()
  let n = 0
  URL.createObjectURL = vi.fn(() => `blob:test-${++n}`)
  URL.revokeObjectURL = vi.fn()
  resetHomeChatForTests()
  resetHomeChatViewCacheForTests()
  noteServerVersion('local', '0.46.18', ['home-chat-v1', 'home-chat-attachments-v1'])
  h.buttons.length = 0
  h.chips.length = 0
  h.picked.length = 0
  h.get.mockReset()
  h.post.mockReset()
  h.get.mockImplementation(async (_s, route) => {
    switch (route) {
      case 'terminal/compose-history':
        return { items: [] }
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {} }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return { ok: true, me: { ext: 'k2:connect:me', memberId: '1' }, people: [] }
      case 'home-chat/channels/members':
        return { ok: true, conv: '600', slug: 'c-0123456789', state: 'active', name: 'design', open: false, creator: 'k2:connect:me', people: [], agents: [] }
      case 'home-chat/messages':
        return { ok: true, conv: '600', page: { items: [], has_more: false } }
      default:
        return []
    }
  })
  h.post.mockImplementation(async (_s, route, b) => {
    if (route === 'home-chat/uploads/create') {
      return { ok: true, upload: { id: '5001', committed: 0, size: (b as { size: number }).size, complete: false, chunkBytes: 1048576, maxRequestBytes: 8388608 } }
    }
    return { ok: true }
  })
})

afterEach(() => {
  cleanup()
  resetHomeChatForTests()
})

describe('one attach component for the agent chat and Home chat', () => {
  it('both boxes render ComposeAttachButton with the same markup, and their files as ComposeAttachmentChip', async () => {
    agentChat()
    const agentPlus = await screen.findByTestId('compose-plus')
    expect(h.buttons).toContain('compose-plus')
    // A picked file in the agent chat: its chip is the shared chip.
    h.picked.push([`${WS}/docs/plan.md`])
    await act(async () => {
      fireEvent.click(agentPlus)
    })
    await act(async () => {
      useContextMenuStore.getState().selectItem('attach')
    })
    await waitFor(() => expect(h.chips).toContain('plan.md'))
    const agentMarkup = plusMarkup(agentPlus)
    cleanup()

    await homeChat()
    const homePlus = screen.getByTestId('home-chat-plus')
    expect(h.buttons).toContain('home-chat-plus')
    // The same "+": the same element, classes, style, glyph and labels.
    expect(plusMarkup(homePlus)).toBe(agentMarkup)
    // And the same as the shared component drawn on its own.
    cleanup()
    render(<ComposeAttachButton menuOpen={false} onClick={() => undefined} testId="alone" />)
    expect(plusMarkup(screen.getByTestId('alone'))).toBe(agentMarkup)
    cleanup()

    // A pasted file in Home chat: the shared chip too.
    await homeChat()
    await act(async () => {
      fireEvent.paste(screen.getByTestId('home-chat-input'), {
        clipboardData: { files: [new File([new Uint8Array(4)], 'notes.txt', { type: 'text/plain' })], items: [], getData: () => '' },
      })
    })
    await waitFor(() => expect(h.chips).toContain('notes.txt'))
  })
})
