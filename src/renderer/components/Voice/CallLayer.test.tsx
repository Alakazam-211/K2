// @vitest-environment jsdom
//
// The call's controls and its stage: a voice-only call has Mute, Deafen,
// Camera and Hang up in the top bar; with video on (a camera or a screen
// share) the stage shows the call's tiles with its own controls, and the top
// bar's controls give way. The stage's header has Pop out, Full screen and
// Compact; Compact folds the stage into the top bar's slim bar, whose Expand
// brings it back. There is no floating tile, no corner, no Bigger and no
// Hide. Real voice and video controllers over the fake engines.

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
import { __resetCallExpandForTests, useCallExpand } from '@/stores/call-expand'
import { __resetCallShareForTests, useCallShare } from '@/stores/call-share'
import { __resetCallPopoutForTests, setCallPopoutBridge, useCallPopout, type CallPopoutBridge } from '@/stores/call-popout'
import { CALL_EXPAND_MARGIN, CALL_HAND_BACK_KEY, CALL_POPPED_FROM_KEY, CALL_SHARE_PICK_KEY } from '@/lib/voice/call-view'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { voice } from '@/stores/voice'
import { video } from '@/stores/video'
import { __resetCallPlaceForTests, useCallPlace } from '@/stores/call-place'
import { fakeTrack } from '@/lib/voice/fake-video-engine'
import { overlayClaims, resetBrowserOverlaysForTest } from '@/lib/browser-overlay'

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

/** Camera on, through the top bar's button: the stage comes up. */
async function cameraOn(): Promise<HTMLElement> {
  fireEvent.click(await screen.findByTestId('call-topbar-camera'))
  return screen.findByTestId('video-stage')
}

/** A control inside the stage. */
function inStage(testId: string): HTMLElement {
  return within(screen.getByTestId('video-stage')).getByTestId(testId)
}

