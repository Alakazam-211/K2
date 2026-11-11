// @vitest-environment jsdom
// The Desktop pane's video and input, D7b (prd-remote-desktop-tab-v1 §6,
// §8; vs-live R20), against a fake daemon and the engine's fake socket,
// fail loud:
//  - it mints a ticket and dials /cli/desktop/stream with it and no token;
//    a viewer with no video decoder offers `jpeg` only;
//  - Take control sends `take-control`; once the daemon says `you`, keys
//    go, and Hand back releases what is held, then `hand-back`;
//  - a hidden tab sends `fps 0` and keeps its socket; shown again, its
//    rate and a `keyframe`;
//  - a pointer lands on the right display point at Fit and at 1:1;
//  - the display picker is the controller's only;
//  - the cursor overlay moves with no React render.

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { act, cleanup, fireEvent } from '@testing-library/react'
import { Profiler, useState } from 'react'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({ emit: vi.fn(async () => {}), listen: vi.fn(async () => () => {}) }))

const DISPLAY = { id: '1', name: 'Built-in', widthPt: 1600, heightPt: 900, widthPx: 3200, heightPx: 1800, scale: 2, main: true }
const SIDE = { id: '2', name: 'Side', widthPt: 1920, heightPt: 1080, widthPx: 1920, heightPx: 1080, scale: 1, main: false }

const daemon = vi.hoisted(() => ({
  status: null as unknown,
  posts: [] as Array<{ route: string; body: unknown }>,
  tickets: 0,
}))
vi.mock('@/stores/session-events', async () => {
  const actual = await vi.importActual<typeof import('@/stores/session-events')>('@/stores/session-events')
  return { ...actual, onDesktopState: () => () => {} }
})
vi.mock('@/lib/daemon-cli', async () => {
  const actual = await vi.importActual<typeof import('@/lib/daemon-cli')>('@/lib/daemon-cli')
  return {
    ...actual,
    daemonCliGet: vi.fn(async (_scope: unknown, route: string) => {
      if (route === 'themes/list') return []
      if (route === 'desktop/status') return daemon.status
      if (route === 'desktop/displays') return { displays: [] }
      throw new Error(`unexpected GET ${route}`)
    }),
    daemonCliPost: vi.fn(async (_scope: unknown, route: string, body: unknown) => {
      daemon.posts.push({ route, body })
      if (route === 'desktop/ticket') {
        daemon.tickets += 1
        return { ok: true, ticket: `k2dt_test${daemon.tickets}`, expiresIn: 60 }
      }
      throw new Error(`unexpected POST ${route}`)
    }),
  }
})

import { DesktopPane } from './DesktopPane'
import type { DesktopStatus } from './desktop-state'
import { renderInRoom, testRoom } from '@/test-utils/room'
import { fakeScope } from '@/test-utils/fake-scope'
import { resetGridDialQueueForTests } from '@/lib/grid-dial-queue'
import { resetVideoSupportForTests } from '@/components/Stream/stream-video'
import { FakeWebSocket } from '@/components/Stream/testing'
import { TabVisibilityContext } from '@/contexts/TabVisibilityContext'
import type { ServerScope } from '@/kessel/server-scope'
import type { DesktopItemData } from '@/stores/tabs'

const setDesktopItemState = vi.fn()
const tabs = { getState: () => ({ setDesktopItemState }) }

function status(): DesktopStatus {
  return {
    desktop: 'console', machine: 'office-mini', enabled: true, backend: 'macos', state: 'ready', message: '', lane: 'main',
    you: { role: 'owner', mayView: true, mayControl: true, mayManage: true, sameMachine: false },
  }
}

const scope = (): ServerScope =>
  ({ ...fakeScope('b.k2.dev'), creds: async () => ({ host: 'b.k2.dev', port: 443, secure: true, token: 'tok-SECRET' }) }) as unknown as ServerScope

let setShown: (v: boolean) => void = () => {}
let renders = 0

function Harness(props: { data: DesktopItemData }): React.JSX.Element {
  const [shown, set] = useState(true)
  setShown = set
  return (
    <TabVisibilityContext.Provider value={shown}>
      <Profiler id="desktop" onRender={() => { renders++ }}>
        <DesktopPane tabId="t1" paneGroupId="pg1" itemId="i1" data={props.data} />
      </Profiler>
    </TabVisibilityContext.Provider>
  )
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i++) await act(async () => {})
}

