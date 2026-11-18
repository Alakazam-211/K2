// @vitest-environment jsdom
//
// How a call shows, by what it carries. Audio only (nobody's camera, no
// share): the top bar's slim bar has the whole call (name, time, Mute,
// Deafen, Camera, Hang up, Expand, Pop out), and there's no tile and no
// stage. Video (anyone's camera or a share): the floating tile in its corner
// (draggable, snapping to the nearest of four corners, the corner kept for
// this client) with the call's controls on its bar, and the top bar gives
// way. The video stopping goes back to the top bar, unless the person chose
// Expand (the stage in the call's place) or Pop out, which hold until the
// call ends. The stage's header has Float, Pop out and Full screen. There is
// no Hide and no Bigger. One call, one home window. Real voice and video
// controllers over the fake engines.

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

// Opening a Home row (Back to call opens the call's DM): recorded.
const opened = vi.hoisted(() => ({ rows: [] as unknown[] }))
vi.mock('@/lib/home-open', () => ({
  openHomeRow: (row: unknown) => {
    opened.rows.push(row)
    return 'selected'
  },
}))

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
        roomName: roomId.startsWith('dm:') ? 'Ada' : 'Standup',
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
    loadPrefs: () => ({ inputDeviceId: null, outputDeviceId: null, joinMuted: false, noiseRemoval: true }),
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
import { STAGE_INSET } from './VideoStage'
import { CALL_TILE_WIDTH } from './CallTile'
import { CORNER_KEY, __resetCornersForTests } from './useCornerDrag'
import { TOPBAR_HEIGHT_CSS } from '../../../shared/constants'
import { __resetCallExpandForTests, useCallExpand } from '@/stores/call-expand'
import { __resetCallShareForTests, useCallShare } from '@/stores/call-share'
import { __resetCallPopoutForTests, setCallPopoutBridge, useCallPopout, type CallPopoutBridge } from '@/stores/call-popout'
import { CALL_HAND_BACK_KEY, CALL_POPPED_FROM_KEY, CALL_SHARE_PICK_KEY, DOUBLE_CLICK_MS } from '@/lib/voice/call-view'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { voice } from '@/stores/voice'
import { video } from '@/stores/video'
import { __resetCallPlaceForTests, floatCall, useCallPlace } from '@/stores/call-place'
import { fakeTrack } from '@/lib/voice/fake-video-engine'
import { overlayClaims, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'
import { dmCallRoom } from '@/lib/home-chat/calls'
import { ConversationCallPlace, RoomCallBand } from './CallPlace'
import { useHomeChatStore } from '@/stores/home-chat'
import { __resetCallHomeForTests, setCallHomeBridge } from '@/stores/call-home'
import { CALL_HOME_KEY, readCallHome, writeCallHome } from '@/lib/voice/call-home'
import { usePanelsStore } from '@/stores/panels'
import { useCallTilePlace } from '@/stores/call-tile-place'
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

/** The app's window: the top bar's headset, the window's call layer, and
 *  the content area, with a voice room's band at its top. */
function AppWindow({ children }: { children?: React.ReactNode }): React.JSX.Element {
  return (
    <>
      <VoiceButton />
      <CallLayer />
      <div data-testid="content-area">
        <RoomCallBand />
        <div data-testid="page">{children}</div>
      </div>
    </>
  )
}

/** In a call on the native engine (the fake seat engine, drawn as native),
 *  with the top bar's headset and the window's call layer mounted. */
async function inCall(): Promise<void> {
  h.entries = { 'sewlab.k2.dev': entry('sewlab.k2.dev') }
  h.lists = { 'sewlab.k2.dev': list }
  act(() => voice.recompute())
  render(<AppWindow />)
  fireEvent.click(screen.getByTestId('voice-button'))
  fireEvent.click(await screen.findByTestId('voice-join'))
  await waitFor(() => expect(voice.store.getState().call?.phase).toBe('connected'))
  act(() => {
    voice.setPopoverOpen(false)
    voice.store.setState({ engineKind: 'native' })
  })
  if (h.videoPreview) await waitFor(() => expect(video.store.getState().availability).toBe('ready'))
}

const DM_HOST = 'sewlab.k2.dev'

/** A DM's view: its call's place above its messages. */
function DmView({ conv = '42' }: { conv?: string }): React.JSX.Element {
  return (
    <div data-testid="dm-view">
      <ConversationCallPlace hostKey={DM_HOST} roomId={`dm:${conv}`} fallback={<div data-testid="dm-seat-strip" />} />
      <div data-testid="dm-messages" />
    </div>
  )
}

/** In a DM call with Ada (conversation 42), its DM open (`open`) or not. */
async function inDmCall(open = true): Promise<ReturnType<typeof render>> {
  h.entries = { [DM_HOST]: entry(DM_HOST) }
  h.lists = { [DM_HOST]: list }
  act(() => voice.recompute())
  const r = render(<AppWindow>{open ? <DmView /> : null}</AppWindow>)
  await act(async () => {
    await voice.join(DM_HOST, dmCallRoom('42', 'Ada'))
  })
  await waitFor(() => expect(voice.store.getState().call?.phase).toBe('connected'))
  act(() => voice.store.setState({ engineKind: 'native' }))
  if (h.videoPreview) await waitFor(() => expect(video.store.getState().availability).toBe('ready'))
  return r
}

/** Camera on, through the top bar's button: the floating tile comes up. */
async function cameraOn(): Promise<HTMLElement> {
  fireEvent.click(await screen.findByTestId('call-topbar-camera'))
  return screen.findByTestId('call-tile')
}

/** Expand, from the floating tile: the stage comes up in the call's place. */
async function expandFromTile(): Promise<HTMLElement> {
  fireEvent.click(within(await screen.findByTestId('call-tile')).getByTestId('call-tile-expand'))
  return screen.findByTestId('video-stage')
}

/** Camera on, then Expand: the stage. */
async function stageWithCamera(): Promise<HTMLElement> {
  await cameraOn()
  return expandFromTile()
}

/** A button draws the shared keyboard focus ring in the look: the k2-focus
 *  class (a solid ring, round in Glass, square in Terminal), and for a
 *  labelled button its Terminal cut (the cut ring). */
function expectFocusRing(el: HTMLElement, cut: 'control' | 'chip' | null = 'control'): void {
  expect(el.tagName).toBe('BUTTON')
  expect(el.className).toMatch(/(^|\s)k2-focus(\s|$)/)
  expect(el.getAttribute('data-k2-chamfer')).toBe(cut)
}

/** A control inside the stage. */
function inStage(testId: string): HTMLElement {
  return within(screen.getByTestId('video-stage')).getByTestId(testId)
}

/** A control inside the floating tile. */
function inTile(testId: string): HTMLElement {
  return within(screen.getByTestId('call-tile')).getByTestId(testId)
}

/** The top bar's slim bar. */
function topBar(): HTMLElement {
  return screen.getByTestId('call-bar')
}

/** A click on the tile's picture that never drags (a press and a release). */
function tapPicture(): void {
  const picture = screen.getByTestId('call-tile-picture')
  fireEvent.pointerDown(picture, { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
  fireEvent.pointerUp(picture, { clientX: 101, clientY: 100, pointerId: 1, button: 0 })
}

beforeEach(async () => {
  h.videoPreview = true
  h.settingsOpen = false
  h.videoOpts = {}
  localStorage.clear()
  __resetCornersForTests()
  await act(async () => {
    if (voice.store.getState().call) await voice.leave()
    voice.store.setState({ rooms: {}, notice: null, popoverOpen: false, engineKind: 'seat' })
  })
  __resetCallPlaceForTests()
  useHomeChatStore.setState({ byHost: {} })
  __resetCallExpandForTests()
  __resetCallShareForTests()
  __resetCallPopoutForTests()
})
afterEach(() => {
  cleanup()
  uninstallFullscreenApi()
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
  resetBrowserOverlaysForTest()
})

describe('audio only: the call is the top bar slim bar; no tile, no stage', () => {
  it('shows Mute, Deafen, Noise removal, Camera and Leave call beside the headset, and Expand', async () => {
    await inCall()
    const bar = await screen.findByTestId('call-topbar-controls')
    expect(bar.querySelectorAll('button')).toHaveLength(5)
    expect(screen.getByTestId('call-topbar-noise').getAttribute('aria-label')).toBe('Noise removal on')
    expect(screen.getByTestId('call-topbar-mute').getAttribute('aria-label')).toBe('Mute')
    expect(screen.getByTestId('call-topbar-deafen').getAttribute('aria-label')).toBe('Deafen')
    expect(screen.getByTestId('call-topbar-camera').getAttribute('aria-label')).toBe('Turn camera on')
    expect(screen.getByTestId('call-topbar-leave').getAttribute('aria-label')).toBe('Leave call')
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    // Expand is the slim bar's, in words; no Hide, no Bigger anywhere.
    const expand = within(topBar()).getByTestId('call-bar-expand')
    expect(expand.textContent).toBe('Expand')
    expectFocusRing(expand)
    expect(screen.queryByText('Hide')).toBeNull()
    expect(screen.queryByText('Bigger')).toBeNull()
  })

  it('with video rooms off: no Camera, and still no tile or stage', async () => {
    h.videoPreview = false
    await inCall()
    expect(screen.getByTestId('call-topbar-controls').querySelectorAll('button')).toHaveLength(4)
    expect(screen.queryByTestId('call-topbar-camera')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
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

  it('a seat-only call (the web client: no audio here) shows no controls, no tile and no stage', async () => {
    await inCall()
    act(() => voice.store.setState({ engineKind: 'seat' }))
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })
})

describe('video on: the floating tile, and the top bar gives way', () => {
  it('your camera brings up the tile with your self-view and the call bar; stopping it goes back to the top bar', async () => {
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.picture).toBe('local:camera')
    expect(within(tile).getByTestId('video-tile').dataset.track).toBe('local:camera')
    // In its remembered corner: bottom right the first time.
    expect(tile.dataset.corner).toBe('bottom-right')
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    expect(screen.queryByTestId('call-bar')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
    // The call's larger controls, on the call's bar: Mute, Deafen, Noise
    // removal, Camera, Share screen, Leave call.
    expect(inTile('call-control-bar').contains(inTile('call-overlay-mute'))).toBe(true)
    expect(inTile('call-overlay-controls').querySelectorAll('button')).toHaveLength(6)
    // The call's time, on the picture at its bottom right, in the UI font.
    await waitFor(() => expect(inTile('call-tile-duration').textContent).toMatch(/^\d+:\d\d$/))
    expectUiFontTime(inTile('call-tile-duration'))
    // Floating over the page, never in its flow.
    expect(tile.className).toMatch(/\bfixed\b/)
    expect(screen.getByTestId('content-area').contains(tile)).toBe(false)
    // Camera off from the tile: no video left, so the top bar has the call again.
    fireEvent.click(inTile('call-overlay-camera'))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    expect(screen.queryByTestId('video-stage')).toBeNull()
  })

  it("the other side's camera brings up the tile, and it follows the active speaker's video", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7'), fakeTrack('8')]))
    const tile = await screen.findByTestId('call-tile')
    expect(tile.dataset.picture).toBe('TR_7')
    expect(screen.queryByTestId('call-bar')).toBeNull()
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
    // Their cameras stop: back to the top bar.
    act(() => h.videoEngine?.setTracks([]))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(screen.getByTestId('call-bar')).toBeTruthy()
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

  it("the tile's own actions: Expand and Pop out on the picture; no Bigger, no Hide, no resize", async () => {
    setCallPopoutBridge({ open: async () => {}, isOpen: async () => false, watch: async () => () => {} })
    await inCall()
    await cameraOn()
    const buttons = inTile('call-tile-view-buttons')
    expect(within(buttons).getByTestId('call-tile-expand').getAttribute('aria-label')).toMatch(/^Expand/)
    expect(within(buttons).getByTestId('call-tile-popout').getAttribute('aria-label')).toBe('Pop out into its own window')
    expect(screen.queryByText('Bigger')).toBeNull()
    expect(screen.queryByText('Hide')).toBeNull()
    expect(screen.getByTestId('call-tile').querySelector('[data-resize]')).toBeNull()
    // The picture keeps its width; the tile is never resized.
    expect(screen.getByTestId('call-tile-picture').style.width).toBe(`${CALL_TILE_WIDTH}px`)
  })

  it("Expand opens the stage in the call's place; Float puts the tile back; and again", async () => {
    await inCall()
    await cameraOn()
    const stage = await expandFromTile()
    expect(useCallPlace.getState().choice).toBe('stage')
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.queryByTestId('call-bar')).toBeNull()
    expect(screen.getByTestId('content-area').firstElementChild?.contains(stage)).toBe(true)
    // The stage's header: Float, Full screen (Pop out where the app can); no Compact, no Hide, no Bigger.
    const float = inStage('video-stage-float')
    expect(float.textContent).toBe('Float')
    expectFocusRing(float)
    expect(inStage('video-stage-fullscreen').getAttribute('aria-label')).toMatch(/^Full screen \(.+\)$/)
    expect(screen.queryByTestId('video-stage-compact')).toBeNull()
    expect(screen.queryByText('Compact')).toBeNull()
    expect(screen.queryByText('Hide')).toBeNull()
    expect(screen.queryByText('Bigger')).toBeNull()
    fireEvent.click(float)
    expect(useCallPlace.getState().choice).toBe('auto')
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(screen.queryByTestId('video-stage')).toBeNull()
    await expandFromTile()
    fireEvent.click(inStage('video-stage-float'))
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
  })

  it('a click on the picture expands the call; a click on a tile button never does', async () => {
    await inCall()
    await cameraOn()
    fireEvent.pointerDown(inTile('call-overlay-mute'), { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
    fireEvent.pointerUp(inTile('call-overlay-mute'), { clientX: 100, clientY: 100, pointerId: 1, button: 0 })
    fireEvent.click(inTile('call-overlay-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    await act(async () => new Promise((r) => setTimeout(r, DOUBLE_CLICK_MS + 20)))
    expect(useCallPlace.getState().choice).toBe('auto')
    tapPicture()
    // After the double-click window: Expand.
    expect(useCallPlace.getState().choice).toBe('auto')
    expect(await screen.findByTestId('video-stage', {}, { timeout: 2000 })).toBeTruthy()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('a double-click on the picture opens it full screen, and does not expand it', async () => {
    await inCall()
    await cameraOn()
    tapPicture()
    tapPicture()
    fireEvent.doubleClick(screen.getByTestId('call-tile-picture'))
    expect(useCallExpand.getState().expanded).toBe(true)
    await act(async () => new Promise((r) => setTimeout(r, DOUBLE_CLICK_MS + 20)))
    expect(useCallPlace.getState().choice).toBe('auto')
    // Full screen draws the video; the tile rests meanwhile.
    expect(screen.queryByTestId('call-tile')).toBeNull()
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
  })

  it('Expand holds through the video stopping and starting again, until Float', async () => {
    await inCall()
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({
        call: {
          ...c,
          participants: [
            { user: 'You', memberId: '1', isSelf: true, speaking: false, muted: false },
            { user: 'Ada', memberId: '7', speaking: false, muted: false },
          ],
        },
      })
    })
    await stageWithCamera()
    fireEvent.click(inStage('call-overlay-camera'))
    await waitFor(() => expect(video.store.getState().local.camera).toBe('off'))
    // No video, still the stage: everyone as their picture, and Float; no Full screen without video.
    const stage = screen.getByTestId('video-stage')
    const people = within(stage).getAllByTestId('video-tile')
    expect(people.map((t) => within(t).getByTestId('video-tile-name').textContent)).toEqual(['Ada', 'You'])
    expect(people.map((t) => t.dataset.state)).toEqual(['off', 'off'])
    expect(within(people[0]).getByTestId('video-tile-avatar')).toBeTruthy()
    expect(inStage('video-stage-float').getAttribute('title')).toBe('Float: the call back in the top bar')
    expect(within(stage).queryByTestId('video-stage-fullscreen')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.queryByTestId('call-bar')).toBeNull()
    // Video again: still the stage, never the tile.
    fireEvent.click(inStage('call-overlay-camera'))
    await waitFor(() => expect(within(screen.getByTestId('video-stage')).getByTestId('video-tile').dataset.track).toBe('local:camera'))
    expect(screen.queryByTestId('call-tile')).toBeNull()
    // Float with no video: the top bar has the call.
    fireEvent.click(inStage('call-overlay-camera'))
    await waitFor(() => expect(video.store.getState().local.camera).toBe('off'))
    fireEvent.click(inStage('video-stage-float'))
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.getByTestId('call-bar')).toBeTruthy()
  })

  it("Expand from the top bar's slim bar on an audio call opens the stage; the call's end forgets it", async () => {
    await inCall()
    fireEvent.click(within(topBar()).getByTestId('call-bar-expand'))
    // Nobody's video: everyone in the call as their picture.
    const stage = await screen.findByTestId('video-stage')
    const people = within(stage).getAllByTestId('video-tile')
    expect(people.length).toBeGreaterThan(0)
    for (const t of people) expect(t.dataset.state).toBe('off')
    expect(screen.queryByTestId('call-bar')).toBeNull()
    fireEvent.click(inStage('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(useCallPlace.getState().choice).toBe('auto')
  })

  it("the tile's buttons drive the engine, and Leave call takes the tile with the call", async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(inTile('call-overlay-mute'))
    expect(h.voiceEngine?.calls).toContain('muted true')
    // Muted, in words beside the mic.
    expect(inTile('call-overlay-muted').textContent).toBe('Muted')
    fireEvent.click(inTile('call-overlay-deafen'))
    expect(h.voiceEngine?.calls).toContain('deafened true')
    fireEvent.click(inTile('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it("the stage's buttons drive the engine, and Leave call takes the stage with the call", async () => {
    await inCall()
    await stageWithCamera()
    fireEvent.click(inStage('call-overlay-mute'))
    expect(h.voiceEngine?.calls).toContain('muted true')
    fireEvent.click(inStage('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(screen.queryByTestId('video-stage')).toBeNull()
  })

  it('Settings open: the tile steps back and the top bar has the call', async () => {
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

describe('the stage lives in its place: a DM call in its conversation, a voice room at the top of the page', () => {
  it("a voice room's stage sits at the top of the content area, in the page, with the page under it", async () => {
    await inCall()
    const stage = await stageWithCamera()
    const area = screen.getByTestId('content-area')
    expect(area.contains(stage)).toBe(true)
    expect(area.firstElementChild?.contains(stage)).toBe(true)
    expect(area.lastElementChild).toBe(screen.getByTestId('page'))
    // In the flow of the page: never fixed over it.
    for (let el: HTMLElement | null = stage; el && el !== area; el = el.parentElement) expect(el.className).not.toMatch(/\bfixed\b/)
  })

  it("audio only: the DM gives the call no room; the top bar's slim bar has its name, time and controls", async () => {
    await inDmCall()
    const view = screen.getByTestId('dm-view')
    expect(within(view).queryByTestId('call-bar')).toBeNull()
    expect(view.firstElementChild).toBe(screen.getByTestId('dm-messages'))
    const bar = topBar()
    expect(within(bar).getByTestId('call-bar-title').textContent).toBe('Call with Ada')
    await waitFor(() => expect(within(bar).getByTestId('call-bar-time').textContent).toMatch(/^\d+:\d\d$/))
    expect(screen.getAllByTestId('call-topbar-controls')).toHaveLength(1)
    fireEvent.click(within(bar).getByTestId('call-topbar-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('video on in the DM: the tile floats; Expand puts the stage at the top of the DM, messages below; Float floats it again', async () => {
    await inDmCall()
    const view = screen.getByTestId('dm-view')
    const tile = await cameraOn()
    expect(view.contains(tile)).toBe(false)
    expect(view.firstElementChild).toBe(screen.getByTestId('dm-messages'))
    const stage = await expandFromTile()
    expect(view.firstElementChild?.contains(stage)).toBe(true)
    expect(view.lastElementChild).toBe(screen.getByTestId('dm-messages'))
    expect(within(stage).getByTestId('video-tile').dataset.track).toBe('local:camera')
    // A set height (under half the window), so the tiles fit in the room
    // above the controls rather than running under them.
    expect(stage.className).toContain('h-[calc(var(--k2-vh)*0.45)]')
    const box = within(stage).getByTestId('video-grid-box')
    expect(box.className).toContain('@container-[size]')
    expect(box.contains(within(stage).getByTestId('video-stage-controls'))).toBe(false)
    // Once: not in the band, not in the top bar, no tile.
    expect(screen.getAllByTestId('video-stage')).toHaveLength(1)
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    fireEvent.click(within(stage).getByTestId('video-stage-float'))
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
  })

  it('away from the DM, the tile keeps floating over the other page', async () => {
    const r = await inDmCall()
    await cameraOn()
    r.rerender(<AppWindow />)
    expect(screen.getByTestId('call-tile')).toBeTruthy()
    expect(screen.queryByTestId('call-bar')).toBeNull()
  })

  it("away from the DM with the stage chosen, it is drawn nowhere and the top bar has the call; opening the DM brings the stage back", async () => {
    const r = await inDmCall()
    await stageWithCamera()
    r.rerender(<AppWindow />)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    await waitFor(() => expect(video.sinks()).toEqual([]), { timeout: 3500 })
    r.rerender(
      <AppWindow>
        <DmView />
      </AppWindow>,
    )
    expect(await within(screen.getByTestId('dm-view')).findByTestId('video-stage')).toBeTruthy()
  })

  it("the tile's Expand away from the DM opens the DM, with its stage", async () => {
    opened.rows = []
    const r = await inDmCall(false)
    act(() => useHomeChatStore.getState().noteDm(DM_HOST, 'k2:connect:ada', '42'))
    await cameraOn()
    fireEvent.click(inTile('call-tile-expand'))
    expect(opened.rows).toEqual([{ kind: 'person', address: 'k2:connect:ada::sewlab.k2.dev', workspaceId: null, label: 'Ada' }])
    expect(useCallPlace.getState().choice).toBe('stage')
    r.rerender(
      <AppWindow>
        <DmView />
      </AppWindow>,
    )
    expect(await within(screen.getByTestId('dm-view')).findByTestId('video-stage')).toBeTruthy()
  })

  it("a DM call whose DM this window can't find has no Expand on its tile or its slim bar (never a dead button)", async () => {
    await inDmCall(false)
    expect(within(topBar()).queryByTestId('call-bar-expand')).toBeNull()
    await cameraOn()
    expect(within(screen.getByTestId('call-tile')).queryByTestId('call-tile-expand')).toBeNull()
  })

  it("another DM shows nothing of this call, and a call without the app's engine keeps the DM's own strip", async () => {
    const r = await inDmCall()
    r.rerender(
      <AppWindow>
        <DmView conv="43" />
      </AppWindow>,
    )
    expect(within(screen.getByTestId('dm-view')).queryByTestId('call-bar')).toBeNull()
    expect(within(screen.getByTestId('dm-view')).queryByTestId('dm-seat-strip')).toBeNull()
    r.rerender(
      <AppWindow>
        <DmView />
      </AppWindow>,
    )
    act(() => voice.store.setState({ engineKind: 'seat' }))
    expect(within(screen.getByTestId('dm-view')).getByTestId('dm-seat-strip')).toBeTruthy()
    expect(screen.queryByTestId('call-bar')).toBeNull()
  })
})

/** A call's time is set in the look's UI font, as the words beside it are
 *  (the system font in Glass, the code face in Terminal, whose UI font it
 *  is), with tabular figures so it never jitters as it counts. */
function expectUiFontTime(el: HTMLElement): void {
  expect(el.className).not.toMatch(/\bfont-mono\b/)
  expect(el.className).toMatch(/\btabular-nums\b/)
}

describe('the slim bar in the top bar', () => {
  it("a voice room: the call's name, its time, its controls and Expand beside the headset", async () => {
    await inCall()
    const bar = topBar()
    expect(within(bar).getByTestId('call-bar-title').textContent).toBe('Standup')
    await waitFor(() => expect(within(bar).getByTestId('call-bar-time').textContent).toMatch(/^\d+:\d\d$/))
    expectUiFontTime(within(bar).getByTestId('call-bar-time'))
    expect(bar.contains(screen.getByTestId('call-topbar-mute'))).toBe(true)
    expect(bar.contains(screen.getByTestId('call-topbar-leave'))).toBe(true)
    expect(within(bar).queryByTestId('call-bar-back')).toBeNull()
    expect(within(bar).getByTestId('call-bar-expand')).toBeTruthy()
    // In the top bar's flow, never floating.
    expect(bar.className).not.toMatch(/\b(fixed|absolute)\b/)
    // A narrow top bar folds it (call-bar-fit): mute and hang up always
    // stay; the others can go, as they're in the headset's list too.
    expect(screen.getByTestId('call-topbar-mute').hasAttribute('data-call-optional')).toBe(false)
    expect(screen.getByTestId('call-topbar-leave').hasAttribute('data-call-optional')).toBe(false)
    expect(screen.getByTestId('call-topbar-deafen').hasAttribute('data-call-optional')).toBe(true)
    // Every button on the slim bar shows keyboard focus in the look.
    for (const b of Array.from(screen.getByTestId('call-topbar-controls').querySelectorAll('button'))) {
      expectFocusRing(b as HTMLElement, null)
      expect(b.className).toContain('rounded-[var(--radius-pill)]')
    }
  })

  it('Pop out is on the slim bar where the app can open windows, in words, for an audio call too', async () => {
    const opens: unknown[] = []
    setCallPopoutBridge({ open: async (f) => void opens.push(f), isOpen: async () => false, watch: async () => () => {} })
    await inCall()
    const pop = within(topBar()).getByTestId('call-bar-popout')
    expect(pop.textContent).toBe('Pop out')
    expect(pop.getAttribute('aria-label')).toBe('Pop out')
    expectFocusRing(pop)
    fireEvent.click(pop)
    await waitFor(() => expect(opens).toHaveLength(1))
    expect(useCallPopout.getState().open).toBe(true)
    expect(within(topBar()).getByTestId('call-bar-where').textContent).toBe('In its own window')
    expect(within(topBar()).queryByTestId('call-bar-popout')).toBeNull()
  })

  it('no Pop out on the slim bar where the app cannot open windows (the web client)', async () => {
    await inCall()
    expect(within(topBar()).queryByTestId('call-bar-popout')).toBeNull()
  })

  it("a DM call away from its DM with the stage chosen: Back to call opens the DM, with its stage", async () => {
    opened.rows = []
    const r = await inDmCall(false)
    act(() => useHomeChatStore.getState().noteDm(DM_HOST, 'k2:connect:ada', '42'))
    fireEvent.click(screen.getByTestId('call-topbar-camera'))
    await waitFor(() => expect(video.store.getState().local.camera).not.toBe('off'))
    act(() => useCallPlace.setState({ choice: 'stage' }))
    const bar = topBar()
    expect(within(bar).getByTestId('call-bar-title').textContent).toBe('Call with Ada')
    expect(within(bar).queryByTestId('call-bar-expand')).toBeNull()
    const back = within(bar).getByTestId('call-bar-back')
    expect(back.textContent).toBe('Back to call')
    // Folded to its icon in a narrow top bar, it keeps its name.
    expect(back.getAttribute('aria-label')).toBe('Back to call')
    expect(back.querySelector('[data-call-bar-glyph] svg')).not.toBeNull()
    expect(back.querySelector('[data-call-bar-label]')?.textContent).toBe('Back to call')
    expectFocusRing(back)
    fireEvent.click(back)
    expect(opened.rows).toEqual([{ kind: 'person', address: 'k2:connect:ada::sewlab.k2.dev', workspaceId: null, label: 'Ada' }])
    expect(useCallPlace.getState().choice).toBe('stage')
    // The app opens the DM: the stage is at its top, and the top bar's bar is gone.
    r.rerender(
      <AppWindow>
        <DmView />
      </AppWindow>,
    )
    expect(await within(screen.getByTestId('dm-view')).findByTestId('video-stage')).toBeTruthy()
    expect(screen.queryByTestId('call-bar')).toBeNull()
  })

  it("an audio DM call away from its DM: Expand opens the DM, with its stage", async () => {
    opened.rows = []
    await inDmCall(false)
    act(() => useHomeChatStore.getState().noteDm(DM_HOST, 'k2:connect:ada', '42'))
    const expand = await within(topBar()).findByTestId('call-bar-expand')
    fireEvent.click(expand)
    expect(opened.rows).toHaveLength(1)
    expect(useCallPlace.getState().choice).toBe('stage')
  })
})

describe('one call, one home window (this window is main; the app lists main and window-2)', () => {
  const ROOM = 'sewlab.k2.dev|room-1'
  const later = (): number => Date.now() + 60_000

  beforeEach(() => setCallHomeBridge({ liveWindows: async () => ['main', 'window-2', 'window-call'] }))
  afterEach(() => __resetCallHomeForTests())

  /** Another window's write, as this window hears it. */
  function otherWindowWrites(owner: string): void {
    act(() => {
      writeCallHome({ call: ROOM, owner, until: later() })
      window.dispatchEvent(new StorageEvent('storage', { key: CALL_HOME_KEY }))
    })
  }

  it('another window is home: this one draws no tile and opens no sinks; Show call here brings the tile here', async () => {
    writeCallHome({ call: ROOM, owner: 'window-2', until: later() })
    await inCall()
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    await waitFor(() => expect(video.store.getState().local.camera).not.toBe('off'))
    const here = await screen.findByTestId('call-bar-here')
    expect(here.textContent).toBe('Show call here')
    expectFocusRing(here)
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
    // No Expand or Pop out here: those are the home window's.
    expect(screen.queryByTestId('call-bar-expand')).toBeNull()
    expect(video.sinks()).toEqual([])
    // Full screen's chord does nothing here either.
    fireEvent.keyDown(window, fullScreenChord())
    expect(useCallExpand.getState().expanded).toBe(false)
    fireEvent.click(here)
    expect(readCallHome()).toMatchObject({ call: ROOM, owner: 'main' })
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['local:camera']))
    expect(screen.queryByTestId('call-bar-here')).toBeNull()
  })

  it('home here, then Show call here in another window: this one keeps the slim bar and lets its video go', async () => {
    await inCall()
    await cameraOn()
    await waitFor(() => expect(readCallHome()).toMatchObject({ call: ROOM, owner: 'main' }))
    otherWindowWrites('window-2')
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.getByTestId('call-bar-here')).toBeTruthy()
    await waitFor(() => expect(video.sinks()).toEqual([]), { timeout: 3500 })
    // The call itself never moved.
    expect(voice.store.getState().call?.phase).toBe('connected')
  })

  it("with the call in its own window, the window it came from says Back to chat, which brings the stage back", async () => {
    localStorage.setItem(CALL_POPPED_FROM_KEY, 'main')
    writeCallHome({ call: ROOM, owner: 'window-call', until: later() })
    await inCall()
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    const back = await screen.findByTestId('call-bar-back-to-chat')
    expect(back.textContent).toBe('Back to chat')
    expectFocusRing(back)
    expect(screen.queryByTestId('call-bar-here')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    fireEvent.click(back)
    expect(readCallHome()?.owner).toBe('main')
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('with the call in its own window, the slim bar says so, video or not, and offers to bring it back', async () => {
    localStorage.setItem(CALL_POPPED_FROM_KEY, 'main')
    writeCallHome({ call: ROOM, owner: 'window-call', until: later() })
    await inCall()
    const bar = await screen.findByTestId('call-bar')
    expect((await within(bar).findByTestId('call-bar-where')).textContent).toBe('In its own window')
    expect(within(bar).getByTestId('call-bar-back-to-chat')).toBeTruthy()
    // Brought back: it no longer says so.
    fireEvent.click(within(bar).getByTestId('call-bar-back-to-chat'))
    await waitFor(() => expect(screen.queryByTestId('call-bar-where')).toBeNull())
  })

  it("another window that didn't pop it out says Show call here", async () => {
    localStorage.setItem(CALL_POPPED_FROM_KEY, 'window-2')
    writeCallHome({ call: ROOM, owner: 'window-call', until: later() })
    await inCall()
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    expect(await screen.findByTestId('call-bar-here')).toBeTruthy()
    expect(screen.queryByTestId('call-bar-back-to-chat')).toBeNull()
  })

  it("the call's own window closes itself when another window takes the stage; the call goes on", async () => {
    writeCallHome({ call: ROOM, owner: 'window-call', until: later() })
    await inCall()
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    const closeWindow = vi.fn()
    render(<CallWindow closeWindow={closeWindow} />)
    await waitFor(() => expect(within(screen.getByTestId('call-window')).getByTestId('video-grid')).toBeTruthy())
    // The stage's one inset, and none of the window's own on top of it.
    const box = within(screen.getByTestId('call-window')).getByTestId('video-grid-box')
    expect(box.style.padding).toBe(STAGE_INSET)
    expect(box.className).not.toMatch(/(^|\s)p[xytrbl]?-\d/)
    expect(closeWindow).not.toHaveBeenCalled()
    otherWindowWrites('main')
    await waitFor(() => expect(closeWindow).toHaveBeenCalledTimes(1))
    expect(voice.store.getState().call?.phase).toBe('connected')
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
    fireEvent.click(inTile('call-overlay-camera'))
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
    fireEvent.click(inTile('call-overlay-screen'))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(h.videoEngine?.calls).toContain('screen {"on":false}')
  })

  it("where the OS only lists, the tile's Share screen shows the list under it, in K2's own menu", async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(inTile('call-overlay-screen'))
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

// ── Full screen, labels, pop out ──

/** The element Fullscreen API, as WKWebView has it, recording every request:
 *  Full screen stays inside the app and never asks for OS full screen. */
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

function fullScreenChord(): KeyboardEventInit {
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
  it('the tile: the name at the bottom left and the time at the bottom right, both on the picture, on the look\'s chip', async () => {
    await inCall()
    await cameraOn()
    const picture = screen.getByTestId('call-tile-picture')
    const label = within(picture).getByTestId('video-tile-label')
    expect(label.className).toContain('bg-[var(--color-media-label-bg)]')
    expect(within(label).getByTestId('video-tile-name').textContent).toBe('You')
    const corner = within(picture).getByTestId('video-tile-corner')
    expect(corner.dataset.at).toBe('bottom-right')
    await waitFor(() => expect(within(corner).getByTestId('call-tile-duration').textContent).toMatch(/^\d+:\d\d$/))
    // The look's tile surface: rounded in Glass, cut in Terminal.
    expect(within(picture).getByTestId('video-tile').getAttribute('data-k2-chamfer')).toBe('surface')
    // Its float shadow follows the cut in Terminal.
    expect(picture.hasAttribute('data-k2-chamfer-float')).toBe(true)
  })

  it("every tile on the stage carries its name on the picture (the self-view's too)", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7')]))
    await expandFromTile()
    fireEvent.click(inStage('call-overlay-camera'))
    await waitFor(() => expect(within(screen.getByTestId('video-grid')).getAllByTestId('video-tile-label')).toHaveLength(2))
    for (const l of within(screen.getByTestId('video-grid')).getAllByTestId('video-tile-label')) {
      expect(l.className).toContain('bg-[var(--color-media-label-bg)]')
    }
  })
})

describe('full screen: the call over K2, inside the app', () => {
  it("the stage's Full screen opens the call over the whole window under the top bar, never OS full screen; Exit full screen puts the stage back", async () => {
    installFullscreenApi()
    await inCall()
    await stageWithCamera()
    const full = inStage('video-stage-fullscreen')
    full.focus()
    fireEvent.click(full)
    expect(useCallExpand.getState().expanded).toBe(true)
    const view = screen.getByTestId('call-expanded')
    expect(view.getAttribute('role')).toBe('dialog')
    expect(view.getAttribute('aria-modal')).toBe('true')
    expect(view.getAttribute('aria-label')).toBe('Standup, full screen')
    // The whole window under the top bar, to the window's own inset: its top
    // edge is the top bar's bottom edge, never across it, and nothing of the
    // page shows round it or through it (an opaque fill, no Frost). The top
    // bar stays in view, dimmed by the scrim, and a click there exits.
    expect(view.className).toMatch(/\bfixed\b/)
    expect(view.style.top).toBe(`calc(var(--inset-window) + ${TOPBAR_HEIGHT_CSS})`)
    expect(view.style.left).toBe('var(--inset-window)')
    expect(view.style.right).toBe('var(--inset-window)')
    expect(view.style.bottom).toBe('var(--inset-window)')
    expect(view.className).toContain('bg-[var(--color-bg)]')
    expect(view.closest('[data-material="frost"]')).toBeNull()
    expect(view.hasAttribute('data-material')).toBe(false)
    expect(Number(view.style.zIndex)).toBeGreaterThan(Number(screen.getByTestId('call-expanded-scrim').style.zIndex))
    expect(screen.getByTestId('call-expanded-scrim').dataset.scrim).toBe('')
    expect(fsCalls).toEqual([])
    // The stage gives way; the video is drawn once, in the view.
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.queryByTestId('call-tile')).toBeNull()
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['local:camera']))
    // Focus moves in, to Exit full screen.
    const exit = within(view).getByTestId('call-expanded-exit')
    expect(document.activeElement).toBe(exit)
    expect(exit.textContent).toBe('Exit full screen')
    fireEvent.click(exit)
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(screen.queryByTestId('call-expanded')).toBeNull()
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    // Focus is back on the stage's Full screen button.
    expect(document.activeElement).toBe(inStage('video-stage-fullscreen'))
  })

  it('a click on the scrim round it exits; a click inside does not', async () => {
    await inCall()
    await stageWithCamera()
    inStage('video-stage-fullscreen').focus()
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.click(screen.getByTestId('call-expanded-title'))
    expect(useCallExpand.getState().expanded).toBe(true)
    fireEvent.click(screen.getByTestId('call-expanded-scrim'))
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(document.activeElement).toBe(inStage('video-stage-fullscreen'))
  })

  it('Esc exits, and focus goes back to the Full screen button (a click there never focused it, as in WebKit)', async () => {
    await inCall()
    await stageWithCamera()
    expect(document.activeElement).toBe(document.body)
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(document.activeElement).toBe(inStage('video-stage-fullscreen'))
  })

  it('Tab stays inside the view', async () => {
    await inCall()
    await stageWithCamera()
    fireEvent.click(inStage('video-stage-fullscreen'))
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
    await stageWithCamera()
    fireEvent.click(inStage('video-stage-fullscreen'))
    const bar = screen.getByTestId('call-expanded-controls')
    expect(bar.tagName).toBe('FOOTER')
    expect(screen.getByTestId('call-expanded').lastElementChild).toBe(bar)
    // Mute, Deafen, Noise removal, Camera, Share screen, Leave call.
    expect(within(bar).getByTestId('call-overlay-controls').querySelectorAll('button')).toHaveLength(6)
    fireEvent.click(within(bar).getByTestId('call-overlay-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(screen.getByTestId('call-expanded-duration').textContent).toMatch(/^\d+:\d\d$/)
    expectUiFontTime(screen.getByTestId('call-expanded-duration'))
  })

  it('the chord toggles it (⌃⌘V on macOS, Ctrl+Alt+V elsewhere)', async () => {
    installFullscreenApi()
    await inCall()
    await stageWithCamera()
    fireEvent.keyDown(window, fullScreenChord())
    expect(useCallExpand.getState().expanded).toBe(true)
    fireEvent.keyDown(window, fullScreenChord())
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(fsCalls).toEqual([])
  })

  it('the chord does nothing without video', async () => {
    await inCall()
    fireEvent.keyDown(window, fullScreenChord())
    expect(useCallExpand.getState().expanded).toBe(false)
  })

  it('Leave call in full screen ends the call and the view with it', async () => {
    await inCall()
    await stageWithCamera()
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.click(screen.getByTestId('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(screen.queryByTestId('call-expanded')).toBeNull()
  })

  it('from the tile, the chord opens full screen, and exiting goes back to the tile', async () => {
    await inCall()
    await cameraOn()
    fireEvent.keyDown(window, fullScreenChord())
    expect(useCallExpand.getState().expanded).toBe(true)
    expect(screen.queryByTestId('call-tile')).toBeNull()
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
  })

  it('claims the window over a live Browser page while open', async () => {
    await inCall()
    await stageWithCamera()
    const before = overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length
    fireEvent.click(inStage('video-stage-fullscreen'))
    expect(overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length).toBe(before + 1)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length).toBe(before)
  })
})

describe('a screen share in the middle, the people on the left', () => {
  /** The share is up: Expand from the tile, then full screen from the stage. */
  async function fullScreen(): Promise<void> {
    await expandFromTile()
    fireEvent.click(inStage('video-stage-fullscreen'))
  }

  it('full screen: the share takes the middle, and everyone is a smaller tile in the column, with names, speaking and muted', async () => {
    await inCall()
    fourPeopleAndAShare()
    await fullScreen()
    const layout = await screen.findByTestId('call-share-layout')
    expect(layout.dataset.main).toBe('TR_S7')
    // The same inset as the grid's.
    expect(layout.style.padding).toBe(STAGE_INSET)
    expect(layout.className).not.toMatch(/(^|\s)p[xytrbl]?-\d/)
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

  it('full screen: the share in the middle zooms (only it, not the column); Esc on it fits first, the next Esc exits', async () => {
    await inCall()
    fourPeopleAndAShare()
    await fullScreen()
    const main = within(await screen.findByTestId('call-share-main')).getByTestId('video-tile')
    expect(main.dataset.zoomable).toBe('true')
    expect(within(screen.getByTestId('call-share-column')).queryAllByTestId('video-zoom-controls')).toEqual([])
    fireEvent.click(within(main).getByTestId('video-zoom-in'))
    expect(main.dataset.zoomed).toBe('true')
    expect(within(main).getByTestId('video-zoom-percent').textContent).toBe('150%')
    // The view's Esc (a window listener) leaves a zoomed share's Esc to it.
    const zoomIn = within(main).getByTestId('video-zoom-in')
    zoomIn.focus()
    fireEvent.keyDown(zoomIn, { key: 'Escape' })
    expect(useCallExpand.getState().expanded).toBe(true)
    expect(main.dataset.zoomed).toBeUndefined()
    fireEvent.keyDown(zoomIn, { key: 'Escape' })
    expect(useCallExpand.getState().expanded).toBe(false)
  })

  it('a zoomed share that stops takes its zoom with it; the next share starts at fit', async () => {
    await inCall()
    fourPeopleAndAShare()
    await fullScreen()
    const main = within(await screen.findByTestId('call-share-main')).getByTestId('video-tile')
    fireEvent.click(within(main).getByTestId('video-zoom-in'))
    expect(main.dataset.zoomed).toBe('true')
    act(() => h.videoEngine?.setTracks([fakeTrack('8')]))
    expect(await screen.findByTestId('video-grid')).toBeTruthy()
    act(() => h.videoEngine?.setTracks([fakeTrack('9', { source: 'screen', track: 'TR_S9' }), fakeTrack('8')]))
    const next = within(await screen.findByTestId('call-share-main')).getByTestId('video-tile')
    expect(next.dataset.track).toBe('TR_S9')
    expect(next.dataset.zoomed).toBeUndefined()
    expect(within(next).queryByTestId('video-zoom-percent')).toBeNull()
  })

  it('no share: the grid, as before', async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7'), fakeTrack('8')]))
    await fullScreen()
    expect(within(screen.getByTestId('call-expanded')).getByTestId('video-grid')).toBeTruthy()
    expect(screen.queryByTestId('call-share-layout')).toBeNull()
  })

  it('the layout switches as a share starts and stops', async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('8')]))
    await fullScreen()
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
    await fullScreen()
    expect((await screen.findByTestId('call-share-layout')).dataset.main).toBe('TR_S7')
    const pick = screen.getByTestId('call-share-pick')
    expect(pick.dataset.track).toBe('TR_S9')
    expect(pick.getAttribute('aria-label')).toBe("Show Linus's screen in the middle")
    // Keyboard focus rings it inside its own box (the column scrolls, so a
    // ring outside would be clipped): cut in Terminal, rounded in Glass.
    expect(pick.hasAttribute('data-k2-focus')).toBe(true)
    fireEvent.click(pick)
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    expect(screen.getByTestId('call-share-pick').dataset.track).toBe('TR_S7')
    // The pick sticks while that share lasts; when it stops, the other takes the middle.
    act(() => h.videoEngine?.setTracks([fakeTrack('7', { source: 'screen', track: 'TR_S7' }), fakeTrack('8')]))
    await waitFor(() => expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S7'))
    expect(screen.queryByTestId('call-share-pick')).toBeNull()
  })

  it("the tile following someone talking shows a cue for the share it isn't showing; the cue opens full screen onto it", async () => {
    await inCall()
    fourPeopleAndAShare()
    // Grace talks: the tile follows her camera, and the cue names Ada's share.
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({ call: { ...c, participants: c.participants.map((p) => ({ ...p, speaking: p.memberId === '8' })) } })
    })
    // After the last speaker's hold (1.5 s).
    await waitFor(() => expect(screen.getByTestId('call-tile').dataset.picture).toBe('TR_8'), { timeout: 3000 })
    const cue = inTile('call-tile-share-cue')
    expect(cue.dataset.track).toBe('TR_S7')
    expect(cue.textContent).toContain("Ada's screen")
    expectFocusRing(cue, 'chip')
    fireEvent.click(cue)
    expect(useCallExpand.getState().expanded).toBe(true)
    expect((await screen.findByTestId('call-share-layout')).dataset.main).toBe('TR_S7')
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

  it("the tile's Pop out takes the call from here without touching it; closing the window brings the tile back", async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    const before = h.voiceEngine?.calls.length ?? 0
    fireEvent.click(inTile('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    expect(useCallPopout.getState().open).toBe(true)
    expect(screen.queryByTestId('call-tile')).toBeNull()
    // The call is untouched: still connected, nothing sent to the engine.
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls.length ?? 0).toBe(before)
    // While it's out, this window's top bar has the call's controls, and says where it went.
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    expect(screen.getByTestId('call-bar-where').textContent).toBe('In its own window')
    // The window closes (its close button, ⌘W): the tile is back; the call never dropped.
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls).not.toContain('disconnect')
  })

  it('Pop out holds through the video stopping and starting again: this window keeps the slim bar', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    fireEvent.click(inTile('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    fireEvent.click(screen.getByTestId('call-topbar-camera'))
    await waitFor(() => expect(video.store.getState().local.camera).toBe('off'))
    expect(useCallPopout.getState().open).toBe(true)
    expect(screen.getByTestId('call-bar-where')).toBeTruthy()
    fireEvent.click(screen.getByTestId('call-topbar-camera'))
    await waitFor(() => expect(video.store.getState().local.camera).not.toBe('off'))
    expect(useCallPopout.getState().open).toBe(true)
    expect(screen.queryByTestId('call-tile')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.getByTestId('call-bar-where')).toBeTruthy()
  })

  it("the stage's Pop out, then closing the window, brings the stage back: Expand still holds", async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await stageWithCamera()
    const pop = inStage('video-stage-popout')
    expect(pop.getAttribute('aria-label')).toBe('Pop out into its own window')
    fireEvent.click(pop)
    await waitFor(() => expect(p.opened).toHaveLength(1))
    expect(screen.queryByTestId('video-stage')).toBeNull()
    p.emit(false)
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(screen.queryByTestId('call-tile')).toBeNull()
  })

  it('Pop out from full screen exits it first, and notes which window it came from', async () => {
    installFullscreenApi()
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await stageWithCamera()
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.click(screen.getByTestId('call-expanded-popout'))
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(fsCalls).toEqual([])
    await waitFor(() => expect(p.opened).toHaveLength(1))
    expect(screen.queryByTestId('call-expanded')).toBeNull()
    expect(localStorage.getItem(CALL_POPPED_FROM_KEY)).toBe('main')
  })

  it("a window that won't open says so where Pop out was pressed, with Try again and Dismiss; the call stays here", async () => {
    let tries = 0
    setCallPopoutBridge({
      open: async () => {
        tries += 1
        throw new Error('no display')
      },
      isOpen: async () => false,
      watch: async () => () => {},
    })
    await inCall()
    await stageWithCamera()
    fireEvent.click(inStage('video-stage-popout'))
    const line = await within(await screen.findByTestId('video-stage')).findByTestId('call-popout-error')
    expect(line.getAttribute('role')).toBe('alert')
    expect(line.textContent).toContain("Couldn't open the call in its own window: no display")
    fireEvent.click(within(line).getByTestId('call-popout-retry'))
    await waitFor(() => expect(tries).toBe(2))
    expect(await screen.findByTestId('call-popout-error')).toBeTruthy()
    fireEvent.click(within(screen.getByTestId('call-popout-error')).getByTestId('call-popout-dismiss'))
    expect(screen.queryByTestId('call-popout-error')).toBeNull()
    expect(useCallPopout.getState().error).toBeNull()
    expect(inStage('video-stage-popout')).toBeTruthy()
    // From the tile, the same line sits under its controls.
    fireEvent.click(inStage('video-stage-float'))
    fireEvent.click(within(await screen.findByTestId('call-tile')).getByTestId('call-tile-popout'))
    expect(await within(await screen.findByTestId('call-tile-notices')).findByTestId('call-popout-error')).toBeTruthy()
  })

  it('no Pop out where the app cannot open windows (the web client)', async () => {
    await inCall()
    await cameraOn()
    expect(screen.queryByTestId('call-tile-popout')).toBeNull()
    await expandFromTile()
    expect(screen.queryByTestId('video-stage-popout')).toBeNull()
    expect(inStage('video-stage-fullscreen')).toBeTruthy()
  })

  it("the call's window draws the call and its controls, says Back to chat, and closes itself when the call ends", async () => {
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
    expectUiFontTime(within(win).getByTestId('call-window-duration'))
    // No Full screen here (the OS's own is the window's); Back to chat instead.
    expect(within(win).queryByTestId('call-window-fullscreen')).toBeNull()
    const back = within(win).getByTestId('call-window-popin')
    expect(back.textContent).toBe('Back to chat')
    expect(back.getAttribute('title')).toBe('Back to chat: the call goes back into the window it came from, and this one closes')
    // In the look: the title in the heading size, the controls on the call's bar.
    expect(within(win).getByTestId('call-window-title').className).toContain('type-heading')
    expect(within(win).getByTestId('call-control-bar').contains(within(win).getByTestId('call-overlay-leave'))).toBe(true)
    fireEvent.click(within(win).getByTestId('call-overlay-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(closeWindow).not.toHaveBeenCalled()
    fireEvent.click(within(win).getByTestId('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(closeWindow).toHaveBeenCalledTimes(1)
  })

  it("the call's window keeps the video above its controls: the tiles fit the room left at any window size", async () => {
    await inCall()
    await cameraOn()
    act(() => useCallPopout.setState({ open: true }))
    render(<CallWindow closeWindow={() => {}} />)
    const win = screen.getByTestId('call-window')
    const box = within(win).getByTestId('video-grid-box')
    const controls = within(win).getByTestId('call-window-controls')
    // The tiles measure their own box (a size container) and fit inside it.
    expect(box.className).toContain('@container-[size]')
    expect(box.className).toMatch(/\bflex-1\b/)
    expect(box.className).toMatch(/\bmin-h-0\b/)
    // The bar is the box's sibling, after it: they share no space.
    expect(box.parentElement).toBe(controls.parentElement)
    expect(box.contains(controls)).toBe(false)
    expect(controls.className).toMatch(/\bflex-shrink-0\b/)
    expect(box.compareDocumentPosition(controls) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
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
    // The share zooms here too.
    const main = within(within(win).getByTestId('call-share-main')).getByTestId('video-tile')
    fireEvent.doubleClick(main)
    expect(main.dataset.zoomed).toBe('true')
    expect(within(main).getByTestId('video-zoom-percent').textContent).toBe('200%')
  })

  it('Back to chat closes the call window and hands the call back: the stage opens where it came from, with the same share in the middle', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    fourPeopleAndAShare([fakeTrack('9', { source: 'screen', track: 'TR_S9' })])
    await expandFromTile()
    // Full screen here, Linus's share picked for the middle, then Pop out.
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.click(screen.getByTestId('call-share-pick'))
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    fireEvent.click(screen.getByTestId('call-expanded-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    // Floating here meanwhile: the hand-back opens the stage all the same.
    act(() => floatCall())
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

    // Back here as the stage, with Ada's share.
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(useCallPlace.getState().choice).toBe('stage')
    expect(useCallShare.getState().picked).toBe('TR_S7')
    expect(localStorage.getItem(CALL_HAND_BACK_KEY)).toBeNull()
    expect(JSON.parse(localStorage.getItem(CALL_SHARE_PICK_KEY) ?? '{}').key).toBe('TR_S7')
    fireEvent.click(inStage('video-stage-fullscreen'))
    expect(within(screen.getByTestId('call-expanded')).getByTestId('call-share-layout').dataset.main).toBe('TR_S7')
    // The call itself never moved.
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls.length ?? 0).toBe(before)
  })

  it('a hand-back meant for another window, or a stale one, leaves this window as it was', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    const callId = voice.store.getState().call?.callId ?? ''
    fireEvent.click(inTile('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to: 'window-2', at: Date.now() }))
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(useCallPlace.getState().choice).toBe('auto')
    // Still there for the window it's for.
    expect(localStorage.getItem(CALL_HAND_BACK_KEY)).toContain('window-2')
    fireEvent.click(inTile('call-tile-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(2))
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to: 'main', at: Date.now() - 60_000 }))
    p.emit(false)
    expect(await screen.findByTestId('call-tile')).toBeTruthy()
    expect(useCallPlace.getState().choice).toBe('auto')
  })

  it("an audio call pops out too: its window says nobody has video on", async () => {
    await inCall()
    act(() => useCallPopout.setState({ open: true }))
    render(<CallWindow closeWindow={() => {}} />)
    expect(within(screen.getByTestId('call-window')).getByTestId('call-window-no-video').textContent).toBe('No one has video on.')
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

describe('the tile moves: dragged, it snaps to the nearest of four corners', () => {
  // jsdom has no layout: the window is 1024 x 768, the tile CALL_TILE_WIDTH
  // wide and (measured) 0 high.
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

  beforeEach(() => {
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
  })

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

  it('starts in the bottom right, lifts while held, and a slow drag lands in the nearest corner, kept for this client', async () => {
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
    expect(localStorage.getItem(CORNER_KEY)).toBe('top-left')
    // The drag's click is not a click on the tile: it never expands it.
    fireEvent.click(picture)
    await act(async () => new Promise((r) => setTimeout(r, DOUBLE_CLICK_MS + 20)))
    expect(useCallPlace.getState().choice).toBe('auto')
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

  it('the call bar under the picture drags the tile too', async () => {
    await inCall()
    const tile = await cameraOn()
    drag(inTile('call-control-bar'), [900, 740], [[880, 740], [860, 740], [840, 740]], 10)
    expect(tile.dataset.corner).toBe('bottom-left')
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
      expect(tile.style.left).toBe(`${1024 - 300 - 12 - CALL_TILE_WIDTH}px`)
      // The drawer closes: once its slide is over, the tile goes back to the window's corner.
      width = 0
      act(() => usePanelsStore.setState({ rightPanelOpen: false }))
      await waitFor(() => expect(tile.style.left).toBe(`${1024 - 12 - CALL_TILE_WIDTH}px`), { timeout: 2000 })
      // It opens again: the tile moves out of its way.
      width = 300
      for (let i = 0; i < 120; i++) step(16)
      act(() => usePanelsStore.setState({ rightPanelOpen: true }))
      await waitFor(() => expect(tile.style.left).toBe(`${1024 - 300 - 12 - CALL_TILE_WIDTH}px`), { timeout: 2000 })
    } finally {
      drawer.remove()
      act(() => usePanelsStore.setState({ rightPanelOpen: false }))
    }
  })

  // The app zoom (CSS zoom on <html>): rects, the window size and the
  // pointer are window px; the tile's left/top and translate are CSS px,
  // which the zoom scales.
  describe('under the app zoom (150 %)', () => {
    const Z = 1.5
    afterEach(() => resetAppZoomForTest())

    it("rests in the window's bottom-right corner, inside the window", async () => {
      applyAppZoomForTest(Z)
      await inCall()
      const tile = await cameraOn()
      expect(tile.dataset.corner).toBe('bottom-right')
      const left = Number.parseFloat(tile.style.left)
      expect(Math.abs(left - (1024 / Z - 12 - CALL_TILE_WIDTH))).toBeLessThan(0.5)
      // On screen: the right edge 12 CSS px in from the window's.
      expect(Math.abs((left + CALL_TILE_WIDTH) * Z - (1024 - 12 * Z))).toBeLessThan(1)
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

  it('the corner it last used comes back for this client, in the next call and after video stops and starts', async () => {
    localStorage.setItem(CORNER_KEY, 'top-right')
    await inCall()
    const tile = await cameraOn()
    expect(tile.dataset.corner).toBe('top-right')
    drag(screen.getByTestId('call-tile-picture'), [900, 100], [[880, 100], [860, 100], [840, 100]], 10)
    expect(tile.dataset.corner).toBe('top-left')
    await act(async () => video.setCamera(false))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect((await cameraOn()).dataset.corner).toBe('top-left')
  })
})

describe('the tile never covers the message box', () => {
  // jsdom has no layout: the window is 1024 x 768; the tile is given its
  // height here, and the message box its place.
  const TILE_HEIGHT = 240
  beforeEach(() => {
    vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockImplementation(function (this: HTMLElement) {
      return this.dataset.testid === 'call-tile' ? TILE_HEIGHT : 0
    })
  })

  /** A Home chat message box (the Thread's composer look), centred in the
   *  column, nowhere under the tile's corner. */
  function messageBox(top: number): { el: HTMLElement; rect: { left: number; top: number; width: number; height: number } } {
    const rect = { left: 200, top, width: 400, height: 96 }
    const el = document.createElement('div')
    el.dataset.zenCompose = ''
    el.getBoundingClientRect = () => ({ ...rect, right: rect.left + rect.width, bottom: rect.top + rect.height, x: rect.left, y: rect.top, toJSON: () => ({}) }) as DOMRect
    document.body.appendChild(el)
    return { el, rect }
  }

  function tileRect(tile: HTMLElement): { left: number; top: number; width: number; height: number } {
    return { left: Number.parseFloat(tile.style.left), top: Number.parseFloat(tile.style.top), width: CALL_TILE_WIDTH, height: TILE_HEIGHT }
  }

  function overlaps(a: { left: number; top: number; width: number; height: number }, b: { left: number; top: number; width: number; height: number }): boolean {
    return a.left < b.left + b.width && b.left < a.left + a.width && a.top < b.top + b.height && b.top < a.top + a.height
  }

  it('its bottom corners sit above a message box on screen, by its height plus the gap, so Send and the newest message stay clear', async () => {
    const box = messageBox(768 - 12 - 96)
    try {
      await inCall()
      const tile = await cameraOn()
      expect(tile.dataset.corner).toBe('bottom-right')
      const r = tileRect(tile)
      expect(r.top + r.height).toBe(box.rect.top - 8)
      expect(overlaps(r, box.rect)).toBe(false)
      // Dragged to the other bottom corner: above it there too.
      fireEvent.pointerDown(screen.getByTestId('call-tile-picture'), { clientX: 900, clientY: 600, pointerId: 1, button: 0 })
      fireEvent.pointerMove(screen.getByTestId('call-tile-picture'), { clientX: 100, clientY: 600, pointerId: 1 })
      fireEvent.pointerUp(screen.getByTestId('call-tile-picture'), { clientX: 100, clientY: 600, pointerId: 1 })
      expect(tile.dataset.corner).toBe('bottom-left')
      expect(tileRect(tile).top + TILE_HEIGHT).toBe(box.rect.top - 8)
    } finally {
      box.el.remove()
    }
  })

  it('says where it rests, so a message list under it can make room: a bottom corner counts, a top one does not, and none once it goes', async () => {
    await inCall()
    const tile = await cameraOn()
    expect(useCallTilePlace.getState().rest).toEqual({ left: Number.parseFloat(tile.style.left), top: 768 - 12 - TILE_HEIGHT, width: CALL_TILE_WIDTH, height: TILE_HEIGHT, bottom: true })
    const picture = screen.getByTestId('call-tile-picture')
    fireEvent.pointerDown(picture, { clientX: 900, clientY: 600, pointerId: 1, button: 0 })
    fireEvent.pointerMove(picture, { clientX: 900, clientY: 100, pointerId: 1 })
    fireEvent.pointerUp(picture, { clientX: 900, clientY: 100, pointerId: 1 })
    expect(tile.dataset.corner).toBe('top-right')
    expect(useCallTilePlace.getState().rest?.bottom).toBe(false)
    await act(async () => video.setCamera(false))
    await waitFor(() => expect(screen.queryByTestId('call-tile')).toBeNull())
    expect(useCallTilePlace.getState().rest).toBeNull()
  })

  it('a message box that comes on screen later moves the tile up out of its way', async () => {
    await inCall()
    const tile = await cameraOn()
    expect(Number.parseFloat(tile.style.top) + TILE_HEIGHT).toBe(768 - 12)
    const box = messageBox(600)
    try {
      await waitFor(() => expect(tileRect(tile).top + TILE_HEIGHT).toBe(600 - 8), { timeout: 2500 })
      expect(overlaps(tileRect(tile), box.rect)).toBe(false)
    } finally {
      box.el.remove()
    }
  })
})
