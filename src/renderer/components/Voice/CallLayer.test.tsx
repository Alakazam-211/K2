// @vitest-environment jsdom
//
// The call's controls and the floating video tile (Rosson 2026-11-06, after
// K2 Companion's phone card): a voice-only call has Mute, Deafen, Camera and
// Hang up in the top bar and no tile; with video on (a camera or a screen
// share) the tile floats with its own control strip under it, and the top
// bar's controls give way; stopping the video takes the tile away. Clicking
// the tile opens the call view, whose Hide brings the tile back. The tile
// drags, snaps to a corner on the release velocity (on the spring, or a short
// ease with reduced motion), can be caught mid-flight, and its corner is
// saved per window. Real voice and video controllers over the fake engines.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { useStore } from 'zustand'
import type { HostEntry } from '@/lib/host-pool'
import type { ServerScope } from '@/kessel/server-scope'
import type { VoiceRoomsList } from '@/lib/voice/voice-wire'
import type { FakeVoiceEngine } from '@/lib/voice/fake-voice-engine'
import type { FakeVideoEngine, FakeVideoOptions } from '@/lib/voice/fake-video-engine'

const h = vi.hoisted(() => ({
  entries: {} as Record<string, unknown>,
  lists: {} as Record<string, unknown>,
  videoPreview: true,
  settingsOpen: false,
  voiceEngine: null as FakeVoiceEngine | null,
  videoEngine: null as FakeVideoEngine | null,
  /** The next video engine's options (caps, the camera permission). */
  videoOpts: {} as FakeVideoOptions,
}))

vi.mock('@/stores/settings', () => {
  const state = () => ({
    voiceRoomsPreview: true,
    videoRoomsPreview: h.videoPreview,
    settingsOpen: h.settingsOpen,
    setSection: () => {},
  })
  const useSettingsStore = <T,>(sel: (s: ReturnType<typeof state>) => T): T => sel(state())
  useSettingsStore.getState = state
  return { useSettingsStore }
})

// The call's window's title bar: the OS's window buttons need the app.
vi.mock('@/components/TopBar/DesktopChromeLeft', () => ({ default: () => null }))
vi.mock('@/components/TopBar/DesktopChromeRight', () => ({ default: () => null }))

vi.mock('@/components/Home/room/RoomFailure', () => ({
  roomFailureActions: () => ({ signIn: () => {}, retry: () => {}, openServer: () => {} }),
}))

vi.mock('@/stores/voice', async () => {
  const core = await import('@/stores/voice-core')
  const { FakeVoiceEngine } = await import('@/lib/voice/fake-voice-engine')
  const voice = core.createVoiceController({
    keys: () => ['sewlab.k2.dev'],
    windowKey: () => 'sewlab.k2.dev',
    labelOf: (k: string) => k,
    entries: () => h.entries as Record<string, HostEntry>,
    check: async () => undefined,
    onPoolChange: () => () => {},
    onAuth: () => () => {},
    onWindowRoomsChanged: () => () => {},
    onRoomsChanged: () => () => {},
    selfNames: () => [],
    scopeFor: (k: string) => ({ hostKey: k, id: k, label: k }) as unknown as ServerScope,
    api: {
      rooms: async (s: ServerScope) => {
        const l = h.lists[s.hostKey]
        if (!l) throw new Error('no list')
        return l as VoiceRoomsList
      },
      join: async (_s: ServerScope, roomId: string) => ({
        callId: `call-${roomId}`,
        roomId,
        roomName: 'Standup',
        signalUrl: 'wss://voice.sewlab.k2.dev',
        mediaToken: 't',
        e2ee: null,
        relayOnly: false,
      }),
      leave: async () => {},
      selfState: async () => {},
    },
    makeEngine: () => {
      h.voiceEngine = new FakeVoiceEngine()
      return h.voiceEngine
    },
    toast: () => {},
    loadReported: () => new Set(),
    saveReported: () => {},
    loadPrefs: () => ({ inputDeviceId: null, outputDeviceId: null, joinMuted: false }),
    savePrefs: () => {},
    now: () => Date.now(),
  })
  return {
    voice,
    retainVoice: () => () => {},
    useVoice: <T,>(sel: (s: ReturnType<typeof voice.store.getState>) => T): T => useStore(voice.store, sel),
  }
})

vi.mock('@/stores/video', async () => {
  const core = await import('@/stores/video-core')
  const { FakeVideoEngine } = await import('@/lib/voice/fake-video-engine')
  const { fakeWebCodecs, RecordingSurface } = await import('@/lib/voice/fake-webcodecs')
  const { voice } = await import('@/stores/voice')
  const video = core.createVideoController({
    loadEngine: async () => {
      h.videoEngine = new FakeVideoEngine(h.videoOpts)
      return h.videoEngine
    },
    webCodecs: fakeWebCodecs,
    watchBox: (_el, onBox) => {
      onBox({ width: 160, height: 90, visible: true })
      return () => {}
    },
    devicePixelRatio: () => 1,
    surfaceFor: () => new RecordingSurface(),
    loadPrefs: () => ({ cameraId: null }),
    savePrefs: () => {},
    toast: () => {},
    now: () => Date.now(),
  })
  const sync = (): void => video.setActive({ enabled: h.videoPreview, inCall: voice.store.getState().call !== null })
  return {
    video,
    retainVideo: () => {
      const off = voice.store.subscribe(sync)
      sync()
      return () => {
        off()
        video.stop()
      }
    },
  }
})

