// @vitest-environment jsdom
// Home chat pushes in the chat view (prd-home-chat-pushes-v1 §7, §12, HC-15):
//
//   1. The Notify menu: a channel offers exactly All messages and @mentions
//      only (no Off); a level of `none` shows as "Off (set elsewhere)" with
//      both still offered; a DM says "DMs always notify" and offers none;
//      All messages before `allMessagesLive` says when it starts.
//   2. Picks show at once and go back, with the daemon's hint, when refused;
//      the push-prefs toggles post only what changed.
//   3. The daemon's `notify` frame (another device) moves the menu.
//   4. None of it shows on a server that doesn't list `home-chat-push-v1`.
//   5. The viewing lease: sent on the events socket while the chat is open
//      and the window focused; `visible:false` on blur and close; sent again
//      when the socket reconnects.
//   6. A message that mentions me (my member id, @channel, @here) is marked.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => undefined), listen: vi.fn(async () => () => undefined) }))

type Link = (up: boolean, send?: (t: string) => boolean) => void
const h = vi.hoisted(() => ({
  get: vi.fn(async (_s: unknown, _r: string, _p?: Record<string, unknown>): Promise<unknown> => ({})),
  post: vi.fn(async (_s: unknown, _r: string, _b?: unknown): Promise<unknown> => ({ ok: true })),
  frames: null as null | ((f: unknown) => void),
  link: null as null | Link,
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (s: unknown, r: string, p?: Record<string, unknown>) => h.get(s, r, p),
  daemonCliPost: (s: unknown, r: string, b?: unknown) => h.post(s, r, b),
  withHostCliSlot: async <T,>(_s: unknown, fn: () => Promise<T>): Promise<T> => fn(),
}))
vi.mock('@/lib/home-chat/events-socket', async (orig) => ({
  ...(await orig<typeof import('@/lib/home-chat/events-socket')>()),
  subscribeHomeChatEvents: (_scope: unknown, fn: (f: unknown) => void, onLink: Link) => {
    h.frames = fn
    h.link = onLink
    return () => {
      h.frames = null
      h.link = null
    }
  },
}))

import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react'
import HomeChatView from './HomeChatView'
import { ALL_MESSAGES_PENDING, DM_ALWAYS_NOTIFIES, NOTIFY_OFF_ELSEWHERE, SERVER_TEXT_OFF, homeChatNotifyItems } from './HomeChatNotifyMenu'
import { resetHomeChatForTests } from '@/stores/home-chat'
import { useContextMenuStore } from '@/stores/context-menu'
import { useToastStore } from '@/stores/toast'
import { noteServerVersion } from '@/kessel/server-scope'
import { resetHomeChatViewCacheForTests } from '@/lib/home-chat/chat'
import type { HomeChatNotify, HomeChatPushPrefs } from '@/lib/home-chat/api'
import type { HomeChatTarget } from '@/lib/home-chat/address'

;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const CHANNEL: HomeChatTarget = { kind: 'channel', hostKey: 'local', conv: '600', address: '600::local' }
const ME = '1'
const ALEX = '100'

function m(id: string, author: string, body: string, extra: Record<string, unknown> = {}): Record<string, unknown> {
  return { id, channel_id: '600', author_id: author, body, seq: Number(id), created_at: '2026-11-14T10:00:00Z', reactions: [], ...extra }
}

const MESSAGES = [
  m('20', ALEX, 'plain'),
  m('21', ALEX, 'for you', { mentions: { members: [ME], channel: false, here: false } }),
  m('22', ALEX, 'for everyone', { mentions: { members: [], channel: true, here: false } }),
  m('23', ALEX, 'for who is here', { mentions: { members: [], channel: false, here: true } }),
  m('24', ALEX, 'for someone else', { mentions: { members: ['555'], channel: false, here: false } }),
  m('25', ME, 'mine, at everyone', { mentions: { members: [], channel: true, here: false } }),
]

const notifyOf = (over: Partial<HomeChatNotify> = {}): HomeChatNotify => ({
  conv: '600',
  convKind: 'channel',
  level: 'mentions',
  effective: 'mentions',
  inherited: true,
  fixed: false,
  muted: false,
  allMessagesLive: false,
  ...over,
})
const PREFS: HomeChatPushPrefs = { on: true, previews: false, agents: false, serverPreviews: true, bell: true, devices: 1, allMessagesLive: false }

let level: HomeChatNotify['level'] = 'mentions'

function setPushes(on: boolean): void {
  noteServerVersion('local', '0.46.15', on ? ['home-chat-v1', 'home-chat-push-v1'] : ['home-chat-v1'])
}

