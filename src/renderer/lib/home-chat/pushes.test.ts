// Home chat pushes, the client's wire (prd-home-chat-pushes-v1 §7, HC-14,
// HC-15):
//   1. the notify and push-prefs routes: one path per method, the bodies
//      exactly as the daemon reads them;
//   2. a message's mentions, and when one is the viewer's;
//   3. the daemon's `notify` frame;
//   4. the viewing lease: sent at once and every 30 s, `visible:false` at
//      once on a change or stop, sent again when the socket reconnects;
//   5. the events socket's `send`, handed over by `onLink(true, send)`.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const cli = vi.hoisted(() => ({ get: vi.fn(), post: vi.fn() }))
vi.mock('@/lib/daemon-cli', () => ({ daemonCliGet: cli.get, daemonCliPost: cli.post }))

import type { ServerScope } from '@/kessel/server-scope'
import {
  fetchHomeChatNotify,
  fetchHomeChatPushPrefs,
  mentionsMe,
  parseHomeChatMessage,
  parseHomeChatNotify,
  setHomeChatNotify,
  setHomeChatPushPrefs,
} from './api'
import { parseHomeChatFrame, subscribeHomeChatEvents, type HomeChatEventsDeps, type HomeChatEventsSend } from './events-socket'
import { VIEWING_LEASE_REPEAT_MS, createViewingLease, viewingFrame } from './viewing-lease'

const scope = { hostKey: 'box.k2.dev' } as unknown as ServerScope

const NOTIFY = { ok: true, conv: '600', convKind: 'channel', level: 'mentions', effective: 'mentions', inherited: true, fixed: false, muted: false, allMessagesLive: false }
const PREFS = { ok: true, on: true, previews: false, agents: true, serverPreviews: true, bell: true, devices: 2, allMessagesLive: true }

beforeEach(() => {
  cli.get.mockReset()
  cli.post.mockReset()
})

describe('the notify and push-prefs routes', () => {
  it('GET home-chat/notify names the conversation; the answer is read whole', async () => {
    cli.get.mockResolvedValueOnce(NOTIFY)
    const n = await fetchHomeChatNotify(scope, '600')
    expect(cli.get).toHaveBeenCalledWith(scope, 'home-chat/notify', { conv: '600' })
    expect(n).toEqual({ conv: '600', convKind: 'channel', level: 'mentions', effective: 'mentions', inherited: true, fixed: false, muted: false, allMessagesLive: false })
  })

  it('POST home-chat/notify/set carries {conv, level}', async () => {
    cli.post.mockResolvedValueOnce({ ...NOTIFY, level: 'all', effective: 'all', inherited: false })
    const n = await setHomeChatNotify(scope, '600', 'all')
    expect(cli.post).toHaveBeenCalledWith(scope, 'home-chat/notify/set', { conv: '600', level: 'all' })
    expect(n.level).toBe('all')
    expect(n.inherited).toBe(false)
  })

  it('a refusal (409 dm_always_notifies) reaches the caller', async () => {
    cli.post.mockRejectedValueOnce(new Error('{"error":{"code":"dm_always_notifies","hint":"DMs always notify"}}'))
    await expect(setHomeChatNotify(scope, '700', 'mentions')).rejects.toThrow(/dm_always_notifies/)
  })

  it('a DM is fixed; an answer with no level is an error, never a guess', () => {
    expect(parseHomeChatNotify({ ...NOTIFY, conv: 700, convKind: 'dm', level: 'all', fixed: true })).toMatchObject({ conv: '700', convKind: 'dm', fixed: true })
    expect(() => parseHomeChatNotify({ ...NOTIFY, level: 'off' })).toThrow(/no notify level/)
    expect(() => parseHomeChatNotify({ ...NOTIFY, conv: null })).toThrow(/no notify level/)
  })

  it('GET home-chat/push-prefs; POST home-chat/push-prefs/set sends only what changed', async () => {
    cli.get.mockResolvedValueOnce(PREFS)
    expect(await fetchHomeChatPushPrefs(scope)).toEqual({ on: true, previews: false, agents: true, serverPreviews: true, bell: true, devices: 2, allMessagesLive: true })
    expect(cli.get).toHaveBeenCalledWith(scope, 'home-chat/push-prefs')

    cli.post.mockResolvedValueOnce({ ...PREFS, previews: true })
    const p = await setHomeChatPushPrefs(scope, { previews: true })
    expect(cli.post).toHaveBeenCalledWith(scope, 'home-chat/push-prefs/set', { previews: true })
    expect(p.previews).toBe(true)

    cli.post.mockResolvedValueOnce({ ...PREFS, agents: false })
    await setHomeChatPushPrefs(scope, { agents: false, previews: undefined })
    expect(cli.post).toHaveBeenLastCalledWith(scope, 'home-chat/push-prefs/set', { agents: false })
  })
})