import VoiceButton from './VoiceButton'
import CallLayer from './CallLayer'
import CallWindow from './CallWindow'
import { __resetCallExpandForTests, useCallExpand } from '@/stores/call-expand'
import { __resetCallShareForTests, useCallShare } from '@/stores/call-share'
import { __resetCallPopoutForTests, setCallPopoutBridge, useCallPopout, type CallPopoutBridge } from '@/stores/call-popout'
import { CALL_EXPAND_MARGIN, CALL_HAND_BACK_KEY, CALL_POPPED_FROM_KEY, CALL_SHARE_PICK_KEY, CALL_TILE_WIDTH_KEY, DOUBLE_CLICK_MS, __resetTileWidthForTests } from '@/lib/voice/call-view'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { CORNER_KEY_PREFIX, __resetCornersForTests } from './useCornerDrag'
import { voice } from '@/stores/voice'
import { video } from '@/stores/video'
import { useCallScreen } from '@/stores/call-screen'
import { fakeTrack } from '@/lib/voice/fake-video-engine'
import { overlayClaims, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'
import { usePanelsStore } from '@/stores/panels'
import { applyAppZoomForTest, resetAppZoomForTest } from '@/test-utils/app-zoom'

const list: VoiceRoomsList = {
  hosting: { state: 'ok', line: 'Voice rooms are on.', detail: null },
  canManage: false,
  feedLive: true,
  rev: 1,
  rooms: [{ id: 'room-1', name: 'Standup', limit: 25, closing: false, closesAt: null, unavailable: null, occupants: [] }],
}

function entry(hostKey: string): HostEntry {
  return {
    hostKey,
    saved: true,
    hostId: hostKey,
    reach: 'live',
    boot: { phase: 'ready', ready: true, version: '0.46.9', protocol: 1, instanceId: 'i', features: ['voice-rooms-v1'], view: 'full', at: 1 },
    auth: 'ok',
    authNote: null,
    role: 'member',
    presence: null,
    offlineStreak: 0,
    checkedAt: 1,
  }
}

/** In a call on the native engine (the fake seat engine, drawn as native),
 *  with the top bar's headset and the window's call layer mounted. */
async function inCall(): Promise<void> {
  h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
  h.lists = { 'sewlab.k2.dev': list }
  act(() => voice.recompute())
  render(
    <>
      <VoiceButton />
      <CallLayer />
    </>,
  )
  fireEvent.click(screen.getByTestId('voice-button'))
  fireEvent.click(await screen.findByTestId('voice-join'))
  await waitFor(() => expect(voice.store.getState().call?.phase).toBe('connected'))
  act(() => {
    voice.setPopoverOpen(false)
    voice.store.setState({ engineKind: 'native' })
  })
  if (h.videoPreview) await waitFor(() => expect(video.store.getState().availability).toBe('ready'))
}

/** Camera on, through the top bar's button: the tile comes up. */
async function cameraOn(): Promise<HTMLElement> {
  fireEvent.click(await screen.findByTestId('call-topbar-camera'))
  return screen.findByTestId('call-tile')
}

function setReducedMotion(reduce: boolean): void {
  vi.stubGlobal(
    'matchMedia',
    (q: string) => ({ matches: reduce && q.includes('reduce'), media: q, addEventListener() {}, removeEventListener() {} }) as unknown as MediaQueryList,
  )
}

// Animation frames and the clock, stepped by hand.
let frames = new Map<number, (t: number) => void>()
let nextFrame = 1
let clock = 0
function step(ms: number): void {
  clock += ms
  const run = [...frames.values()]
  frames = new Map()
  for (const f of run) f(clock)
}

beforeEach(async () => {
  h.videoPreview = true
  h.settingsOpen = false
  h.videoOpts = {}
  localStorage.clear()
  __resetCornersForTests()
  frames = new Map()
  clock = 0
  vi.stubGlobal('requestAnimationFrame', (cb: (t: number) => void) => {
    const id = nextFrame++
    frames.set(id, cb)
    return id
  })
  vi.stubGlobal('cancelAnimationFrame', (id: number) => {
    frames.delete(id)
  })
  vi.spyOn(performance, 'now').mockImplementation(() => clock)
  setReducedMotion(false)
  await act(async () => {
    if (voice.store.getState().call) await voice.leave()
    voice.store.setState({ rooms: {}, notice: null, popoverOpen: false, engineKind: 'seat' })
  })
  useCallScreen.setState({ open: false })
  __resetCallExpandForTests()
  __resetCallShareForTests()
  __resetCallPopoutForTests()
  __resetTileWidthForTests()
})
afterEach(() => {
  cleanup()
  uninstallFullscreenApi()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  resetBrowserOverlaysForTest()
})

describe('voice only: the controls are in the top bar, and there is no tile', () => {
  it('shows Mute, Deafen, Camera and Leave call beside the headset', async () => {
    await inCall()
    const bar = await screen.findByTestId('call-topbar-controls')
    expect(bar.querySelectorAll('button')).toHaveLength(4)
    expect(screen.getByTestId('call-topbar-mute').getAttribute('aria-label')).toBe('Mute')
    expect(screen.getByTestId('call-topbar-deafen').getAttribute('aria-label')).toBe('Deafen')
    expect(screen.getByTestId('call-topbar-camera').getAttribute('aria-label')).toBe('Turn camera on')
    expect(screen.getByTestId('call-topbar-leave').getAttribute('aria-label')).toBe('Leave call')
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('with video rooms off: no Camera, and still no tile', async () => {
    h.videoPreview = false
    await inCall()
    expect(screen.getByTestId('call-topbar-controls').querySelectorAll('button')).toHaveLength(3)
    expect(screen.queryByTestId('call-topbar-camera')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('the buttons drive the engine: mute, deafen, hang up', async () => {
    await inCall()
    fireEvent.click(screen.getByTestId('call-topbar-mute'))
    expect(h.voiceEngine?.calls).toContain('muted true')
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(screen.getByTestId('call-topbar-mute').getAttribute('aria-label')).toBe('Unmute')
    fireEvent.click(screen.getByTestId('call-topbar-deafen'))
    expect(h.voiceEngine?.calls).toContain('deafened true')
    fireEvent.click(screen.getByTestId('call-topbar-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(h.voiceEngine?.calls).toContain('disconnect')
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
  })

  it('a seat-only call (the web client: no audio here) shows no controls and no tile', async () => {
    await inCall()
    act(() => voice.store.setState({ engineKind: 'seat' }))
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })
})

describe('video on: the tile with its strip, and the top bar gives way', () => {
  it('your camera brings up the tile (self-view) and the strip; stopping it takes the tile away', async () => {
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.picture).toBe('local:camera')
    expect(screen.getByTestId('call-tile-strip')).toBeTruthy()
    // Mute, Deafen, Camera, Share screen, Leave call.
    expect(screen.getByTestId('call-tile-controls').querySelectorAll('button')).toHaveLength(5)
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    // The call's time, on the picture at its bottom right (no band under it).
    expect(screen.getByTestId('call-tile-duration').textContent).toMatch(/^\d+:\d\d$/)
    expect(screen.getByTestId('call-tile-duration').className).toContain('tabular-nums')
    // Camera off from the strip: the tile goes, the controls are back up top.
    fireEvent.click(screen.getByTestId('call-tile-camera'))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
  })

  it("someone else's screen share shows the same way (the fake engine's share track)", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7', { source: 'screen', track: 'TR_S7' })]))
    const tile = await screen.findByTestId('call-tile')
    expect(tile.dataset.picture).toBe('TR_S7')
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['TR_S7']))
    act(() => h.videoEngine?.setTracks([]))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
  })

  it("follows the active speaker's video", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7'), fakeTrack('8')]))
    const tile = await screen.findByTestId('call-tile')
    expect(tile.dataset.picture).toBe('TR_7')
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({
        call: {
          ...c,
          participants: [
            { user: 'ada', memberId: '7', speaking: false, muted: false },
            { user: 'grace', memberId: '8', speaking: true, muted: false },
          ],
        },
      })
    })
    await waitFor(() => expect(screen.getByTestId('call-tile').dataset.picture).toBe('TR_8'))
  })

  it("the strip's buttons drive the engine", async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(screen.getByTestId('call-tile-mute'))
    expect(h.voiceEngine?.calls).toContain('muted true')
    fireEvent.click(screen.getByTestId('call-tile-deafen'))
    expect(h.voiceEngine?.calls).toContain('deafened true')
    fireEvent.click(screen.getByTestId('call-tile-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('clicking the tile opens the call view over the page; Hide brings the tile back', async () => {
    await inCall()
    await cameraOn()
    const picture = screen.getByTestId('call-tile-picture')
    fireEvent.pointerDown(picture, { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
    fireEvent.pointerUp(picture, { clientX: 101, clientY: 100, pointerId: 1, button: 0 })
    await waitFor(() => expect(screen.getByTestId('video-stage')).toBeTruthy())
    expect(screen.queryByTestId('call-tile')).toBeNull()
    // The call view has its own controls; the top bar's are back too.
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    fireEvent.click(screen.getByTestId('video-stage-hide'))
    await waitFor(() => expect(screen.getByTestId('call-tile')).toBeTruthy())
    expect(screen.queryByTestId('video-stage')).toBeNull()
  })

  it('a click on a strip button never opens the call view', async () => {
    await inCall()
    await cameraOn()
    const mute = screen.getByTestId('call-tile-mute')
    fireEvent.pointerDown(mute, { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
    fireEvent.pointerUp(mute, { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
    fireEvent.click(mute)
    expect(useCallScreen.getState().open).toBe(false)
    expect(voice.store.getState().call?.muted).toBe(true)
  })

  it('Settings open: the tile steps back and the top bar has the controls', async () => {
    await inCall()
    await cameraOn()
    h.settingsOpen = true
    // The settings mock isn't a store: re-render through the call's.
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({ call: { ...c } })
    })
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
  })
})

describe('motion', () => {
  // jsdom has no layout: the window is 1024 x 768 and the tile 240 wide.
  /** Press on the picture, drag through `path` (ms apart), and let go. */
  function drag(el: HTMLElement, from: [number, number], path: Array<[number, number]>, msApart: number): void {
    fireEvent.pointerDown(el, { clientX: from[0], clientY: from[1], pointerId: 1, button: 0 })
    for (const [x, y] of path) {
      step(msApart)
      fireEvent.pointerMove(el, { clientX: x, clientY: y, pointerId: 1 })
    }
    step(msApart)
    const [lx, ly] = path[path.length - 1]
    fireEvent.pointerUp(el, { clientX: lx, clientY: ly, pointerId: 1 })
  }

  it('starts in the bottom right, lifts while held, and a slow drag lands in the nearest corner, saved for this window', async () => {
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.corner).toBe('bottom-right')
    const picture = screen.getByTestId('call-tile-picture')
    fireEvent.pointerDown(picture, { clientX: 900, clientY: 700, pointerId: 1, button: 0 })
    step(16)
    fireEvent.pointerMove(picture, { clientX: 700, clientY: 500, pointerId: 1 })
    expect(tile.dataset.held).toBe('true')
    step(200)
    // Lifted: a little bigger while held.
    expect(tile.style.transform).toMatch(/scale\(1\.0[1-4]/)
    for (const [x, y] of [[400, 300], [150, 120], [140, 110]] as const) {
      step(200)
      fireEvent.pointerMove(picture, { clientX: x, clientY: y, pointerId: 1 })
    }
    step(200)
    fireEvent.pointerUp(picture, { clientX: 140, clientY: 110, pointerId: 1 })
    expect(tile.dataset.corner).toBe('top-left')
    expect(tile.dataset.held).toBe('false')
    expect(tile.dataset.motion).toBe('spring')
    expect(localStorage.getItem(`${CORNER_KEY_PREFIX}main`)).toBe('top-left')
    // The drag's click is not a click on the tile.
    fireEvent.click(picture)
    expect(useCallScreen.getState().open).toBe(false)
    // It settles at the corner.
    for (let i = 0; i < 120; i++) step(16)
    expect(tile.style.transform).toBe('')
  })

  it('a fling goes to the far corner', async () => {
    await inCall()
    const tile = await cameraOn()
    // From the bottom-right, a quick flick left: 2000 px/s.
    drag(screen.getByTestId('call-tile-picture'), [900, 700], [[880, 700], [860, 700], [840, 700]], 10)
    expect(tile.dataset.corner).toBe('bottom-left')
  })

  it('can be caught mid-flight: it stops under the pointer', async () => {
    await inCall()
    const tile = await cameraOn()
    const picture = screen.getByTestId('call-tile-picture')
    drag(picture, [900, 700], [[700, 500], [500, 300], [480, 280]], 100)
    step(16)
    step(16)
    const mid = tile.style.transform
    expect(mid).not.toBe('')
    fireEvent.pointerDown(picture, { clientX: 480, clientY: 280, pointerId: 2, button: 0 })
    step(16)
    step(16)
    expect(tile.style.transform).toBe(mid)
    // Dragged on from there.
    fireEvent.pointerMove(picture, { clientX: 470, clientY: 300, pointerId: 2 })
    expect(tile.dataset.held).toBe('true')
  })

  it('reduced motion: no lift, and a short ease into the corner', async () => {
    setReducedMotion(true)
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.reducedMotion).toBe('true')
    const picture = screen.getByTestId('call-tile-picture')
    fireEvent.pointerDown(picture, { clientX: 900, clientY: 700, pointerId: 1, button: 0 })
    step(16)
    fireEvent.pointerMove(picture, { clientX: 500, clientY: 300, pointerId: 1 })
    step(200)
    expect(tile.style.transform).not.toContain('scale')
    step(200)
    fireEvent.pointerUp(picture, { clientX: 150, clientY: 120, pointerId: 1 })
    expect(tile.dataset.motion).toBe('ease')
    step(100)
    expect(tile.style.transform).not.toBe('')
    step(130)
    expect(tile.style.transform).toBe('')
  })

  it('rests beside an open drawer, moves out of its way when it opens, and back when it closes', async () => {
    // A right drawer (as AnimatedDrawer marks it), 300 wide, under the top bar.
    const drawer = document.createElement('div')
    drawer.dataset.drawerSide = 'right'
    let width = 300
    drawer.getBoundingClientRect = () => ({ left: 1024 - width, top: 38, width, height: 730, right: 1024, bottom: 768, x: 1024 - width, y: 38, toJSON: () => ({}) }) as DOMRect
    document.body.appendChild(drawer)
    try {
      act(() => usePanelsStore.setState({ rightPanelOpen: true, rightPanelWidth: 300 }))
      await inCall()
      const tile = await cameraOn()
      expect(tile.dataset.corner).toBe('bottom-right')
      expect(tile.style.left).toBe(`${1024 - 300 - 12 - 240}px`)
      // The drawer closes: once its slide is over, the tile goes back to the window's corner.
      width = 0
      act(() => usePanelsStore.setState({ rightPanelOpen: false }))
      await waitFor(() => expect(tile.style.left).toBe(`${1024 - 12 - 240}px`), { timeout: 2000 })
      // It opens again: the tile moves out of its way.
      width = 300
      for (let i = 0; i < 120; i++) step(16)
      act(() => usePanelsStore.setState({ rightPanelOpen: true }))
      await waitFor(() => expect(tile.style.left).toBe(`${1024 - 300 - 12 - 240}px`), { timeout: 2000 })
    } finally {
      drawer.remove()
      act(() => usePanelsStore.setState({ rightPanelOpen: false }))
    }
  })

  // The app zoom (CSS zoom on <html>): rects, the window size and the
  // pointer are window px; the tile's left/top and translate are CSS px,
  // which the zoom scales. At 150 % the tile rested past the window's
  // right and bottom edges and ran ahead of the pointer.
  describe('under the app zoom (150 %)', () => {
    const Z = 1.5
    afterEach(() => resetAppZoomForTest())

    it("rests in the window's bottom-right corner, inside the window", async () => {
      applyAppZoomForTest(Z)
      await inCall()
      const tile = await cameraOn()
      expect(tile.dataset.corner).toBe('bottom-right')
      const left = Number.parseFloat(tile.style.left)
      expect(Math.abs(left - (1024 / Z - 12 - 240))).toBeLessThan(0.5)
      // On screen: the right edge 12 CSS px in from the window's.
      expect(Math.abs((left + 240) * Z - (1024 - 12 * Z))).toBeLessThan(1)
    })

    it('follows the pointer while held: a 150 px drag on screen moves it 150 px on screen', async () => {
      applyAppZoomForTest(Z)
      await inCall()
      await cameraOn()
      const tile = screen.getByTestId('call-tile')
      const picture = screen.getByTestId('call-tile-picture')
      fireEvent.pointerDown(picture, { clientX: 900, clientY: 700, pointerId: 1, button: 0 })
      step(16)
      fireEvent.pointerMove(picture, { clientX: 750, clientY: 550, pointerId: 1 })
      const m = /translate\((-?[\d.]+)px, (-?[\d.]+)px\)/.exec(tile.style.transform)
      if (!m) throw new Error(`no translate in "${tile.style.transform}"`)
      // The translate is CSS px: 100 of them draw 150 window px.
      expect(Number(m[1]) * Z).toBeCloseTo(-150, 0)
      expect(Number(m[2]) * Z).toBeCloseTo(-150, 0)
      fireEvent.pointerUp(picture, { clientX: 750, clientY: 550, pointerId: 1 })
    })
  })

  it('the corner comes back for this window next time', async () => {
    localStorage.setItem(`${CORNER_KEY_PREFIX}main`, 'top-right')
    localStorage.setItem(`${CORNER_KEY_PREFIX}focus-1`, 'bottom-left')
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.corner).toBe('top-right')
  })
})

describe("camera and screen from the call's controls (Sew's plugin video)", () => {
  it("the camera's first press asks the OS (the plugin never does), and a refusal says why with the way to the settings", async () => {
    h.videoOpts = { cameraPermission: 'prompt', cameraPrompt: 'granted' }
    await inCall()
    await cameraOn()
    expect(h.videoEngine?.calls.slice(0, 3)).toEqual(['camera_permission', 'request_camera_permission', 'camera {"on":true}'])
    // Then the self-view's sink.
    await waitFor(() => expect(video.sinks().map((x) => x.track)).toEqual(['local:camera']))
    fireEvent.click(screen.getByTestId('call-tile-camera'))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    // Denied now (System Settings): the press asks nothing and says so.
    if (!h.videoEngine) throw new Error('no video engine')
    h.videoEngine.permission = 'denied'
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    await waitFor(() => expect(video.store.getState().notice).toMatchObject({ settings: 'camera' }))
    expect(h.videoEngine.calls.slice(-1)).toEqual(['camera_permission'])
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it("Share screen opens the OS picker where there is one, and the tile shows the share's self-view", async () => {
    h.videoOpts = { caps: { screen: { supported: true, systemPicker: true, list: true, windows: true } } }
    await inCall()
    // The top bar has it too, where the OS has a picker.
    fireEvent.click(await screen.findByTestId('call-topbar-screen'))
    const tile = await screen.findByTestId('call-tile')
    expect(h.videoEngine?.calls).toContain('screen {"on":true}')
    expect(tile.dataset.picture).toBe('local:screen')
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['local:screen']))
    fireEvent.click(screen.getByTestId('call-tile-screen'))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(h.videoEngine?.calls).toContain('screen {"on":false}')
  })

  it("where the OS only lists, the strip's Share screen shows the list under it, in K2's own menu", async () => {
    await inCall()
    await cameraOn()
    // No OS picker: the top bar has no Share; the tile's strip does.
    fireEvent.click(screen.getByTestId('call-tile-screen'))
    await screen.findByTestId('video-screen-picker')
    expect(h.videoEngine?.calls).toContain('screen_sources')
    fireEvent.click(screen.getByTestId('video-screen-source'))
    const pick = (await screen.findAllByText(/Quarterly plan/)).map((el) => el.closest('button, [role="option"]')).find((b) => b !== null)
    if (!pick) throw new Error('the window is not in the list')
    fireEvent.click(pick)
    await waitFor(() => expect(h.videoEngine?.calls).toContain('screen {"on":true,"source":"window:42"}'))
    await waitFor(() => expect(screen.queryByTestId('video-screen-picker')).toBeNull())
  })
})

// ── Expand, labels on the picture, resize, pop out (Rosson 2026-11-10) ──

/** The element Fullscreen API, as WKWebView has it, recording every request:
 *  Expand must never ask for OS full screen. */
const fsCalls: string[] = []
function installFullscreenApi(): void {
  fsCalls.length = 0
  Object.defineProperty(document, 'fullscreenEnabled', { configurable: true, get: () => true })
  Object.defineProperty(document, 'fullscreenElement', { configurable: true, get: () => null })
  Object.defineProperty(document, 'exitFullscreen', { configurable: true, value: () => fsCalls.push('exit') })
  Object.defineProperty(HTMLElement.prototype, 'requestFullscreen', {
    configurable: true,
    value(this: HTMLElement) {
      fsCalls.push(`request:${this.dataset.testid ?? this.tagName}`)
      return Promise.resolve()
    },
  })
}
function uninstallFullscreenApi(): void {
  for (const k of ['fullscreenEnabled', 'fullscreenElement', 'exitFullscreen']) delete (document as unknown as Record<string, unknown>)[k]
  delete (HTMLElement.prototype as unknown as Record<string, unknown>).requestFullscreen
}

function expandChord(): KeyboardEventInit {
  return isMacPlatform() ? { key: 'v', code: 'KeyV', ctrlKey: true, metaKey: true } : { key: 'v', code: 'KeyV', ctrlKey: true, altKey: true }
}

/** Four people (you, Ada, Grace, Linus) and Ada sharing her screen; Grace's
 *  camera is on, Linus is muted and talking. */
function fourPeopleAndAShare(more: ReturnType<typeof fakeTrack>[] = []): void {
  act(() => h.videoEngine?.setTracks([fakeTrack('7', { source: 'screen', track: 'TR_S7' }), fakeTrack('8'), ...more]))
  act(() => {
    const c = voice.store.getState().call
    if (!c) throw new Error('no call')
    voice.store.setState({
      call: {
        ...c,
        participants: [
          { user: 'You', memberId: '1', isSelf: true, speaking: false, muted: false },
          { user: 'Ada', memberId: '7', speaking: false, muted: false },
          { user: 'Grace', memberId: '8', speaking: false, muted: false },
          { user: 'Linus', memberId: '9', speaking: true, muted: true },
        ],
      },
    })
  })
}

describe('labels on the picture', () => {
  it('the name at the bottom left and the time at the bottom right, both on the picture; no strip of its own under it', async () => {
    await inCall()
    await cameraOn()
    const picture = screen.getByTestId('call-tile-picture')
    const name = within(picture).getByTestId('video-tile-label')
    expect(name.dataset.at).toBe('bottom-left')
    expect(name.className).toMatch(/\bbottom-0\b/)
    expect(name.className).toMatch(/\bleft-0\b/)
    expect(within(name).getByTestId('video-tile-name').textContent).toBe('You')
    const corner = within(picture).getByTestId('video-tile-corner')
    expect(corner.dataset.at).toBe('bottom-right')
    expect(corner.className).toMatch(/\bright-0\b/)
    expect(within(corner).getByTestId('call-tile-duration').textContent).toMatch(/^\d+:\d\d$/)
    // Both on the video itself, on the token chip (readable on any picture).
    const tile = within(picture).getByTestId('video-tile')
    for (const el of [name, corner]) {
      expect(tile.contains(el)).toBe(true)
      expect(el.className).toContain('bg-[var(--color-media-label-bg)]')
      expect(el.className).toContain('text-[var(--color-media-label-text)]')
    }
    // The picture holds the video, its buttons and the resize grips: no band under it.
    const kids = [...picture.children].map((c) => (c as HTMLElement).dataset.testid ?? (c as HTMLElement).dataset.resize)
    expect(kids).toEqual(['video-tile', 'call-tile-view-buttons', 'call-tile-resize-x', 'call-tile-resize-y', 'call-tile-resize-corner'])
  })

  it("every tile in the call view carries its name the same way (the self-view's too)", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7')]))
    await screen.findByTestId('call-tile')
    fireEvent.click(screen.getByTestId('call-tile-camera'))
    act(() => useCallScreen.getState().ask())
    await waitFor(() => expect(within(screen.getByTestId('video-grid')).getAllByTestId('video-tile-label')).toHaveLength(2))
    for (const l of within(screen.getByTestId('video-grid')).getAllByTestId('video-tile-label')) {
      expect(l.className).toContain('bg-[var(--color-media-label-bg)]')
    }
  })
})

describe('expand: the call over K2, inside the app', () => {
  it("the tile's Expand button opens the call over K2 with a margin, never OS full screen; Shrink puts the tile back", async () => {
    installFullscreenApi()
    await inCall()
    await cameraOn()
    const expand = screen.getByTestId('call-tile-expand')
    expect(expand.getAttribute('aria-label')).toMatch(/^Expand \(.+\)$/)
    expect(expand.getAttribute('title')).toBe(expand.getAttribute('aria-label'))
    expand.focus()
    fireEvent.click(expand)
    expect(useCallExpand.getState().expanded).toBe(true)
    const view = screen.getByTestId('call-expanded')
    expect(view.getAttribute('role')).toBe('dialog')
    expect(view.getAttribute('aria-modal')).toBe('true')
    // A margin all round, inside the window; over the scrim.
    expect(view.style.width).toBe(`calc(var(--k2-vw) - ${CALL_EXPAND_MARGIN * 2}px)`)
    expect(view.style.height).toBe(`calc(var(--k2-vh) - ${CALL_EXPAND_MARGIN * 2}px)`)
    expect(screen.getByTestId('call-expanded-scrim').dataset.scrim).toBe('')
    expect(fsCalls).toEqual([])
    // The tile gives way; the video is drawn once, in the view.
    expect(screen.queryByTestId('call-tile')).toBeNull()
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['local:camera']))
    // Focus moves in, to Shrink.
    const shrink = within(view).getByTestId('call-expanded-shrink')
    expect(document.activeElement).toBe(shrink)
    expect(shrink.textContent).toBe('Shrink')
    fireEvent.click(shrink)
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(screen.queryByTestId('call-expanded')).toBeNull()
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    // Focus is back on the tile's Expand button.
    expect(document.activeElement).toBe(screen.getByTestId('call-tile-expand'))
  })

  it('a click on the scrim round it shrinks it; a click inside does not', async () => {
    await inCall()
    await cameraOn()
    screen.getByTestId('call-tile-expand').focus()
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    fireEvent.click(screen.getByTestId('call-expanded-title'))
    expect(useCallExpand.getState().expanded).toBe(true)
    fireEvent.click(screen.getByTestId('call-expanded-scrim'))
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByTestId('call-tile-expand'))
  })

  it('Esc shrinks it, and focus goes back to the Expand button (a click there never focused it, as in WebKit)', async () => {
    await inCall()
    await cameraOn()
    expect(document.activeElement).toBe(document.body)
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByTestId('call-tile-expand'))
  })

  it('Tab stays inside the view', async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    const view = screen.getByTestId('call-expanded')
    const buttons = [...view.querySelectorAll('button')]
    buttons[buttons.length - 1].focus()
    fireEvent.keyDown(buttons[buttons.length - 1], { key: 'Tab' })
    expect(document.activeElement).toBe(buttons[0])
    fireEvent.keyDown(buttons[0], { key: 'Tab', shiftKey: true })
    expect(document.activeElement).toBe(buttons[buttons.length - 1])
  })

  it("the call's controls sit along its bottom, and work", async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    const bar = screen.getByTestId('call-expanded-controls')
    expect(bar.tagName).toBe('FOOTER')
    expect(screen.getByTestId('call-expanded').lastElementChild).toBe(bar)
    // Mute, Deafen, Camera, Share screen, Leave call.
    expect(within(bar).getByTestId('call-overlay-controls').querySelectorAll('button')).toHaveLength(5)
    fireEvent.click(within(bar).getByTestId('call-overlay-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(screen.getByTestId('call-expanded-duration').textContent).toMatch(/^\d+:\d\d$/)
  })

  it('a double-click on the tile expands it, and does not open the call view', async () => {
    await inCall()
    await cameraOn()
    const picture = screen.getByTestId('call-tile-picture')
    for (let i = 0; i < 2; i++) {
      fireEvent.pointerDown(picture, { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
      fireEvent.pointerUp(picture, { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
    }
    fireEvent.doubleClick(picture)
    expect(useCallExpand.getState().expanded).toBe(true)
    await new Promise((r) => setTimeout(r, DOUBLE_CLICK_MS + 50))
    expect(useCallScreen.getState().open).toBe(false)
  })

  it('the chord toggles it (⌃⌘V on macOS, Ctrl+Alt+V elsewhere)', async () => {
    installFullscreenApi()
    await inCall()
    await cameraOn()
    fireEvent.keyDown(window, expandChord())
    expect(useCallExpand.getState().expanded).toBe(true)
    fireEvent.keyDown(window, expandChord())
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(fsCalls).toEqual([])
  })

  it('the chord does nothing without video', async () => {
    await inCall()
    fireEvent.keyDown(window, expandChord())
    expect(useCallExpand.getState().expanded).toBe(false)
  })

  it('Leave call in the expanded view ends the call and the view with it', async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    fireEvent.click(screen.getByTestId('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(screen.queryByTestId('call-expanded')).toBeNull()
  })

  it("the call view's header has Expand too, and shrinking goes back to the call view", async () => {
    await inCall()
    await cameraOn()
    act(() => useCallScreen.getState().ask())
    const expand = await screen.findByTestId('video-stage-expand')
    expect(expand.getAttribute('aria-label')).toMatch(/^Expand/)
    expand.focus()
    fireEvent.click(expand)
    expect(useCallExpand.getState().expanded).toBe(true)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByTestId('video-stage-expand'))
  })

  it('claims the window over a live Browser page while open', async () => {
    await inCall()
    await cameraOn()
    const before = overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    expect(overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length).toBe(before + 1)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length).toBe(before)
  })
})

describe('a screen share in the middle, the people on the left', () => {
  it('expanded: the share takes the middle, and everyone is a smaller tile in the column, with names, speaking and muted', async () => {
    await inCall()
    fourPeopleAndAShare()
    await screen.findByTestId('call-tile')
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    const layout = await screen.findByTestId('call-share-layout')
    expect(layout.dataset.main).toBe('TR_S7')
    const main = within(screen.getByTestId('call-share-main')).getByTestId('video-tile')
    expect(main.dataset.track).toBe('TR_S7')
    expect(main.dataset.large).toBe('true')
    expect(within(main).getByTestId('video-tile-name').textContent).toBe("Ada's screen")
    // The column comes first (the left), then the middle.
    expect(layout.firstElementChild).toBe(screen.getByTestId('call-share-column'))
    const people = within(screen.getByTestId('call-share-people')).getAllByTestId('video-tile')
    // Ada, Grace (camera), Linus (no camera: their picture), then you.
    expect(people.map((t) => within(t).getByTestId('video-tile-name').textContent)).toEqual(['Ada', 'Grace', 'Linus', 'You'])
    expect(people.map((t) => t.dataset.large)).toEqual(['false', 'false', 'false', 'false'])
    expect(people[1].dataset.track).toBe('TR_8')
    expect(people[1].dataset.state).toBe('live')
    expect(people[2].dataset.state).toBe('off')
    expect(people[2].dataset.speaking).toBe('true')
    expect(within(people[2]).getByTestId('video-tile-muted')).toBeTruthy()
    // One sink per track: the share and Grace's camera.
    await waitFor(() => expect(video.sinks().map((s) => s.track).sort()).toEqual(['TR_8', 'TR_S7']))
  })

  it('no share: the grid, as before', async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7'), fakeTrack('8')]))
    await screen.findByTestId('call-tile')
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    expect(within(screen.getByTestId('call-expanded')).getByTestId('video-grid')).toBeTruthy()
    expect(screen.queryByTestId('call-share-layout')).toBeNull()
  })

  it('the layout switches as a share starts and stops', async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('8')]))
    await screen.findByTestId('call-tile')
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    expect(screen.getByTestId('video-grid')).toBeTruthy()
    fourPeopleAndAShare()
    expect(await screen.findByTestId('call-share-layout')).toBeTruthy()
    expect(screen.queryByTestId('video-grid')).toBeNull()
    act(() => h.videoEngine?.setTracks([fakeTrack('8')]))
    expect(await screen.findByTestId('video-grid')).toBeTruthy()
    expect(screen.queryByTestId('call-share-layout')).toBeNull()
  })

  it('several shares: the first one in the middle, the others at the top of the column, a click away', async () => {
    await inCall()
    fourPeopleAndAShare([fakeTrack('9', { source: 'screen', track: 'TR_S9' })])
    await screen.findByTestId('call-tile')
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    expect((await screen.findByTestId('call-share-layout')).dataset.main).toBe('TR_S7')
    const pick = screen.getByTestId('call-share-pick')
    expect(pick.dataset.track).toBe('TR_S9')
    expect(pick.getAttribute('aria-label')).toBe("Show Linus's screen in the middle")
    fireEvent.click(pick)
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    expect(screen.getByTestId('call-share-pick').dataset.track).toBe('TR_S7')
    // The pick sticks while that share lasts; when it stops, the other takes the middle.
    act(() => h.videoEngine?.setTracks([fakeTrack('7', { source: 'screen', track: 'TR_S7' }), fakeTrack('8')]))
    await waitFor(() => expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S7'))
    expect(screen.queryByTestId('call-share-pick')).toBeNull()
  })

  it("the tile following someone talking shows a cue for the share it isn't showing; the cue expands the call onto it", async () => {
    await inCall()
    fourPeopleAndAShare()
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({ call: { ...c, participants: c.participants.map((p) => ({ ...p, speaking: p.memberId === '8' })) } })
    })
    // After Linus's 1.5 s hold.
    await waitFor(() => expect(screen.getByTestId('call-tile').dataset.picture).toBe('TR_8'), { timeout: 3000 })
    const cue = screen.getByTestId('call-tile-share-cue')
    expect(cue.dataset.track).toBe('TR_S7')
    expect(cue.getAttribute('aria-label')).toBe("Expand to see Ada's screen")
    fireEvent.click(cue)
    expect(useCallExpand.getState().expanded).toBe(true)
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S7')
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(document.activeElement).toBe(screen.getByTestId('call-tile-share-cue'))
    // The tile showing the share itself needs no cue.
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({ call: { ...c, participants: c.participants.map((p) => ({ ...p, speaking: p.memberId === '7' })) } })
    })
    await waitFor(() => expect(screen.getByTestId('call-tile').dataset.picture).toBe('TR_S7'), { timeout: 3000 })
    expect(screen.queryByTestId('call-tile-share-cue')).toBeNull()
  })

  it('the call view and the floating tile keep their own layout (the share on the tile, the grid in the view)', async () => {
    await inCall()
    fourPeopleAndAShare()
    const tile = await screen.findByTestId('call-tile')
    expect(tile.dataset.picture).toBe('TR_S7')
    act(() => useCallScreen.getState().ask())
    expect(within(await screen.findByTestId('video-stage')).getByTestId('video-grid')).toBeTruthy()
    expect(screen.queryByTestId('call-share-layout')).toBeNull()
  })
})