/** Render, wait for the dial, open the socket and say hello. */
async function openPane(data: DesktopItemData = { desktopId: 'console' }, hello: Record<string, unknown> = {}): Promise<FakeWebSocket> {
  await act(async () => {
    renderInRoom(testRoom({ tabs, scope: scope(), isPrimary: false }), <Harness data={data} />)
  })
  for (let i = 0; i < 50 && FakeWebSocket.instances.length === 0; i++) await act(async () => {})
  const sock = FakeWebSocket.last()
  await act(async () => sock.open())
  await act(async () =>
    sock.serverText({
      t: 'hello', desktop: 'console', display: '1', displays: [DISPLAY, SIDE], codec: 'jpeg', window: 3, fps: 15, path: 'lan',
      lane: 'main', encoder: 'hardware', mode: 'view', controller: null, os: 'macos', localWins: true, sameMachine: false, ...hello,
    }),
  )
  await flush()
  return sock
}

function rect(r: { left: number; top: number; width: number; height: number }): void {
  vi.spyOn(HTMLCanvasElement.prototype, 'getBoundingClientRect').mockReturnValue({
    ...r, x: r.left, y: r.top, right: r.left + r.width, bottom: r.top + r.height, toJSON: () => ({}),
  } as DOMRect)
}

const sent = (s: FakeWebSocket): Array<{ t: string; [k: string]: unknown }> => s.sentJson().filter((m) => m.t !== 'ack')
const button = (sel: string): HTMLButtonElement => {
  const b = document.querySelector<HTMLButtonElement>(sel)
  if (!b) throw new Error(`no ${sel}; pane: ${document.body.innerHTML.slice(0, 600)}`)
  return b
}

beforeEach(() => {
  daemon.status = status()
  daemon.posts = []
  daemon.tickets = 0
  renders = 0
  FakeWebSocket.reset()
  resetGridDialQueueForTests()
  vi.stubGlobal('WebSocket', FakeWebSocket)
  setDesktopItemState.mockReset()
})
afterEach(() => {
  cleanup()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  resetVideoSupportForTests()
})

