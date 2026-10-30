// The Mobile tab's adapters against Builder A's frozen contract
// (k2-core::sim::api and ::stream), fail loud:
//  - route bodies parse into SimStatus / SimView / devices, and anything
//    else throws;
//  - the K2SF frame header, the `t`-tagged server messages, close codes;
//  - the stub stream the pane and screenshots run on.

import { describe, expect, it, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))

import { firstUsableLane, parseSimDevices, parseSimHold, parseSimStatus } from './simApi'
import {
  closeReason,
  createStubSimStream,
  decodeFrame,
  defaultFps,
  encodeFrame,
  parseServerMsg,
  simStreamUrl,
  type SimStreamHandlers,
} from './sim-stream'

const profile = { id: 'pixel_9', name: 'Pixel 9', widthPt: 412, heightPt: 915, scale: 2.625, os: 'android-35 google_apis x86_64', default: true }
const view = {
  sim: 's1',
  workspace: 'p-docs',
  workspaceHandle: 'docs',
  lane: 'android',
  state: 'running',
  profile,
  appId: 'com.acme.app',
  lastUsedAt: 1,
  hold: { kind: 'person', by: 'rosson', ms: 2000 },
  viewers: 2,
}

describe('route adapter parses (sim::api)', () => {
  it('reads a status: lanes with needs, the device, and no canControl (null)', () => {
    const st = parseSimStatus({
      lanes: [
        { lane: 'ios', usable: false, ready: false, detail: 'Not a Mac.' },
        { lane: 'android', usable: true, ready: false, needs: [{ kind: 'licence', id: 'android-sdk-license', title: 'Android SDK licence' }] },
      ],
      sim: view,
      running: 1,
      limits: {},
      agentsAllowed: false,
    })
    expect(st.canControl).toBeNull()
    expect(st.running).toBe(1)
    expect(st.lanes[0].detail).toBe('Not a Mac.')
    expect(st.lanes[1].needs).toEqual([{ kind: 'licence', id: 'android-sdk-license', title: 'Android SDK licence' }])
    expect(st.sim).toEqual({
      sim: 's1',
      workspace: 'p-docs',
      workspaceHandle: 'docs',
      lane: 'android',
      state: 'running',
      profile,
      appId: 'com.acme.app',
      hold: { kind: 'person', by: 'rosson', ms: 2000 },
      viewers: 2,
    })
    expect(firstUsableLane(st.lanes)?.lane).toBe('android')
    expect(parseSimStatus({ lanes: [], running: 0, limits: {}, agentsAllowed: true, canControl: false }).canControl).toBe(false)
  })

  it('reads every hold, including Take over’s idle_ms as the daemon sends it today', () => {
    expect(parseSimHold({ kind: 'free' })).toEqual({ kind: 'free' })
    expect(parseSimHold({ kind: 'takeover', by: 'jules', idle_ms: 600000 })).toEqual({ kind: 'takeover', by: 'jules', idleMs: 600000 })
    expect(parseSimHold({ kind: 'takeover', by: 'jules', idleMs: 5 })).toEqual({ kind: 'takeover', by: 'jules', idleMs: 5 })
    expect(() => parseSimHold({ kind: 'mine' })).toThrow(/sim hold/)
  })

  it('refuses bodies it does not understand', () => {
    expect(() => parseSimStatus({ lanes: [], sim: null })).toThrow(/unexpected sim status/)
    expect(() => parseSimStatus({ lanes: [{ lane: 'windows', usable: true, ready: true }], agentsAllowed: true })).toThrow(/sim lane/)
    expect(() => parseSimStatus({ lanes: [], sim: { ...view, state: 'dancing' }, agentsAllowed: true })).toThrow(/sim view/)
    expect(() =>
      parseSimStatus({ lanes: [{ lane: 'android', usable: true, ready: false, needs: [{ kind: 'pray' }] }], agentsAllowed: true }),
    ).toThrow(/sim lane need/)
    expect(() => parseSimDevices({ lane: 'android', devices: [{ id: 'x', name: 'X' }] })).toThrow(/sim device/)
    expect(() => parseSimDevices({ devices: [] })).toThrow(/sim devices/)
    expect(parseSimDevices({ lane: 'android', devices: [profile] })).toEqual({ lane: 'android', devices: [profile] })
  })
})