describe('resize', () => {
  // jsdom's window is 1024 × 768 and has no layout: the free area is the
  // window, less the insets.
  const drag = (handle: HTMLElement, from: [number, number], to: [number, number]): void => {
    fireEvent.pointerDown(handle, { clientX: from[0], clientY: from[1], pointerId: 3, button: 0 })
    fireEvent.pointerMove(handle, { clientX: to[0], clientY: to[1], pointerId: 3 })
    fireEvent.pointerUp(handle, { clientX: to[0], clientY: to[1], pointerId: 3 })
  }

  it('drags from the inward edges and corner, keeping 16:9, clamped, and never moves or opens the tile', async () => {
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.corner).toBe('bottom-right')
    expect(tile.dataset.width).toBe('240')
    // Anchored bottom right: the grips are on the left and top edges.
    expect(screen.getByTestId('call-tile-resize-x').className).toContain('left-0')
    expect(screen.getByTestId('call-tile-resize-y').className).toContain('top-0')
    drag(screen.getByTestId('call-tile-resize-x'), [700, 600], [600, 600])
    expect(tile.dataset.width).toBe('340')
    expect(tile.style.width).toBe('340px')
    // The top edge, through the aspect: 90 px up is 160 px wider.
    drag(screen.getByTestId('call-tile-resize-y'), [700, 500], [700, 410])
    expect(tile.dataset.width).toBe('500')
    // The corner: as far as it goes, and no further either way.
    drag(screen.getByTestId('call-tile-resize-corner'), [500, 400], [-5000, -5000])
    expect(tile.dataset.width).toBe('960')
    drag(screen.getByTestId('call-tile-resize-corner'), [100, 100], [5000, 5000])
    expect(tile.dataset.width).toBe('200')
    // None of it was a drag of the tile, or a click on it.
    expect(tile.dataset.held).toBe('false')
    expect(tile.dataset.corner).toBe('bottom-right')
    await new Promise((r) => setTimeout(r, DOUBLE_CLICK_MS + 50))
    expect(useCallScreen.getState().open).toBe(false)
  })

  it('the size is remembered for this client, and comes back next time', async () => {
    await inCall()
    let tile = await cameraOn()
    drag(screen.getByTestId('call-tile-resize-x'), [700, 600], [580, 600])
    expect(tile.dataset.width).toBe('360')
    expect(localStorage.getItem(CALL_TILE_WIDTH_KEY)).toBe('360')
    // Camera off and on again: the tile comes back at that size.
    fireEvent.click(screen.getByTestId('call-tile-camera'))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    __resetTileWidthForTests()
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    tile = await screen.findByTestId('call-tile')
    expect(tile.dataset.width).toBe('360')
  })

  it('a tile in the top left grows from its right and bottom edges', async () => {
    localStorage.setItem(`${CORNER_KEY_PREFIX}main`, 'top-left')
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.corner).toBe('top-left')
    expect(screen.getByTestId('call-tile-resize-x').className).toContain('right-0')
    expect(screen.getByTestId('call-tile-resize-y').className).toContain('bottom-0')
    drag(screen.getByTestId('call-tile-resize-x'), [300, 300], [400, 300])
    expect(tile.dataset.width).toBe('340')
  })
})