describe('DesktopPane stream', () => {
  it('dials with a fresh ticket and no token; a viewer with no video decoder offers jpeg only', async () => {
    const sock = await openPane()
    expect(daemon.posts.filter((p) => p.route === 'desktop/ticket')).toHaveLength(1)
    const url = new URL(sock.url)
    expect(url.pathname).toBe('/cli/desktop/stream')
    expect(url.searchParams.get('ticket')).toBe('k2dt_test1')
    expect(url.searchParams.has('token')).toBe(false)
    expect(sock.url).not.toContain('tok-SECRET')
    expect(url.searchParams.get('codecs')).toBe('jpeg')
    expect(url.searchParams.get('win')).toBe('3')
  })

  it('a picture that stops ends on the daemon\'s why, with a way to try again', async () => {
    const sock = await openPane()
    await act(async () => sock.serverText({ t: 'state', state: 'unavailable', message: "The picture stopped: VideoToolbox wouldn't open a session. Try again." }))
    await act(async () => sock.serverClose(4410, 'picture_failed'))
    await flush()
    const card = document.querySelector('[data-desktop-stream-closed="4410"]')
    expect(card?.textContent).toContain("The picture stopped: VideoToolbox wouldn't open a session. Try again.")
    expect(button('[data-desktop-reconnect]')).toBeTruthy()
  })

  it('take control, drive, and hand back (held keys released first)', async () => {
    const sock = await openPane()
    const stage = document.querySelector<HTMLElement>('[data-desktop-picture]')!
    // Watching: a key sends nothing.
    fireEvent.keyDown(stage, { code: 'KeyA', key: 'a' })
    expect(sent(sock).filter((m) => m.t === 'key')).toEqual([])
    expect(button('[data-desktop-take-control]').dataset.desktopTakeControl).toBe('take')
    fireEvent.click(button('[data-desktop-take-control]'))
    expect(sent(sock).at(-1)).toEqual({ t: 'take-control' })
    await act(async () => sock.serverText({ t: 'control', controller: 'rosson', you: true, localHold: false }))
    expect(button('[data-desktop-take-control]').dataset.desktopTakeControl).toBe('hand-back')
    fireEvent.keyDown(stage, { code: 'KeyA', key: 'a' })
    expect(sent(sock).at(-1)).toEqual({ t: 'key', code: 'KeyA', down: true })
    fireEvent.click(button('[data-desktop-take-control]'))
    expect(sent(sock).slice(-2)).toEqual([{ t: 'key', code: 'KeyA', down: false }, { t: 'hand-back' }])
    await act(async () => sock.serverText({ t: 'control', controller: null, you: false, localHold: false }))
    expect(button('[data-desktop-take-control]').dataset.desktopTakeControl).toBe('take')
  })

  it('a hidden tab pauses with fps 0 and keeps its socket; shown, its rate and a keyframe', async () => {
    const sock = await openPane()
    await act(async () => setShown(false))
    await flush()
    expect(sent(sock).at(-1)).toEqual({ t: 'fps', fps: 0 })
    // The daemon answers a pause with its floor; that isn't the rate to keep.
    await act(async () => sock.serverText({ t: 'fps', fps: 2 }))
    await act(async () => setShown(true))
    await flush()
    expect(sent(sock).slice(-2)).toEqual([{ t: 'fps', fps: 15 }, { t: 'keyframe' }])
    expect(FakeWebSocket.instances).toHaveLength(1)
  })

  it('a pointer lands on the display point at Fit', async () => {
    rect({ left: 0, top: 0, width: 800, height: 600 })
    const sock = await openPane()
    await act(async () => sock.serverText({ t: 'control', controller: 'rosson', you: true, localHold: false }))
    const stage = document.querySelector<HTMLElement>('[data-desktop-picture]')!
    expect(stage.dataset.desktopPicture).toBe('fit')
    // 1600×900 in 800×600: scale 0.5, letterboxed 75 px top and bottom.
    fireEvent.pointerMove(stage, { clientX: 400, clientY: 375 })
    expect(sent(sock).at(-1)).toEqual({ t: 'pointer', x: 800, y: 600 })
    fireEvent.pointerMove(stage, { clientX: 400, clientY: 20 })
    expect(sent(sock).at(-1), 'the letterbox band sends nothing').toEqual({ t: 'pointer', x: 800, y: 600 })
    fireEvent.pointerDown(stage, { clientX: 0, clientY: 75, button: 0 })
    expect(sent(sock).slice(-2)).toEqual([{ t: 'pointer', x: 0, y: 0 }, { t: 'button', button: 'left', down: true }])
  })

  it('a pointer lands on the display point at 1:1 (scrolled)', async () => {
    vi.stubGlobal('devicePixelRatio', 2)
    // 1:1 on a 2× viewer: 3200 px over 1600 pt is one CSS px per point; the
    // frame is scrolled 100 px left.
    rect({ left: -100, top: 0, width: 1600, height: 900 })
    const sock = await openPane({ desktopId: 'console', fit: '1:1' })
    await act(async () => sock.serverText({ t: 'control', controller: 'rosson', you: true, localHold: false }))
    const stage = document.querySelector<HTMLElement>('[data-desktop-picture]')!
    expect(stage.dataset.desktopPicture).toBe('1:1')
    const frame = document.querySelector<HTMLElement>('[data-desktop-frame]')!
    expect(frame.style.width).toBe('1600px')
    fireEvent.pointerMove(stage, { clientX: 50, clientY: 20 })
    expect(sent(sock).at(-1)).toEqual({ t: 'pointer', x: 150, y: 20 })
  })

  it('only the controller switches the display', async () => {
    const sock = await openPane()
    const picker = document.querySelector<HTMLSelectElement>('[data-desktop-display-picker]')!
    expect(picker.disabled).toBe(true)
    await act(async () => sock.serverText({ t: 'control', controller: 'rosson', you: true, localHold: false }))
    expect(picker.disabled).toBe(false)
    fireEvent.change(picker, { target: { value: '2' } })
    expect(sent(sock).at(-1)).toEqual({ t: 'display', id: '2' })
  })

  it('a controller keeps its own pointer until deskd sends the cursor out of band', async () => {
    const sock = await openPane({ desktopId: 'console' }, { mode: 'control', controller: 'me' })
    await act(async () => sock.serverText({ t: 'control', controller: 'me', you: true, localHold: false }))
    await flush()
    const canvas = document.querySelector<HTMLCanvasElement>('[data-desktop-canvas]')!
    expect(canvas.style.cursor).toBe('default')
    await act(async () => sock.serverText({ t: 'cursor', id: 1, hotX: 0, hotY: 0, png: 'iVBORw0KGgo=' }))
    await flush()
    expect(canvas.style.cursor).toBe('none')
  })

  it('the cursor overlay moves with no React render', async () => {
    rect({ left: 0, top: 0, width: 800, height: 450 })
    const sock = await openPane()
    await act(async () => sock.serverText({ t: 'cursor', id: 7, hotX: 2, hotY: 3, png: 'iVBORw0KGgo=' }))
    await flush()
    const img = document.querySelector<HTMLImageElement>('img[aria-hidden="true"]')!
    const before = renders
    for (let i = 0; i < 20; i++) {
      await act(async () => sock.serverText({ t: 'cursor-pos', x: 100 + i, y: 50, visible: true }))
    }
    expect(renders).toBe(before)
    expect(img.getAttribute('src')).toBe('data:image/png;base64,iVBORw0KGgo=')
    expect(img.dataset.desktopCursor).toBe('119,50')
    // 1600 pt in 800 px: half scale; the cursor image is 2× pixels.
    expect(img.style.transform).toBe('translate3d(59.5px, 25px, 0) scale(0.25) translate(-2px, -3px)')
    await act(async () => sock.serverText({ t: 'cursor-pos', x: 1, y: 1, visible: false }))
    expect(img.style.display).toBe('none')
  })
})
