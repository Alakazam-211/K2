// @vitest-environment jsdom
// A heartbeat turned off (0.46.17): after repeated failures the daemon
// posts its notice on the agent's Thread with one button:
//
//   1. "Turn it back on" posts `{project, name, enabled: true}` to the
//      heartbeat's enable route on the Thread's server (busy, then a quiet
//      "Turned back on").
//   2. The button aimed anywhere else, or missing the workspace or name,
//      draws no button; whatever else it carries, it only ever turns on.
//   3. A paused heartbeat (the usage limit) is an info row with no button.
//   4. An App guest sees the words without the button.
import { afterEach, describe, expect, it, vi } from 'vitest'

const h = vi.hoisted(() => ({
  daemonCliPost: vi.fn(async (..._args: unknown[]): Promise<unknown> => ({ ok: true })),
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ listen: vi.fn(async () => () => {}), emit: vi.fn(async () => {}) }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: vi.fn(async () => []), daemonCliPost: h.daemonCliPost }))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import type { OverlayThreadItem } from '@/components/SessionView/overlayThread'
import { SessionViewChromeContext } from '@/components/SessionView/sessionViewChrome'
import type { ServerScope } from '@/kessel/server-scope'
import { OWNER_VIEWER, type ThreadViewer } from '@/lib/thread-viewer'
import { fakeScope } from '@/test-utils/fake-scope'
import { ThreadAttachmentSourceProvider } from './ThreadImageObjects'
import { ThreadBubble } from './ThreadZenKit'
import { parseSystemNotice } from './systemNotice'
import { HEARTBEAT_ENABLE_WORDS } from './ThreadSystemNotice'
import { SYSTEM_AUTHOR } from './systemNotice.fixtures'

const scope = {
  ...fakeScope('box.example.k2.dev'),
  serverSupports: (f: string) => f === 'thread-system-notice-v1',
  isWindowHost: () => true,
} as ServerScope

const ENABLE = {
  id: 'heartbeat_enable',
  label: 'Turn it back on',
  route: '/cli/heartbeat/enable',
  body: { project: 'sales', name: 'morning', enabled: true },
}

function notice(id: string, body: string, systemNotice: unknown): OverlayThreadItem {
  return {
    collection: 'thread',
    seq: 1,
    id,
    doc: { id, kind: 'text', from: 'k2', to: 'sales', via: 'thread', body, created_at: 1_795_089_600, author: { ...SYSTEM_AUTHOR }, systemNotice },
  }
}

const OFF_WORDS = "sales's heartbeat 'morning' was turned off after 5 failed runs (resume spawn failed: exit status 1)."
const OFF = notice('h-1', OFF_WORDS, { kind: 'action', code: 'heartbeat.auto_disabled', actions: [ENABLE] })
const PAUSED_WORDS = "sales's heartbeat 'morning' is paused until 15:00 (UTC-07:00) (usage limit); it resumes by itself."
const PAUSED = notice('h-2', PAUSED_WORDS, { kind: 'info', code: 'heartbeat.paused_usage_limit' })

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
})

describe('a heartbeat turned off, in the Thread', () => {
  it('Turn it back on posts the workspace and name to the enable route, then says so', async () => {
    draw(OFF)
    expect(row().getAttribute('data-notice-kind')).toBe('action')
    expect(row().getAttribute('data-notice-code')).toBe('heartbeat.auto_disabled')
    expect(words()).toBe(OFF_WORDS)
    let release: (v: unknown) => void = () => {}
    h.daemonCliPost.mockImplementationOnce(() => new Promise((r) => (release = r)))
    fireEvent.click(screen.getByRole('button', { name: 'Turn it back on' }))
    expect(h.daemonCliPost).toHaveBeenCalledTimes(1)
    expect(h.daemonCliPost).toHaveBeenCalledWith(scope, 'heartbeat/enable', { project: 'sales', name: 'morning', enabled: true })
    expect(screen.getByRole('button', { name: HEARTBEAT_ENABLE_WORDS.busy })).toBeTruthy()
    await act(async () => release({ success: true }))
    await waitFor(() => expect(row().querySelector('[data-notice-done]')?.textContent).toBe(HEARTBEAT_ENABLE_WORDS.done))
    expect(screen.queryByRole('button', { name: 'Turn it back on' })).toBeNull()
  })

  it('aimed elsewhere or missing its workspace or name: no button; it only ever turns on', () => {
    const parsed = (a: unknown) => parseSystemNotice(notice('x', 'w', { kind: 'action', code: 'c', actions: [a] }))!.actions
    expect(parsed({ ...ENABLE, route: '/cli/heartbeat/remove' })).toEqual([])
    expect(parsed({ ...ENABLE, route: 'https://evil.example/cli/heartbeat/enable' })).toEqual([])
    expect(parsed({ ...ENABLE, body: { name: 'morning' } })).toEqual([])
    expect(parsed({ ...ENABLE, body: { project: 'sales' } })).toEqual([])
    const kept = parsed({ ...ENABLE, body: { project: 'sales', name: 'morning', enabled: false, purge: true } })
    expect(kept).toHaveLength(1)
    expect(kept[0].call).toEqual({ kind: 'post', route: 'heartbeat/enable', body: { project: 'sales', name: 'morning', enabled: true } })
  })

  it('a pause on the usage limit is an info row with no button', () => {
    draw(PAUSED)
    expect(row().getAttribute('data-notice-kind')).toBe('info')
    expect(words()).toBe(PAUSED_WORDS)
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })

  it('an App guest sees the words without the button', () => {
    const guest: ThreadViewer = { principal: 'qa-1', aliases: [], guest: true }
    draw(OFF, guest)
    expect(words()).toBe(OFF_WORDS)
    expect(screen.queryAllByRole('button')).toHaveLength(0)
  })
})