describe('pop out: the call in its own window', () => {
  function fakePopout(): { bridge: CallPopoutBridge; opened: unknown[]; emit: (open: boolean) => void } {
    let handler: ((open: boolean) => void) | null = null
    const opened: unknown[] = []
    return {
      opened,
      bridge: {
        open: async (frame) => {
          opened.push(frame)
        },
        isOpen: async () => false,
        watch: async (on) => {
          handler = on
          return () => {
            handler = null
          }
        },
      },
      emit: (open) =>
        act(() => {
          if (!handler) throw new Error('nobody is watching the call window')
          handler(open)
        }),
    }
  }

  it('Pop out hides the tile here without touching the call; closing the window brings it back', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    const before = h.voiceEngine?.calls.length ?? 0
    fireEvent.click(screen.getByTestId('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    expect(useCallPopout.getState().open).toBe(true)
    expect(screen.queryByTestId('call-tile')).toBeNull()
    // The call is untouched: still connected, nothing sent to the engine.
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls.length ?? 0).toBe(before)
    // While it's out, this window's top bar has the call's controls.
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    // The window closes (its close button, ⌘W): the tile is back; the call never dropped.
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls).not.toContain('disconnect')
  })

  it('Pop out from the expanded view shrinks it first, and notes which window it came from', async () => {
    installFullscreenApi()
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    fireEvent.click(screen.getByTestId('call-expanded-popout'))
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(fsCalls).toEqual([])
    await waitFor(() => expect(p.opened).toHaveLength(1))
    expect(screen.queryByTestId('call-expanded')).toBeNull()
    expect(localStorage.getItem(CALL_POPPED_FROM_KEY)).toBe('main')
  })

  it('no Pop out where the app cannot open windows (the web client)', async () => {
    await inCall()
    await cameraOn()
    expect(screen.queryByTestId('call-tile-popout')).toBeNull()
    expect(screen.getByTestId('call-tile-expand')).toBeTruthy()
  })

  it("the call's window draws the call and its controls, and closes itself when the call ends", async () => {
    await inCall()
    await cameraOn()
    const closeWindow = vi.fn()
    // The room's window has popped it out: its tile has given way.
    act(() => useCallPopout.setState({ open: true }))
    render(<CallWindow closeWindow={closeWindow} />)
    const win = screen.getByTestId('call-window')
    expect(within(win).getByTestId('call-window-title').textContent).toBe('Standup')
    expect(within(win).getByTestId('video-grid')).toBeTruthy()
    expect(within(win).getByTestId('call-window-duration').textContent).toMatch(/^\d+:\d\d$/)
    // No Expand here (the window is the call); Pop in instead.
    expect(within(win).queryByTestId('call-window-fullscreen')).toBeNull()
    expect(within(win).getByTestId('call-window-popin').getAttribute('aria-label')).toBe("Pop back into K2's window")
    fireEvent.click(within(win).getByTestId('call-overlay-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(closeWindow).not.toHaveBeenCalled()
    fireEvent.click(within(win).getByTestId('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(closeWindow).toHaveBeenCalledTimes(1)
  })

  it("the call's window puts a share in the middle and the people on the left", async () => {
    await inCall()
    fourPeopleAndAShare()
    await screen.findByTestId('call-tile')
    act(() => useCallPopout.setState({ open: true }))
    render(<CallWindow closeWindow={() => {}} />)
    const win = screen.getByTestId('call-window')
    expect(within(win).getByTestId('call-share-layout').dataset.main).toBe('TR_S7')
    expect(within(within(win).getByTestId('call-share-people')).getAllByTestId('video-tile')).toHaveLength(4)
    expect(within(win).queryByTestId('video-grid')).toBeNull()
  })

  it('Pop in closes the call window and hands the call back: the call view opens where it came from, with the same share in the middle', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    fourPeopleAndAShare([fakeTrack('9', { source: 'screen', track: 'TR_S9' })])
    await screen.findByTestId('call-tile')
    // Expanded here, Linus's share picked for the middle, then Pop out.
    fireEvent.click(screen.getByTestId('call-tile-expand'))
    fireEvent.click(screen.getByTestId('call-share-pick'))
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    fireEvent.click(screen.getByTestId('call-expanded-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    const before = h.voiceEngine?.calls.length ?? 0

    // The call's window is a fresh webview: its own store, the kept pick.
    act(() => __resetCallShareForTests())
    const closeWindow = vi.fn()
    render(<CallWindow closeWindow={closeWindow} />)
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    // Ada's share back in the middle, there.
    fireEvent.click(screen.getByTestId('call-share-pick'))
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S7')

    // The room's window still remembers its own pick.
    act(() => useCallShare.setState({ picked: 'TR_S9' }))
    fireEvent.click(screen.getByTestId('call-window-popin'))
    expect(closeWindow).toHaveBeenCalledTimes(1)
    expect(localStorage.getItem(CALL_HAND_BACK_KEY)).toContain('"to":"main"')
    // The app closes the window and tells every window. (It stays drawn
    // here: in this one document its unmount would stop the shared video
    // controller, which a real window's never shares.)
    p.emit(false)

    // Back here as the call view (not just the tile), with Ada's share.
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(useCallScreen.getState().open).toBe(true)
    expect(useCallShare.getState().picked).toBe('TR_S7')
    expect(localStorage.getItem(CALL_HAND_BACK_KEY)).toBeNull()
    expect(JSON.parse(localStorage.getItem(CALL_SHARE_PICK_KEY) ?? '{}').key).toBe('TR_S7')
    fireEvent.click(screen.getByTestId('video-stage-expand'))
    expect(within(screen.getByTestId('call-expanded')).getByTestId('call-share-layout').dataset.main).toBe('TR_S7')
    // The call itself never moved.
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls.length ?? 0).toBe(before)
  })

  it("closing the call's window with the OS's close button pops it back in as the tile, with the call still up", async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    fireEvent.click(screen.getByTestId('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(useCallScreen.getState().open).toBe(false)
    expect(voice.store.getState().call?.phase).toBe('connected')
  })

  it('a hand-back meant for another window, or a stale one, opens nothing here', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    const callId = voice.store.getState().call?.callId ?? ''
    fireEvent.click(screen.getByTestId('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to: 'window-2', at: Date.now() }))
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(useCallScreen.getState().open).toBe(false)
    // Still there for the window it's for.
    expect(localStorage.getItem(CALL_HAND_BACK_KEY)).toContain('window-2')
    fireEvent.click(screen.getByTestId('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(2))
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to: 'main', at: Date.now() - 60_000 }))
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(useCallScreen.getState().open).toBe(false)
  })

  it("the call's window waits for the call, then says there's none", async () => {
    vi.useFakeTimers({ toFake: ['setTimeout', 'clearTimeout'] })
    try {
      const closeWindow = vi.fn()
      render(<CallWindow closeWindow={closeWindow} />)
      expect(screen.getByTestId('call-window-empty').textContent).toContain('Finding the call')
      act(() => vi.advanceTimersByTime(9_000))
      expect(screen.getByTestId('call-window-empty').textContent).toContain("There's no call to show here.")
      fireEvent.click(screen.getByTestId('call-window-close'))
      expect(closeWindow).toHaveBeenCalledTimes(1)
    } finally {
      vi.useRealTimers()
    }
  })
})
