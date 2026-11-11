// The Desktop socket (`wsDesktopStream`) and the viewer's session model
// (`DesktopSession`), fail loud:
//  - every dial mints its own ticket; the URL carries only the ticket (no
//    token, even on a scope whose creds hold one); the offer is what this
//    viewer decodes (H.264, VP8, JPEG by feature check);
//  - a ticket refusal says why and never dials;
//  - text and binary messages reach the handlers; a close says why;
//  - the session: take / hand back (held keys released first), the
//    display is the controller's, hidden = fps 0, shown = rate + keyframe.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

const daemon = vi.hoisted(() => ({ tickets: 0, refuse: null as null | { status: number; code: string } }))
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  const { DaemonHttpError } = await vi.importActual<typeof import('@/lib/daemon-http-error')>('@/lib/daemon-http-error')
  return {
    ...actual,
    daemonCliPost: vi.fn(async (_scope: unknown, route: string) => {
      if (route !== 'desktop/ticket') throw new Error(`unexpected POST ${route}`)
      if (daemon.refuse) throw new DaemonHttpError('refused', daemon.refuse.status, daemon.refuse.code)
      daemon.tickets += 1
      return { ok: true, ticket: `k2dt_t${daemon.tickets}`, expiresIn: 60 }
    }),
  }
})

import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import type { ServerScope } from '@/kessel/server-scope'
import { fakeScope } from '@/test-utils/fake-scope'
import { encodeFrame } from '@/components/Stream/stream-wire'
import { FakeWebSocket } from '@/components/Stream/testing'
import { desktopOffer, wsDesktopStream, type DesktopStreamHandlers } from './desktop-stream'
import { DesktopSession } from './desktop-session'
import { CLOSE_AUTH_REVOKED, CLOSE_DESKTOP_OFF, CLOSE_FORBIDDEN, desktopStreamUrl, reconnectsAfter, type ServerMsg } from './desktop-wire'

const scope = (): ServerScope =>
  ({ ...fakeScope('box.test'), creds: async () => ({ host: 'box.test', port: 443, secure: true, token: 'tok-SECRET' }) }) as unknown as ServerScope

async function until(cond: () => boolean): Promise<void> {
  for (let i = 0; i < 200 && !cond(); i++) await Promise.resolve()
  if (!cond()) throw new Error('never happened')
}

function recorder(): DesktopStreamHandlers & { conns: Array<[string, string | undefined, number | undefined]>; server: ServerMsg[]; frames: number[] } {
  const r = {
    conns: [] as Array<[string, string | undefined, number | undefined]>,
    server: [] as ServerMsg[],
    frames: [] as number[],
    onConn: (s: string, why?: string, code?: number) => r.conns.push([s, why, code]),
    onServer: (m: ServerMsg) => r.server.push(m),
    onFrame: (f: { seq: number }) => r.frames.push(f.seq),
  }
  return r
}

beforeEach(() => {
  daemon.tickets = 0
  daemon.refuse = null
  FakeWebSocket.reset()
  resetGridDialQueueForTests()
  vi.stubGlobal('WebSocket', FakeWebSocket)
})
afterEach(() => vi.unstubAllGlobals())