beforeEach(async () => {
  h.videoPreview = true
  h.settingsOpen = false
  h.videoOpts = {}
  localStorage.clear()
  await act(async () => {
    if (voice.store.getState().call) await voice.leave()
    voice.store.setState({ rooms: {}, notice: null, popoverOpen: false, engineKind: 'seat' })
  })
  __resetCallPlaceForTests()
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

describe('voice only: the controls are in the top bar, and there is no stage', () => {
  it('shows Mute, Deafen, Noise removal, Camera and Leave call beside the headset', async () => {
    await inCall()
    const bar = await screen.findByTestId('call-topbar-controls')
    expect(bar.querySelectorAll('button')).toHaveLength(5)
    expect(screen.getByTestId('call-topbar-noise').getAttribute('aria-label')).toBe('Noise removal on')
    expect(screen.getByTestId('call-topbar-mute').getAttribute('aria-label')).toBe('Mute')
    expect(screen.getByTestId('call-topbar-deafen').getAttribute('aria-label')).toBe('Deafen')
    expect(screen.getByTestId('call-topbar-camera').getAttribute('aria-label')).toBe('Turn camera on')
    expect(screen.getByTestId('call-topbar-leave').getAttribute('aria-label')).toBe('Leave call')
    expect(screen.queryByTestId('video-stage')).toBeNull()
    // Nothing to expand without video.
    expect(screen.queryByTestId('call-bar-expand')).toBeNull()
  })

  it('with video rooms off: no Camera, and still no stage', async () => {
    h.videoPreview = false
    await inCall()
    expect(screen.getByTestId('call-topbar-controls').querySelectorAll('button')).toHaveLength(4)
    expect(screen.queryByTestId('call-topbar-camera')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
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

  it('a seat-only call (the web client: no audio here) shows no controls and no stage', async () => {
    await inCall()
    act(() => voice.store.setState({ engineKind: 'seat' }))
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    expect(screen.queryByTestId('video-stage')).toBeNull()
  })
})

describe('video on: the stage, and the top bar gives way', () => {
  it('your camera brings up the stage with your self-view and its controls; stopping it takes the stage away', async () => {
    await inCall()
    const stage = await cameraOn()
    expect(within(stage).getByTestId('video-tile').dataset.track).toBe('local:camera')
    expect(screen.queryByTestId('call-topbar-controls')).toBeNull()
    expect(inStage('video-stage-mute')).toBeTruthy()
    expect(inStage('video-stage-leave')).toBeTruthy()
    // No floating tile anywhere.
    expect(screen.queryByTestId('call-tile')).toBeNull()
    fireEvent.click(inStage('video-camera'))
    await waitFor(() => expect(screen.queryByTestId('video-stage')).toBeNull())
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
  })

  it("someone else's screen share shows the same way (the fake engine's share track)", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7', { source: 'screen', track: 'TR_S7' })]))
    const stage = await screen.findByTestId('video-stage')
    expect(within(stage).getByTestId('video-tile').dataset.track).toBe('TR_S7')
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['TR_S7']))
    act(() => h.videoEngine?.setTracks([]))
    await waitFor(() => expect(screen.queryByTestId('video-stage')).toBeNull())
  })

  it("the stage's header: Pop out, Full screen and Compact; no Bigger, no Hide, no corner", async () => {
    await inCall()
    await cameraOn()
    expect(screen.queryByTestId('video-stage-size')).toBeNull()
    expect(screen.queryByTestId('video-stage-hide')).toBeNull()
    expect(screen.queryByText('Bigger')).toBeNull()
    expect(screen.queryByText('Hide')).toBeNull()
    const full = inStage('video-stage-fullscreen')
    expect(full.getAttribute('aria-label')).toMatch(/^Full screen \(.+\)$/)
    expect(full.getAttribute('title')).toBe(full.getAttribute('aria-label'))
    const compact = inStage('video-stage-compact')
    expect(compact.textContent).toBe('Compact')
    expect(screen.getByTestId('video-stage').dataset.corner).toBeUndefined()
  })

  it("Compact folds the stage into the top bar's slim bar, with no video drawn; Expand brings it back", async () => {
    await inCall()
    await cameraOn()
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['local:camera']))
    fireEvent.click(inStage('video-stage-compact'))
    expect(useCallPlace.getState().compact).toBe(true)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    // Nothing draws it here any more (sinks close after their grace).
    await waitFor(() => expect(video.sinks()).toEqual([]), { timeout: 3500 })
    const expand = screen.getByTestId('call-bar-expand')
    expect(expand.textContent).toBe('Expand')
    fireEvent.click(expand)
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(screen.queryByTestId('call-bar-expand')).toBeNull()
  })

  it("the stage's buttons drive the engine, and Leave call takes the stage with the call", async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(inStage('video-stage-mute'))
    expect(h.voiceEngine?.calls).toContain('muted true')
    fireEvent.click(inStage('video-stage-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(screen.queryByTestId('video-stage')).toBeNull()
  })

  it('Settings open: the stage steps back and the top bar has the controls', async () => {
    await inCall()
    await cameraOn()
    h.settingsOpen = true
    // The settings mock isn't a store: re-render through the call's.
    act(() => {
      const c = voice.store.getState().call
      if (!c) throw new Error('no call')
      voice.store.setState({ call: { ...c } })
    })
    await waitFor(() => expect(screen.queryByTestId('video-stage')).toBeNull())
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
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
    fireEvent.click(inStage('video-camera'))
    await waitFor(() => expect(screen.queryByTestId('video-stage')).toBeNull())
    // Denied now (System Settings): the press asks nothing and says so.
    if (!h.videoEngine) throw new Error('no video engine')
    h.videoEngine.permission = 'denied'
    fireEvent.click(await screen.findByTestId('call-topbar-camera'))
    await waitFor(() => expect(video.store.getState().notice).toMatchObject({ settings: 'camera' }))
    expect(h.videoEngine.calls.slice(-1)).toEqual(['camera_permission'])
    expect(screen.queryByTestId('video-stage')).toBeNull()
  })

  it("Share screen opens the OS picker where there is one, and the stage shows the share's self-view", async () => {
    h.videoOpts = { caps: { screen: { supported: true, systemPicker: true, list: true, windows: true } } }
    await inCall()
    // The top bar has it too, where the OS has a picker.
    fireEvent.click(await screen.findByTestId('call-topbar-screen'))
    const stage = await screen.findByTestId('video-stage')
    expect(h.videoEngine?.calls).toContain('screen {"on":true}')
    expect(within(stage).getByTestId('video-tile').dataset.track).toBe('local:screen')
    await waitFor(() => expect(video.sinks().map((s) => s.track)).toEqual(['local:screen']))
    fireEvent.click(inStage('video-screen'))
    await waitFor(() => expect(screen.queryByTestId('video-stage')).toBeNull())
    expect(h.videoEngine?.calls).toContain('screen {"on":false}')
  })

  it("where the OS only lists, the stage's Choose screen shows the list under it, in K2's own menu", async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(inStage('video-screen-pick'))
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
  it("every tile on the stage carries its name on the picture (the self-view's too)", async () => {
    await inCall()
    act(() => h.videoEngine?.setTracks([fakeTrack('7')]))
    await screen.findByTestId('video-stage')
    fireEvent.click(inStage('video-camera'))
    await waitFor(() => expect(within(screen.getByTestId('video-grid')).getAllByTestId('video-tile-label')).toHaveLength(2))
    for (const l of within(screen.getByTestId('video-grid')).getAllByTestId('video-tile-label')) {
      expect(l.className).toContain('bg-[var(--color-media-label-bg)]')
    }
  })
})

describe('full screen: the call over K2, inside the app', () => {
  it("the stage's Full screen opens the call over K2 with a margin, never OS full screen; Exit full screen puts the stage back", async () => {
    installFullscreenApi()
    await inCall()
    await cameraOn()
    const full = inStage('video-stage-fullscreen')
    full.focus()
    fireEvent.click(full)
    expect(useCallExpand.getState().expanded).toBe(true)
    const view = screen.getByTestId('call-expanded')
    expect(view.getAttribute('role')).toBe('dialog')
    expect(view.getAttribute('aria-modal')).toBe('true')
    expect(view.getAttribute('aria-label')).toBe('Standup, full screen')
    // A margin all round, inside the window; over the scrim.
    expect(view.style.width).toBe(`calc(var(--k2-vw) - ${CALL_EXPAND_MARGIN * 2}px)`)
    expect(view.style.height).toBe(`calc(var(--k2-vh) - ${CALL_EXPAND_MARGIN * 2}px)`)
    expect(screen.getByTestId('call-expanded-scrim').dataset.scrim).toBe('')
    expect(fsCalls).toEqual([])
    // The stage gives way; the video is drawn once, in the view.
    expect(screen.queryByTestId('video-stage')).toBeNull()
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
    await cameraOn()
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
    await cameraOn()
    expect(document.activeElement).toBe(document.body)
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(document.activeElement).toBe(inStage('video-stage-fullscreen'))
  })

  it('Tab stays inside the view', async () => {
    await inCall()
    await cameraOn()
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
    await cameraOn()
    fireEvent.click(inStage('video-stage-fullscreen'))
    const bar = screen.getByTestId('call-expanded-controls')
    expect(bar.tagName).toBe('FOOTER')
    expect(screen.getByTestId('call-expanded').lastElementChild).toBe(bar)
    // Mute, Deafen, Noise removal, Camera, Share screen, Leave call.
    expect(within(bar).getByTestId('call-overlay-controls').querySelectorAll('button')).toHaveLength(6)
    fireEvent.click(within(bar).getByTestId('call-overlay-mute'))
    expect(voice.store.getState().call?.muted).toBe(true)
    expect(screen.getByTestId('call-expanded-duration').textContent).toMatch(/^\d+:\d\d$/)
  })

  it('the chord toggles it (⌃⌘V on macOS, Ctrl+Alt+V elsewhere)', async () => {
    installFullscreenApi()
    await inCall()
    await cameraOn()
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
    await cameraOn()
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.click(screen.getByTestId('call-overlay-leave'))
    await waitFor(() => expect(voice.store.getState().call).toBeNull())
    expect(useCallExpand.getState().expanded).toBe(false)
    expect(screen.queryByTestId('call-expanded')).toBeNull()
  })

  it('from Compact, the chord opens full screen, and exiting goes back to the slim bar', async () => {
    await inCall()
    await cameraOn()
    fireEvent.click(inStage('video-stage-compact'))
    fireEvent.keyDown(window, fullScreenChord())
    expect(useCallExpand.getState().expanded).toBe(true)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(screen.queryByTestId('video-stage')).toBeNull()
    expect(screen.getByTestId('call-bar-expand')).toBeTruthy()
  })

  it('claims the window over a live Browser page while open', async () => {
    await inCall()
    await cameraOn()
    const before = overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length
    fireEvent.click(inStage('video-stage-fullscreen'))
    expect(overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length).toBe(before + 1)
    fireEvent.keyDown(window, { key: 'Escape' })
    expect(overlayClaims().filter((c) => c.area === 'fullscreen' && c.kind === 'modal').length).toBe(before)
  })
})

describe('a screen share in the middle, the people on the left', () => {
  /** The share is up: open full screen from the stage. */
  async function fullScreen(): Promise<void> {
    await screen.findByTestId('video-stage')
    fireEvent.click(inStage('video-stage-fullscreen'))
  }

  it('full screen: the share takes the middle, and everyone is a smaller tile in the column, with names, speaking and muted', async () => {
    await inCall()
    fourPeopleAndAShare()
    await fullScreen()
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
    fireEvent.click(pick)
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    expect(screen.getByTestId('call-share-pick').dataset.track).toBe('TR_S7')
    // The pick sticks while that share lasts; when it stops, the other takes the middle.
    act(() => h.videoEngine?.setTracks([fakeTrack('7', { source: 'screen', track: 'TR_S7' }), fakeTrack('8')]))
    await waitFor(() => expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S7'))
    expect(screen.queryByTestId('call-share-pick')).toBeNull()
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

  it('Pop out takes the stage from here without touching the call; closing the window brings it back', async () => {
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    const before = h.voiceEngine?.calls.length ?? 0
    const pop = inStage('video-stage-popout')
    expect(pop.getAttribute('aria-label')).toBe('Pop out into its own window')
    fireEvent.click(pop)
    await waitFor(() => expect(p.opened).toHaveLength(1))
    expect(useCallPopout.getState().open).toBe(true)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    // The call is untouched: still connected, nothing sent to the engine.
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls.length ?? 0).toBe(before)
    // While it's out, this window's top bar has the call's controls.
    expect(screen.getByTestId('call-topbar-controls')).toBeTruthy()
    // The window closes (its close button, ⌘W): the stage is back; the call never dropped.
    p.emit(false)
    expect(await screen.findByTestId('video-stage')).toBeTruthy()
    expect(voice.store.getState().call?.phase).toBe('connected')
    expect(h.voiceEngine?.calls).not.toContain('disconnect')
  })

  it('Pop out from full screen exits it first, and notes which window it came from', async () => {
    installFullscreenApi()
    const p = fakePopout()
    setCallPopoutBridge(p.bridge)
    await inCall()
    await cameraOn()
    fireEvent.click(inStage('video-stage-fullscreen'))
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
    expect(screen.queryByTestId('video-stage-popout')).toBeNull()
    expect(inStage('video-stage-fullscreen')).toBeTruthy()
  })

  it("the call's window draws the call and its controls, says Back to chat, and closes itself when the call ends", async () => {
    await inCall()
    await cameraOn()
    const closeWindow = vi.fn()
    // The room's window has popped it out: its stage has given way.
    act(() => useCallPopout.setState({ open: true }))
    render(<CallWindow closeWindow={closeWindow} />)
    const win = screen.getByTestId('call-window')
    expect(within(win).getByTestId('call-window-title').textContent).toBe('Standup')
    expect(within(win).getByTestId('video-grid')).toBeTruthy()
    expect(within(win).getByTestId('call-window-duration').textContent).toMatch(/^\d+:\d\d$/)
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

  it("the call's window puts a share in the middle and the people on the left", async () => {
    await inCall()
    fourPeopleAndAShare()
    await screen.findByTestId('video-stage')
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
    await screen.findByTestId('video-stage')
    // Full screen here, Linus's share picked for the middle, then Pop out.
    fireEvent.click(inStage('video-stage-fullscreen'))
    fireEvent.click(screen.getByTestId('call-share-pick'))
    expect(screen.getByTestId('call-share-layout').dataset.main).toBe('TR_S9')
    fireEvent.click(screen.getByTestId('call-expanded-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    // Folded here meanwhile: the hand-back opens the stage all the same.
    act(() => useCallPlace.setState({ compact: true }))
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
    expect(useCallPlace.getState().compact).toBe(false)
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
    fireEvent.click(inStage('video-stage-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(1))
    act(() => useCallPlace.setState({ compact: true }))
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to: 'window-2', at: Date.now() }))
    p.emit(false)
    expect(await screen.findByTestId('call-bar-expand')).toBeTruthy()
    expect(useCallPlace.getState().compact).toBe(true)
    // Still there for the window it's for.
    expect(localStorage.getItem(CALL_HAND_BACK_KEY)).toContain('window-2')
    act(() => useCallPlace.setState({ compact: false }))
    fireEvent.click(inStage('video-stage-popout'))
    await waitFor(() => expect(p.opened).toHaveLength(2))
    act(() => useCallPlace.setState({ compact: true }))
    localStorage.setItem(CALL_HAND_BACK_KEY, JSON.stringify({ callId, to: 'main', at: Date.now() - 60_000 }))
    p.emit(false)
    expect(await screen.findByTestId('call-bar-expand')).toBeTruthy()
    expect(useCallPlace.getState().compact).toBe(true)
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