describe('mentions', () => {
  const base = { id: '1', channel_id: '600', author_id: '100', body: 'hey' }
  it('reads Sew’s mentions; my member id, @channel and @here count', () => {
    const mine = parseHomeChatMessage({ ...base, mentions: { members: [1, '7'], channel: false, here: false } })!
    expect(mine.mentions).toEqual({ members: ['1', '7'], channel: false, here: false })
    expect(mentionsMe(mine, '7')).toBe(true)
    expect(mentionsMe(mine, '8')).toBe(false)
    expect(mentionsMe(mine, null)).toBe(false)
    expect(mentionsMe(parseHomeChatMessage({ ...base, mentions: { members: [], channel: true } })!, '8')).toBe(true)
    expect(mentionsMe(parseHomeChatMessage({ ...base, mentions: { members: [], here: true } })!, '8')).toBe(true)
  })

  it('no mentions, an empty one, or my own message: not a mention', () => {
    expect(parseHomeChatMessage(base)!.mentions).toBeNull()
    expect(parseHomeChatMessage({ ...base, mentions: { members: [], channel: false, here: false } })!.mentions).toBeNull()
    expect(mentionsMe(parseHomeChatMessage({ ...base, mentions: { members: ['100'], channel: true } })!, '100')).toBe(false)
  })
})

describe('the notify frame', () => {
  it('parses a level, or null for one the client does not know', () => {
    expect(parseHomeChatFrame(JSON.stringify({ type: 'notify', conv: 600, level: 'all' }))).toEqual({ type: 'notify', conv: '600', level: 'all' })
    expect(parseHomeChatFrame(JSON.stringify({ type: 'notify', conv: '600', level: 'none' }))).toEqual({ type: 'notify', conv: '600', level: 'none' })
    expect(parseHomeChatFrame(JSON.stringify({ type: 'notify', conv: '600', level: null }))).toEqual({ type: 'notify', conv: '600', level: null })
    expect(parseHomeChatFrame(JSON.stringify({ type: 'notify', level: 'all' }))).toBeNull()
  })
})