describe('wsDesktopStream', () => {
  it('a fresh ticket per dial, only the ticket in the URL, the offer by decode support', async () => {
    const h = recorder()
    const all = { h264: true, vp8: true, jpeg: true }
    wsDesktopStream({ scope: scope(), decode: all, window: 3, display: '2', view: true, stats: true, pw: 1600, handlers: h })
    await until(() => FakeWebSocket.instances.length === 1)
    const u = new URL(FakeWebSocket.last().url)
    expect(u.origin + u.pathname).toBe('wss://box.test/cli/desktop/stream')
    expect(Object.fromEntries(u.searchParams)).toEqual({ ticket: 'k2dt_t1', codecs: 'h264,vp8,jpeg', win: '3', display: '2', view: '1', stats: '1', pw: '1600' })
    expect(FakeWebSocket.last().url).not.toContain('tok-SECRET')
    wsDesktopStream({ scope: scope(), decode: { h264: false, vp8: false, jpeg: true }, window: 3, handlers: recorder() })
    await until(() => FakeWebSocket.instances.length === 2)
    const v = new URL(FakeWebSocket.last().url)
    expect(v.searchParams.get('ticket')).toBe('k2dt_t2')
    expect(v.searchParams.get('codecs')).toBe('jpeg')
    expect(v.searchParams.has('token')).toBe(false)
  })

  it('a ticket refusal says why and never dials', async () => {
    daemon.refuse = { status: 403, code: 'not_allowed' }
    const h = recorder()
    wsDesktopStream({ scope: scope(), decode: { h264: true, vp8: false, jpeg: true }, window: 3, handlers: h })
    await until(() => h.conns.length === 2)
    expect(h.conns[1]).toEqual(['error', "You don't have access to this computer's screen.", 403])
    expect(FakeWebSocket.instances).toHaveLength(0)
  })

  it('reads text and frames, sends input queued before open in order, and says why it closed', async () => {
    const h = recorder()
    const c = wsDesktopStream({ scope: scope(), decode: { h264: false, vp8: false, jpeg: true }, window: 3, handlers: h })
    c.send({ t: 'take-control' })
    await until(() => FakeWebSocket.instances.length === 1)
    const s = FakeWebSocket.last()
    s.open()
    expect(s.sentJson()).toEqual([{ t: 'take-control' }])
    s.serverText({ t: 'control', controller: 'rosson', you: true, localHold: false })
    s.serverBinary(encodeFrame({ seq: 4, widthPx: 2, heightPx: 2, widthPt: 2, heightPt: 2 }, new Uint8Array([0xff, 0xd8])))
    c.ack(4)
    expect(h.server).toEqual([{ t: 'control', controller: 'rosson', you: true, localHold: false }])
    expect(h.frames).toEqual([4])
    expect(s.sentJson().at(-1)).toEqual({ t: 'ack', seq: 4 })
    s.serverClose(CLOSE_FORBIDDEN, 'kicked')
    expect(h.conns.at(-1)).toEqual(['closed', 'The owner disconnected you.', CLOSE_FORBIDDEN])
  })

  it('offers JPEG by feature check, never nothing', () => {
    expect(desktopOffer({ h264: false, vp8: false, jpeg: false })).toEqual(['jpeg'])
    expect(desktopOffer({ h264: false, vp8: true, jpeg: false })).toEqual(['vp8'])
    expect(desktopOffer({ h264: true, vp8: true, jpeg: true })).toEqual(['h264', 'vp8', 'jpeg'])
    expect(desktopStreamUrl('ws://x', { ticket: 'k2dt_a', codecs: [], window: 0 })).toBe('ws://x/cli/desktop/stream?ticket=k2dt_a&codecs=jpeg&win=1')
  })

  it('reconnects only after a drop', () => {
    expect(reconnectsAfter(1006)).toBe(true)
    for (const code of [CLOSE_AUTH_REVOKED, CLOSE_FORBIDDEN, CLOSE_DESKTOP_OFF, 1000]) expect(reconnectsAfter(code), String(code)).toBe(false)
  })
})

describe('DesktopSession', () => {
  const hello = (mode: 'view' | 'control'): ServerMsg => ({
    t: 'hello', desktop: 'console', display: '1',
    displays: [
      { id: '1', name: 'A', widthPt: 10, heightPt: 10, widthPx: 20, heightPx: 20, scale: 2, main: true },
      { id: '2', name: 'B', widthPt: 10, heightPt: 10, widthPx: 10, heightPx: 10, scale: 1, main: false },
    ],
    codec: 'h264', window: 3, fps: 60, path: 'lan', lane: 'main', encoder: 'hardware', mode, controller: mode === 'control' ? 'me' : null,
    os: 'macos', localWins: true, sameMachine: false,
  })

  it('take and hand back: held keys and buttons are released first, newest first', () => {
    const s = new DesktopSession({ mayControl: true })
    s.onServer(hello('view'))
    expect(s.takeControl()).toEqual({ send: [{ t: 'take-control' }] })
    expect(s.handBack()).toEqual({ refused: 'not_controller' })
    s.onServer({ t: 'control', controller: 'me', you: true, localHold: false })
    expect(s.takeControl()).toEqual({ refused: 'already' })
    s.sent({ t: 'key', code: 'ShiftLeft', down: true })
    s.sent({ t: 'button', button: 'left', down: true })
    expect(s.handBack()).toEqual({ send: [{ t: 'button', button: 'left', down: false }, { t: 'key', code: 'ShiftLeft', down: false }, { t: 'hand-back' }] })
    expect(new DesktopSession({ mayControl: false }).takeControl()).toEqual({ refused: 'watch_only' })
  })

  it('the display is the controller’s; hidden pauses, shown asks the rate and a key', () => {
    const s = new DesktopSession({ mayControl: true })
    s.onServer(hello('view'))
    expect(s.selectDisplay('2')).toEqual({ refused: 'not_controller' })
    s.onServer({ t: 'control', controller: 'me', you: true, localHold: false })
    expect(s.selectDisplay('2')).toEqual({ send: [{ t: 'display', id: '2' }] })
    expect(s.selectDisplay('9')).toEqual({ refused: 'already' })
    expect(s.setHidden(true)).toEqual([{ t: 'fps', fps: 0 }])
    expect(s.setHidden(true)).toEqual([])
    expect(s.onServer({ t: 'fps', fps: 2 })).toBe(false)
    expect(s.drives(false)).toBe(false)
    expect(s.setHidden(false)).toEqual([{ t: 'fps', fps: 60 }, { t: 'keyframe' }])
    expect(s.drives(false)).toBe(true)
    expect(s.drives(true)).toBe(false)
    s.onServer({ t: 'displays', displays: s.displays, display: '2' })
    expect(s.currentDisplay()?.name).toBe('B')
  })
})