beforeEach(() => {
  resetHomeChatForTests()
  resetHomeChatViewCacheForTests()
  useToastStore.setState({ toasts: [] })
  level = 'mentions'
  setPushes(true)
  h.get.mockReset()
  h.post.mockReset()
  h.get.mockImplementation(async (_s, route, p) => {
    switch (route) {
      case 'home-chat/status':
        return { ok: true, on: true, reason: null, flags: {} }
      case 'home-chat/summary':
        return { ok: true, chat: 'live', convs: [] }
      case 'home-chat/people':
        return {
          ok: true,
          me: { ext: 'k2:connect:me', memberId: ME },
          people: [{ ext: 'k2:connect:a', username: 'alex', displayName: 'Alex', ready: true, memberId: ALEX }],
        }
      case 'home-chat/channels/members':
        return { ok: true, conv: '600', slug: 'c-0123456789', state: 'active', name: 'design', open: true, creator: 'k2:connect:me', people: [], agents: [] }
      case 'home-chat/messages':
        return { ok: true, conv: '600', page: { items: MESSAGES, has_more: false } }
      case 'home-chat/notify':
        expect(p).toEqual({ conv: '600' })
        return { ok: true, ...notifyOf({ level, effective: level, inherited: level === 'mentions' }) }
      case 'home-chat/push-prefs':
        return { ok: true, ...PREFS }
      default:
        throw new Error(`unexpected GET ${route}`)
    }
  })
  h.post.mockImplementation(async () => ({ ok: true }))
})

afterEach(() => {
  cleanup()
  useContextMenuStore.getState().close()
  resetHomeChatForTests()
  noteServerVersion('local', null, [])
  vi.restoreAllMocks()
})

async function open(target: HomeChatTarget = CHANNEL): Promise<void> {
  render(<HomeChatView target={target} />)
  await screen.findByText('plain')
}

/** Open the Notify menu; its rows once shown. */
async function menu(): Promise<Array<{ id: string; label: string; checked?: boolean; enabled?: boolean; type?: string }>> {
  await act(async () => {
    fireEvent.click(screen.getByTestId('home-chat-notify-button'))
  })
  await waitFor(() => expect(useContextMenuStore.getState().isOpen).toBe(true))
  return useContextMenuStore.getState().items
}

async function pick(id: string): Promise<void> {
  await act(async () => {
    useContextMenuStore.getState().selectItem(id)
  })
}

describe('the Notify menu rows', () => {
  it('a channel: exactly All messages and @mentions only, the current one checked', () => {
    const rows = homeChatNotifyItems('channel', notifyOf(), PREFS)
    const choices = rows.filter((r) => r.id.startsWith('level:'))
    expect(choices.map((r) => [r.label, r.checked])).toEqual([
      ['All messages', false],
      ['@mentions only', true],
    ])
    expect(rows.some((r) => /off/i.test(r.label))).toBe(false)
    expect(rows.find((r) => r.id === 'pref:previews')).toMatchObject({ label: 'Show message text', checked: false })
    expect(rows.find((r) => r.id === 'pref:agents')).toMatchObject({ label: "Also notify me for agents' posts", checked: false })
  })

  it('a level of none: "Off (set elsewhere)" is current, and both choices are still offered', () => {
    const rows = homeChatNotifyItems('channel', notifyOf({ level: 'none', effective: 'none' }), PREFS)
    expect(rows.find((r) => r.label === NOTIFY_OFF_ELSEWHERE)).toMatchObject({ checked: true, enabled: false })
    expect(rows.filter((r) => r.id.startsWith('level:')).map((r) => [r.label, r.checked])).toEqual([
      ['All messages', false],
      ['@mentions only', false],
    ])
  })

  it('a DM: the plain line, no choices', () => {
    for (const rows of [homeChatNotifyItems('dm', null, PREFS), homeChatNotifyItems('channel', notifyOf({ convKind: 'dm', fixed: true }), PREFS)]) {
      expect(rows.find((r) => r.label === DM_ALWAYS_NOTIFIES)).toMatchObject({ type: 'note' })
      expect(rows.some((r) => r.id.startsWith('level:'))).toBe(false)
      expect(rows.some((r) => r.id === 'pref:previews')).toBe(true)
    }
  })

  it('the owner keeping text off: Show message text is unchecked, not clickable, and says why', () => {
    const rows = homeChatNotifyItems('channel', notifyOf(), { ...PREFS, previews: true, serverPreviews: false })
    expect(rows.find((r) => r.id === 'pref:previews')).toMatchObject({ checked: false, enabled: false })
    expect(rows.find((r) => r.label === SERVER_TEXT_OFF)).toMatchObject({ type: 'note' })
    const on = homeChatNotifyItems('channel', notifyOf(), { ...PREFS, previews: true })
    expect(on.find((r) => r.id === 'pref:previews')).toMatchObject({ checked: true, enabled: true })
    expect(on.some((r) => r.label === SERVER_TEXT_OFF)).toBe(false)
  })

  it('All messages before allMessagesLive says when it starts; once live it says nothing', () => {
    expect(homeChatNotifyItems('channel', notifyOf({ level: 'all' }), PREFS).find((r) => r.label === ALL_MESSAGES_PENDING)).toMatchObject({ type: 'note' })
    expect(homeChatNotifyItems('channel', notifyOf({ level: 'all', allMessagesLive: true }), PREFS).some((r) => r.label === ALL_MESSAGES_PENDING)).toBe(false)
    expect(homeChatNotifyItems('channel', notifyOf({ level: 'mentions' }), PREFS).some((r) => r.label === ALL_MESSAGES_PENDING)).toBe(false)
  })
})