describe('the viewing lease', () => {
  beforeEach(() => {
    vi.useFakeTimers()
  })
  afterEach(() => {
    vi.useRealTimers()
  })

  function lease() {
    const sent: unknown[] = []
    let open = true
    const l = createViewingLease({
      send: (t) => {
        if (!open) return false
        sent.push(JSON.parse(t))
        return true
      },
      setInterval: (fn, ms) => setInterval(fn, ms),
      clearInterval: (t) => clearInterval(t as ReturnType<typeof setInterval>),
    })
    return { l, sent, setOpen: (v: boolean) => (open = v) }
  }

  it('the frame is exactly {"t":"viewing","conv","visible"}', () => {
    expect(viewingFrame('600', true)).toBe('{"t":"viewing","conv":"600","visible":true}')
    expect(viewingFrame('600', false)).toBe('{"t":"viewing","conv":"600","visible":false}')
  })

  it('sends at once, then every 30 s while held', () => {
    const { l, sent } = lease()
    l.set('600')
    expect(sent).toEqual([{ t: 'viewing', conv: '600', visible: true }])
    vi.advanceTimersByTime(VIEWING_LEASE_REPEAT_MS - 1)
    expect(sent).toHaveLength(1)
    vi.advanceTimersByTime(1)
    expect(sent).toHaveLength(2)
    vi.advanceTimersByTime(VIEWING_LEASE_REPEAT_MS * 2)
    expect(sent).toHaveLength(4)
    expect(sent.every((f) => (f as { visible: boolean }).visible)).toBe(true)
    // The same conversation again sends nothing new.
    l.set('600')
    expect(sent).toHaveLength(4)
  })

  it('blur (null) sends visible:false at once and stops the repeats', () => {
    const { l, sent } = lease()
    l.set('600')
    l.set(null)
    expect(sent).toEqual([
      { t: 'viewing', conv: '600', visible: true },
      { t: 'viewing', conv: '600', visible: false },
    ])
    vi.advanceTimersByTime(VIEWING_LEASE_REPEAT_MS * 3)
    expect(sent).toHaveLength(2)
    expect(l.current()).toBeNull()
  })

  it('switching conversation ends the old one first, then holds the new one', () => {
    const { l, sent } = lease()
    l.set('600')
    l.set('601')
    expect(sent).toEqual([
      { t: 'viewing', conv: '600', visible: true },
      { t: 'viewing', conv: '600', visible: false },
      { t: 'viewing', conv: '601', visible: true },
    ])
    vi.advanceTimersByTime(VIEWING_LEASE_REPEAT_MS)
    expect(sent[3]).toEqual({ t: 'viewing', conv: '601', visible: true })
    expect(sent).toHaveLength(4)
  })

  it('a reconnected socket gets the current lease again at once', () => {
    const { l, sent, setOpen } = lease()
    setOpen(false)
    l.set('600')
    expect(sent).toHaveLength(0)
    setOpen(true)
    l.linked()
    expect(sent).toEqual([{ t: 'viewing', conv: '600', visible: true }])
    // The 30 s count starts again from the reconnect (one timer, not two).
    vi.advanceTimersByTime(VIEWING_LEASE_REPEAT_MS)
    expect(sent).toHaveLength(2)
  })

  it('with no conversation, a reconnect sends nothing; stop ends the lease', () => {
    const { l, sent } = lease()
    l.linked()
    expect(sent).toHaveLength(0)
    l.set('600')
    l.stop()
    expect(sent[1]).toEqual({ t: 'viewing', conv: '600', visible: false })
    vi.advanceTimersByTime(VIEWING_LEASE_REPEAT_MS * 2)
    expect(sent).toHaveLength(2)
  })
})

describe('the events socket hands over a send', () => {
  class FakeSocket {
    readyState = 0
    sent: string[] = []
    onopen: (() => void) | null = null
    onmessage: ((ev: MessageEvent) => void) | null = null
    onclose: ((ev: CloseEvent) => void) | null = null
    send(t: string): void {
      this.sent.push(t)
    }
    close(): void {
      this.readyState = 3
    }
  }

  it('onLink(true, send) writes on the open socket; false once it closed', async () => {
    const socks: FakeSocket[] = []
    const deps: HomeChatEventsDeps = {
      open: () => {
        const s = new FakeSocket()
        socks.push(s)
        return s as unknown as WebSocket
      },
      setTimer: () => 0,
      clearTimer: () => {},
    }
    const s = { hostKey: 'box.k2.dev', wsBase: async () => 'ws://box.k2.dev', creds: async () => ({ token: 't' }) } as unknown as ServerScope
    const links: Array<[boolean, HomeChatEventsSend | undefined]> = []
    const stop = subscribeHomeChatEvents(s, () => {}, (up, send) => links.push([up, send]), deps)
    await vi.waitFor(() => expect(socks).toHaveLength(1))
    socks[0].readyState = 1
    socks[0].onopen?.()
    expect(links).toHaveLength(1)
    const [up, send] = links[0]
    expect(up).toBe(true)
    if (!send) throw new Error('no send handed over')
    expect(send('{"t":"viewing","conv":"600","visible":true}')).toBe(true)
    expect(socks[0].sent).toEqual(['{"t":"viewing","conv":"600","visible":true}'])
    socks[0].readyState = 3
    socks[0].onclose?.({ code: 1006 } as CloseEvent)
    expect(links[1]).toEqual([false, undefined])
    expect(send('x')).toBe(false)
    stop()
  })
})
