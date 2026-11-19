// @vitest-environment jsdom
// Out of usage (0.46.17): when an agent's LLM login runs out, the daemon
// posts K2's notice on the agent's Thread. It draws as a system row:
//
//   1. Switched (auto-rotate moved the default): an info row with the
//      words, no buttons.
//   2. Another login has room: "Switch to <login>" posts `{id}` to
//      `/cli/llm/accounts/switch` on the Thread's server (busy, then a
//      quiet "Switched"); "Add a login" opens the add-a-login sheet on the
//      tool.
//   3. No other login: "Add a login" is the one button.
//   4. A switch action aimed anywhere but the wallet's switch route, or
//      carrying anything but the login's id, draws no button.
//   5. An App guest sees the words without the buttons.
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliPost: vi.fn(async (..._args: unknown[]): Promise<unknown> => ({ ok: true })),
  sheet: vi.fn((_props: { start: unknown }) => null),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn(async () => []), daemonCliPost: h.daemonCliPost }))
vi.mock('@/components/Settings/shared/LlmLoginSheet', () => ({
  LlmLoginSheet: (props: { start: unknown; onClose: () => void }) => {
    h.sheet(props)
    return <div data-testid="llm-login-sheet" />
  },
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { SessionViewChromeContext } from '@/components/SessionView/sessionViewChrome'
import type { ServerScope } from '@/kessel/server-scope'
import { OWNER_VIEWER, type ThreadViewer } from '@/lib/thread-viewer'
import { fakeScope } from '@/test-utils/fake-scope'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'
import { ThreadBubble } from './ThreadZenKit'
import { parseSystemNotice } from './systemNotice'
import { SWITCH_WORDS } from './ThreadSystemNotice'
import { SYSTEM_AUTHOR } from './systemNotice.fixtures'

const scope = {
  ...fakeScope('box.example.k2.dev'),
  serverSupports: (f: string) => f === 'thread-system-notice-v1',
  isWindowHost: () => true,
} as ServerScope

const SWITCH = { id: 'llm_switch', label: 'Switch to Home Pro', route: '/cli/llm/accounts/switch', body: { id: 'acc_2' } }
const ADD = { id: 'llm_add_login', label: 'Add a login', body: { tool: 'claude' } }

function notice(id: string, body: string, systemNotice: unknown): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: 1,
    id,
    doc: { id, kind: 'text', from: 'k2', to: 'sales', via: 'thread', body, created_at: 1_795_089_600, author: { ...SYSTEM_AUTHOR }, systemNotice },
  }
}

const SWITCHED = notice('o-1', 'sales is out of usage on Work Max until 10:00 (UTC-07:00). Switched to Home Pro.', {
  kind: 'info',
  code: 'llm.out_of_usage.switched',
})
const AVAILABLE = notice(
  'o-2',
  'sales is out of usage on Work Max until 10:00 (UTC-07:00). Home Pro has room: switch to it, or add a login.',
  { kind: 'action', code: 'llm.out_of_usage.switch_available', actions: [SWITCH, ADD] },
)
const NO_LOGIN = notice('o-3', 'sales is out of usage on Work Max until 10:00 (UTC-07:00). Add or switch a login.', {
  kind: 'action',
  code: 'llm.out_of_usage.no_login',
  actions: [ADD],
})

function draw(item: OverlayThreadItem, viewer: ThreadViewer | null = OWNER_VIEWER) {
  return render(
    <SessionViewChromeContext.Provider value={{ viewTab: 'thread', splitLeft: 'terminal', splitRight: 'thread', overlayAddr: 'sales', conversationId: 'c1', agentName: 'sales' }}>
      <ThreadAttachmentSourceProvider value={{ scope, addr: 'sales' }}>
        <ThreadBubble item={item} mine={false} author="sales" timeLabel="now" viewer={viewer} runStart tail />
      </ThreadAttachmentSourceProvider>
    </SessionViewChromeContext.Provider>,
  )
}

const row = (): HTMLElement => screen.getByTestId('thread-system-notice')
const words = (): string => row().querySelector('[data-notice-text]')?.textContent ?? ''

afterEach(() => {
  cleanup()
  h.daemonCliPost.mockReset()
  h.daemonCliPost.mockImplementation(async () => ({ ok: true }))
  h.sheet.mockReset()
})

describe('an out-of-usage notice in the Thread', () => {
  it('switched: an info row with the words and no buttons', () => {
    draw(SWITCHED)
    expect(row().getAttribute('data-notice-kind')).toBe('info')
    expect(row().getAttribute('data-notice-code')).toBe('llm.out_of_usage.switched')
    expect(words()).toBe('sales is out of usage on Work Max until 10:00 (UTC-07:00). Switched to Home Pro.')
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })

  it('a login with room: Switch posts the login id to the wallet switch, then says Switched', async () => {
    draw(AVAILABLE)
    expect(row().getAttribute('data-notice-kind')).toBe('action')
    const sw = screen.getByRole('button', { name: 'Switch to Home Pro' })
    let release: (v: unknown) => void = () => {}
    h.daemonCliPost.mockImplementationOnce(() => new Promise((r) => (release = r)))
    fireEvent.click(sw)
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith(scope, 'llm/accounts/switch', { id: 'acc_2' })
    expect(screen.getByRole('button', { name: SWITCH_WORDS.busy })).toBeTruthy()
    await act(async () => release({ ok: true }))
    await waitFor(() => expect(row().querySelector('[data-notice-done]')?.textContent).toBe(SWITCH_WORDS.done))
    expect(screen.queryByRole('button', { name: 'Switch to Home Pro' })).toBeNull()
  })

  it('Add a login opens the add-a-login sheet on the tool, posting nothing', async () => {
    draw(AVAILABLE)
    fireEvent.click(screen.getByRole('button', { name: 'Add a login' }))
    await waitFor(() => expect(screen.getByTestId('llm-login-sheet')).toBeTruthy())
    expect(h.sheet).toHaveBeenCalled()
    expect(h.sheet.mock.calls[0][0].start).toEqual({ kind: 'new', tool: 'claude' })
    expect(h.daemonCliPost).not.toHaveBeenCalled()
  })

  it('no other login: Add a login is the one button', () => {
    draw(NO_LOGIN)
    expect(words()).toBe('sales is out of usage on Work Max until 10:00 (UTC-07:00). Add or switch a login.')
    const buttons = screen.getAllByRole('button')
    expect(buttons.map((b) => b.textContent)).toEqual(['Add a login'])
  })

  it('a switch aimed elsewhere, or carrying more than the id, draws no button', () => {
    const bad = (a: unknown) => parseSystemNotice(notice('x', 'w', { kind: 'action', code: 'c', actions: [a] }))!.actions
    expect(bad({ ...SWITCH, route: '/cli/thread/resend' })).toEqual([])
    expect(bad({ ...SWITCH, route: 'https://evil.example/cli/llm/accounts/switch' })).toEqual([])
    expect(bad({ ...SWITCH, body: {} })).toEqual([])
    const kept = bad({ ...SWITCH, body: { id: 'acc_2', clearDefault: true } })
    expect(kept).toHaveLength(1)
    expect(kept[0].call).toEqual({ kind: 'post', route: 'llm/accounts/switch', body: { id: 'acc_2' } })
    expect(bad({ ...ADD, body: {} })).toEqual([])
  })

  it('an App guest sees the words without the buttons', () => {
    const guest: ThreadViewer = { principal: 'qa-1', aliases: [], guest: true }
    draw(AVAILABLE, guest)
    expect(words()).toContain('Home Pro has room')
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })
})