describe('the Notify menu in the chat view', () => {
  it('is absent on a server without home-chat-push-v1, and reads nothing there', async () => {
    setPushes(false)
    await open()
    expect(screen.queryByTestId('home-chat-notify-button')).toBeNull()
    const routes = [...h.get.mock.calls, ...h.post.mock.calls].map((c) => c[1])
    expect(routes.some((r) => r === 'home-chat/notify' || r === 'home-chat/push-prefs')).toBe(false)
  })

  it('All messages: shows at once, posts {conv, level}, and says when it starts', async () => {
    let answer: (v: unknown) => void = () => undefined
    h.post.mockImplementation(async (_s, route) => (route === 'home-chat/notify/set' ? new Promise((r) => (answer = r)) : { ok: true }))
    await open()
    await waitFor(() => expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe('Notify: @mentions only'))
    const rows = await menu()
    expect(rows.filter((r) => r.id.startsWith('level:')).map((r) => r.label)).toEqual(['All messages', '@mentions only'])
    await pick('level:all')
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/notify/set', { conv: '600', level: 'all' })
    // Shown before the server answers.
    expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe('Notify: All messages')
    await act(async () => {
      answer({ ok: true, ...notifyOf({ level: 'all', effective: 'all', inherited: false }) })
    })
    await waitFor(() => expect(useToastStore.getState().toasts.map((t) => t.message)).toContain(ALL_MESSAGES_PENDING))
  })

  it('a refused pick goes back, with the daemon’s hint', async () => {
    h.post.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/notify/set') throw new Error('{"error":{"code":"not_a_member","hint":"You are not in this channel"}}')
      return { ok: true }
    })
    await open()
    await waitFor(() => expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe('Notify: @mentions only'))
    await menu()
    await pick('level:all')
    await waitFor(() => expect(useToastStore.getState().toasts).toHaveLength(1))
    expect(useToastStore.getState().toasts[0].message).toBe("Couldn't change this channel's notifications (You are not in this channel)")
    expect(useToastStore.getState().toasts[0].type).toBe('warning')
    expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe('Notify: @mentions only')
  })

  it('a level of none shows "Off (set elsewhere)" with both choices', async () => {
    level = 'none'
    await open()
    await waitFor(() => expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe(`Notify: ${NOTIFY_OFF_ELSEWHERE}`))
    const rows = await menu()
    expect(rows.find((r) => r.label === NOTIFY_OFF_ELSEWHERE)).toMatchObject({ checked: true, enabled: false })
    expect(rows.filter((r) => r.id.startsWith('level:')).map((r) => r.checked)).toEqual([false, false])
  })

  it('a toggle posts only what changed; refused, it goes back with the hint', async () => {
    h.post.mockImplementation(async (_s, route, b) => {
      if (route === 'home-chat/push-prefs/set') return { ok: true, ...PREFS, ...(b as object) }
      return { ok: true }
    })
    await open()
    await menu()
    await pick('pref:previews')
    expect(h.post).toHaveBeenCalledWith(expect.anything(), 'home-chat/push-prefs/set', { previews: true })
    const again = await menu()
    expect(again.find((r) => r.id === 'pref:previews')?.checked).toBe(true)
    await act(async () => useContextMenuStore.getState().close())

    h.post.mockImplementation(async (_s, route) => {
      if (route === 'home-chat/push-prefs/set') throw new Error('{"error":{"code":"usage","hint":"previews must be true or false"}}')
      return { ok: true }
    })
    await menu()
    await pick('pref:agents')
    expect(h.post).toHaveBeenLastCalledWith(expect.anything(), 'home-chat/push-prefs/set', { agents: true })
    await waitFor(() => expect(useToastStore.getState().toasts.map((t) => t.message)).toContain("Couldn't change your notification settings (previews must be true or false)"))
    const after = await menu()
    expect(after.find((r) => r.id === 'pref:agents')?.checked).toBe(false)
  })

  it('a DM says "DMs always notify" and never reads or sets a level', async () => {
    const dm: HomeChatTarget = { kind: 'person', hostKey: 'local', ext: 'k2:connect:a', address: 'k2:connect:a::local' }
    h.post.mockImplementation(async (_s, route) => (route === 'home-chat/dm' ? { ok: true, conv: '600' } : { ok: true }))
    await open(dm)
    const rows = await menu()
    expect(rows.find((r) => r.label === DM_ALWAYS_NOTIFIES)).toBeTruthy()
    expect(rows.some((r) => r.id.startsWith('level:'))).toBe(false)
    expect(h.get.mock.calls.some((c) => c[1] === 'home-chat/notify')).toBe(false)
  })

  it('a notify frame (another device) moves the menu, and the level is read again', async () => {
    await open()
    await waitFor(() => expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe('Notify: @mentions only'))
    await waitFor(() => expect(h.frames).not.toBeNull())
    const reads = h.get.mock.calls.filter((c) => c[1] === 'home-chat/notify').length
    level = 'all'
    await act(async () => {
      h.frames?.({ type: 'notify', conv: '600', level: 'all' })
    })
    expect(screen.getByTestId('home-chat-notify-button').getAttribute('aria-label')).toBe('Notify: All messages')
    await waitFor(() => expect(h.get.mock.calls.filter((c) => c[1] === 'home-chat/notify').length).toBe(reads + 1))
  })
})