describe('stream adapter (sim::stream)', () => {
  it('builds the socket URL with the workspace, the fps cap and the token', () => {
    expect(simStreamUrl('ws://127.0.0.1:9', 'p-docs', 'tok', 8)).toBe('ws://127.0.0.1:9/cli/sim/stream?workspace=p-docs&fps=8&token=tok')
    expect(simStreamUrl('wss://box.k2.dev', 'my docs', '', 30)).toBe('wss://box.k2.dev/cli/sim/stream?workspace=my+docs&fps=30')
    // 0.46.7: the offer rides the query (an older daemon ignores it).
    expect(simStreamUrl('ws://127.0.0.1:9', 'p-docs', 'tok', 8, { codecs: ['h264', 'jpeg'], window: 3 })).toBe(
      'ws://127.0.0.1:9/cli/sim/stream?workspace=p-docs&fps=8&codecs=h264%2Cjpeg&win=3&token=tok',
    )
    expect(simStreamUrl('ws://h', 'w', null, 30, { codecs: ['jpeg'], window: 0 })).toBe('ws://h/cli/sim/stream?workspace=w&fps=30&codecs=jpeg&win=1')
    expect(defaultFps(false)).toBe(30)
    expect(defaultFps(true)).toBe(8)
  })

  it('decodes the 16-byte K2SF header (LE) and refuses anything else', () => {
    const buf = encodeFrame({ seq: 77, widthPx: 1081, heightPx: 2401, widthPt: 412, heightPt: 915 }, new Uint8Array([0xff, 0xd8, 0xff]))
    expect(buf.byteLength).toBe(19)
    expect(Array.from(new Uint8Array(buf).slice(0, 16))).toEqual([
      0x4b, 0x32, 0x53, 0x46, 77, 0, 0, 0, 0x39, 0x04, 0x61, 0x09, 0x9c, 0x01, 0x93, 0x03,
    ])
    const f = decodeFrame(buf)
    if (!f) throw new Error('no frame')
    expect({ ...f, jpeg: Array.from(f.jpeg) }).toEqual({
      seq: 77,
      widthPx: 1081,
      heightPx: 2401,
      widthPt: 412,
      heightPt: 915,
      jpeg: [0xff, 0xd8, 0xff],
    })
    expect(decodeFrame(new ArrayBuffer(8))).toBeNull()
    const bad = new Uint8Array(buf.slice(0))
    bad[0] = 0x58
    expect(decodeFrame(bad.buffer)).toBeNull()
  })

  it('reads every server message and says what close codes mean', () => {
    expect(parseServerMsg('{"t":"hold","hold":{"kind":"person","by":"rosson","ms":2000}}')).toEqual({
      t: 'hold',
      hold: { kind: 'person', by: 'rosson', ms: 2000 },
    })
    expect(parseServerMsg('{"t":"state","state":"running"}')).toEqual({ t: 'state', state: 'running' })
    expect(parseServerMsg('{"t":"fps","fps":8}')).toEqual({ t: 'fps', fps: 8 })
    expect(parseServerMsg('{"t":"error","code":"sim_held","message":"held"}')).toEqual({ t: 'error', code: 'sim_held', message: 'held' })
    expect(
      parseServerMsg(
        '{"t":"hello","sim":"s1","lane":"android","state":"running","fps":30,"remote":false,"hold":{"kind":"free"},"isolation":"Separate, not sealed."}',
      ),
    ).toEqual({
      t: 'hello',
      sim: 's1',
      lane: 'android',
      state: 'running',
      fps: 30,
      remote: false,
      hold: { kind: 'free' },
      isolation: 'Separate, not sealed.',
      // A hello from a daemon before 0.46.7 has neither: JPEG, one in flight.
      codec: 'jpeg',
      window: 1,
    })
    expect(
      parseServerMsg('{"t":"hello","sim":"s1","lane":"ios","state":"running","fps":24,"remote":true,"hold":{"kind":"free"},"isolation":"x","codec":"h264","window":3}'),
    ).toMatchObject({ t: 'hello', codec: 'h264', window: 3, fps: 24, remote: true })
    expect(parseServerMsg('{"t":"hello","sim":"s1","lane":"ios","state":"running","fps":24,"remote":true,"hold":{"kind":"free"},"isolation":"x","codec":"av1"}')).toBeNull()
    expect(parseServerMsg('{"t":"codec","codec":"jpeg","message":"The video encoder stopped."}')).toEqual({
      t: 'codec',
      codec: 'jpeg',
      message: 'The video encoder stopped.',
    })
    expect(parseServerMsg('{"t":"codec","codec":"vp9"}')).toBeNull()
    // 0.46.7: hello.input (stream::InputCaps); an older daemon sends none.
    expect(
      parseServerMsg(
        '{"t":"hello","sim":"s1","lane":"android","state":"running","fps":30,"remote":false,"hold":{"kind":"free"},"isolation":"","input":{"multiTouch":true,"fling":true}}',
      ),
    ).toMatchObject({ t: 'hello', input: { multiTouch: true, fling: true } })
    expect(
      parseServerMsg('{"t":"hello","sim":"s1","lane":"ios","state":"running","fps":30,"remote":false,"hold":{"kind":"free"},"isolation":"","input":{"multiTouch":false,"fling":true}}'),
    ).toMatchObject({ input: { multiTouch: false, fling: true } })
    expect(parseServerMsg('{"t":"state","state":"dancing"}')).toBeNull()
    expect(parseServerMsg('not json')).toBeNull()
    expect(closeReason(4401, '')).toBe('Your login was signed out of this server.')
    expect(closeReason(4403, '')).toBe('This login may not watch this device.')
    expect(closeReason(4410, '')).toBe('The device stopped.')
    expect(closeReason(1006, '')).toBe('The device stream closed.')
  })

  it('the stub says hello, pushes frames with the screen size, records input and rotates', async () => {
    const stub = createStubSimStream()
    const seen: string[] = []
    const handlers: SimStreamHandlers = {
      onFrame: (f) => seen.push(`frame:${f.seq}:${f.widthPt}x${f.heightPt}:${f.source.kind}`),
      onServer: (m) => seen.push(`server:${m.t}`),
      onConn: (s) => seen.push(`conn:${s}`),
    }
    const conn = stub.connector({ scope: {} as never, workspace: 'p-docs', fps: 30, handlers })
    await Promise.resolve()
    stub.pushFrame()
    conn.send({ t: 'tap', x: 1, y: 2 })
    conn.send({ t: 'rotate', orientation: 'landscape-right' })
    stub.pushFrame()
    conn.ack(1)
    conn.setFps(12)
    stub.pushServer({ t: 'hold', hold: { kind: 'takeover', by: 'x', idleMs: 1 } })
    expect(stub.openCount()).toBe(1)
    expect(stub.lastWorkspace()).toBe('p-docs')
    conn.close()
    expect(stub.openCount()).toBe(0)
    stub.pushFrame()
    expect(seen).toEqual(['conn:connecting', 'conn:open', 'server:hello', 'frame:1:412x915:paint', 'frame:2:915x412:paint', 'server:hold'])
    expect(stub.inputs).toEqual([
      { t: 'tap', x: 1, y: 2 },
      { t: 'rotate', orientation: 'landscape-right' },
    ])
    expect(stub.acks).toEqual([1])
    expect(stub.fpsAsks).toEqual([12])
  })
})