describe('the viewing lease', () => {
  function frames(send: ReturnType<typeof vi.fn>): Array<{ t: string; conv: string; visible: boolean }> {
    return send.mock.calls.map((c) => JSON.parse(c[0] as string))
  }

  it('held while open and focused; visible:false on blur and on close; sent again on reconnect', async () => {
    let focused = true
    vi.spyOn(document, 'hasFocus').mockImplementation(() => focused)
    await open()
    await waitFor(() => expect(h.link).not.toBeNull())
    const send = vi.fn((_t: string) => true)
    await act(async () => h.link?.(true, send))
    expect(frames(send)).toEqual([{ t: 'viewing', conv: '600', visible: true }])

    focused = false
    await act(async () => {
      window.dispatchEvent(new Event('blur'))
    })
    expect(frames(send).at(-1)).toEqual({ t: 'viewing', conv: '600', visible: false })

    focused = true
    await act(async () => {
      window.dispatchEvent(new Event('focus'))
    })
    expect(frames(send).at(-1)).toEqual({ t: 'viewing', conv: '600', visible: true })

    // The socket drops and comes back: the lease goes again at once.
    await act(async () => h.link?.(false))
    const send2 = vi.fn((_t: string) => true)
    await act(async () => h.link?.(true, send2))
    expect(frames(send2)).toEqual([{ t: 'viewing', conv: '600', visible: true }])

    cleanup()
    expect(frames(send2).at(-1)).toEqual({ t: 'viewing', conv: '600', visible: false })
  })

  it('a hidden window holds no lease', async () => {
    vi.spyOn(document, 'hasFocus').mockImplementation(() => true)
    const vis = vi.spyOn(document, 'visibilityState', 'get').mockReturnValue('hidden')
    await open()
    await waitFor(() => expect(h.link).not.toBeNull())
    const send = vi.fn((_t: string) => true)
    await act(async () => h.link?.(true, send))
    expect(send).not.toHaveBeenCalled()
    vis.mockReturnValue('visible')
    await act(async () => {
      document.dispatchEvent(new Event('visibilitychange'))
    })
    expect(frames(send)).toEqual([{ t: 'viewing', conv: '600', visible: true }])
  })

  it('no lease on a server without home-chat-push-v1', async () => {
    setPushes(false)
    vi.spyOn(document, 'hasFocus').mockImplementation(() => true)
    await open()
    await waitFor(() => expect(h.link).not.toBeNull())
    const send = vi.fn((_t: string) => true)
    await act(async () => h.link?.(true, send))
    expect(send).not.toHaveBeenCalled()
  })
})

describe('mention highlight', () => {
  it('my member id, @channel and @here are marked; someone else’s and my own are not', async () => {
    await open()
    const marked = (id: string) => document.querySelector(`[data-home-chat-message][data-zen-message="${id}"]`)?.hasAttribute('data-mention')
    await waitFor(() => expect(marked('21')).toBe(true))
    expect(marked('22')).toBe(true)
    expect(marked('23')).toBe(true)
    expect(marked('20')).toBe(false)
    expect(marked('24')).toBe(false)
    expect(marked('25')).toBe(false)
  })
})
