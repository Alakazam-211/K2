// @vitest-environment jsdom
//
// Video in voice rooms: the tiles (names, mic, camera off, paused, the
// speaker's emphasis, the mirrored self-view) and the call's video controls
// (camera, the screen picker, refusals, the camera picker), over the fake engine.

import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { FakeVideoEngine, fakeTrack } from '@/lib/voice/fake-video-engine'
import { FakeVideoDecoder, fakeWebCodecs, RecordingSurface } from '@/lib/voice/fake-webcodecs'
import type { CallPerson } from '@/lib/voice/video-labels'
import { createVideoController } from '@/stores/video-core'
import VideoStage, { gridColumns } from './VideoStage'
import { CameraPicker, ScreenPicker, VideoCallButtons, VideoNoticeLine } from './VideoControls'

const people: CallPerson[] = [
  { memberId: 'me', isSelf: true, user: 'You', speaking: false, muted: false },
  { memberId: '7', user: 'Ada', speaking: false, muted: true },
  { memberId: '8', user: 'Grace', speaking: false, muted: false },
  { memberId: '9', user: 'Linus', speaking: false, muted: false },
]

function setup(tracks = [fakeTrack('7'), fakeTrack('8', { muted: true }), fakeTrack('9', { paused: 'background' })]) {
  const engine = new FakeVideoEngine()
  engine.tracks = tracks
  const privacy: string[] = []
  const ctl = createVideoController({
    loadEngine: async () => engine,
    webCodecs: fakeWebCodecs,
    watchBox: (_el, onBox) => {
      onBox({ width: 160, height: 90, visible: true })
      return () => undefined
    },
    devicePixelRatio: () => 1,
    surfaceFor: () => new RecordingSurface(),
    loadPrefs: () => ({ cameraId: null }),
    savePrefs: () => undefined,
    toast: () => undefined,
    openPrivacySettings: async (pane) => {
      privacy.push(pane)
      return true
    },
    now: () => Date.now(),
  })
  return { engine, ctl, privacy }
}

async function ready(s: ReturnType<typeof setup>): Promise<void> {
  await act(async () => {
    s.ctl.setActive({ enabled: true, inCall: true })
    await new Promise((r) => setTimeout(r, 0))
  })
  await waitFor(() => expect(s.ctl.store.getState().availability).toBe('ready'))
}

beforeEach(() => FakeVideoDecoder.reset())
afterEach(() => cleanup())

describe('the tiles', () => {
  it('draws one tile per track with the name, the mic and why there is no picture', async () => {
    const s = setup()
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    const tiles = await screen.findAllByTestId('video-tile')
    expect(tiles.map((t) => [t.dataset.track, t.dataset.state])).toEqual([
      ['TR_7', 'live'],
      ['TR_8', 'off'],
      ['TR_9', 'paused'],
    ])
    expect(tiles.map((t) => within(t).getByTestId('video-tile-name').textContent)).toEqual(['Ada', 'Grace', 'Linus'])
    expect(within(tiles[0]).queryByTestId('video-tile-muted')).not.toBeNull()
    expect(within(tiles[1]).queryByTestId('video-tile-muted')).toBeNull()
    expect(within(tiles[1]).getByTestId('video-tile-line').textContent).toBe('Camera off')
    expect(within(tiles[2]).getByTestId('video-tile-line').textContent).toBe('Camera paused (in the background)')
    // Live and paused tracks have sinks (paused ones held); a camera that's off has none.
    await waitFor(() => expect(s.ctl.sinks().map((x) => x.track).sort()).toEqual(['TR_7', 'TR_9']))
  })

  it('draws a screen share large and first', async () => {
    const s = setup([fakeTrack('7'), fakeTrack('8', { source: 'screen', track: 'TR_S8' })])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    const tiles = await screen.findAllByTestId('video-tile')
    expect(tiles.map((t) => [t.dataset.track, t.dataset.large])).toEqual([
      ['TR_S8', 'true'],
      ['TR_7', 'false'],
    ])
    expect(within(tiles[0]).getByTestId('video-tile-name').textContent).toBe("Grace's screen")
  })

  it('outlines the speaker, and with 3 or more tiles draws the last speaker large', async () => {
    const s = setup([fakeTrack('7'), fakeTrack('8'), fakeTrack('9')])
    const { rerender } = render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    const speaking = people.map((p) => (p.memberId === '9' ? { ...p, speaking: true } : p))
    rerender(<VideoStage ctl={s.ctl} people={speaking} title="Standup" />)
    await waitFor(() => expect(screen.getAllByTestId('video-tile')[0].dataset.track).toBe('TR_9'))
    const first = screen.getAllByTestId('video-tile')[0]
    expect(first.dataset.large).toBe('true')
    expect(first.dataset.speaking).toBe('true')
    // Silence keeps the last speaker large.
    rerender(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    expect(screen.getAllByTestId('video-tile')[0]).toMatchObject({ dataset: { track: 'TR_9', large: 'true', speaking: 'false' } })
  })

  it('shows your camera as a mirrored self-view, and hides to a pill', async () => {
    const s = setup([])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    await act(async () => s.ctl.setCamera(true))
    const self = await screen.findByTestId('video-tile')
    expect(self.dataset).toMatchObject({ track: 'local:camera', self: 'true' })
    expect((within(self).getByTestId('video-canvas') as HTMLCanvasElement).style.transform).toBe('scaleX(-1)')
    fireEvent.click(screen.getByTestId('video-stage-hide'))
    expect(screen.queryByTestId('video-grid')).toBeNull()
    fireEvent.click(screen.getByTestId('video-stage-show'))
    expect(screen.getByTestId('video-grid')).not.toBeNull()
  })

  it('picks columns for the tile count', () => {
    expect([1, 2, 4, 5, 9, 10].map(gridColumns)).toEqual([1, 2, 2, 3, 3, 4])
  })
})

describe('the controls', () => {
  function Controls({ s }: { s: ReturnType<typeof setup> }): React.JSX.Element {
    return (
      <div>
        <VideoCallButtons ctl={s.ctl} />
        <ScreenPicker ctl={s.ctl} />
        <VideoNoticeLine ctl={s.ctl} />
        <CameraPicker ctl={s.ctl} />
      </div>
    )
  }

  it('turns the camera on and off', async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    fireEvent.click(await screen.findByTestId('video-camera'))
    await waitFor(() => expect(screen.getByTestId('video-camera').getAttribute('aria-label')).toBe('Turn camera off'))
    fireEvent.click(screen.getByTestId('video-camera'))
    await waitFor(() => expect(screen.getByTestId('video-camera').getAttribute('aria-label')).toBe('Turn camera on'))
    // The permission is read first; it's granted, so nothing is asked.
    expect(s.engine.calls.filter((c) => c.startsWith('camera'))).toEqual(['camera_permission', 'camera {"on":true}', 'camera {"on":false}'])
    expect(screen.queryByTestId('video-notice')).toBeNull()
  })

  it('Share screen opens the OS picker (no source), and stops', async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    fireEvent.click(await screen.findByTestId('video-screen'))
    await waitFor(() => expect(screen.getByTestId('video-screen').getAttribute('aria-label')).toBe('Stop sharing'))
    expect(s.engine.calls).toContain('screen {"on":true}')
    expect(s.engine.calls).not.toContain('screen_sources')
    // Sharing: no list to pick from.
    expect(screen.queryByTestId('video-screen-pick')).toBeNull()
    fireEvent.click(screen.getByTestId('video-screen'))
    await waitFor(() => expect(screen.getByTestId('video-screen').getAttribute('aria-label')).toBe('Share screen'))
  })

  it('where the OS lists what can be shared, Choose screen offers the list, and a pick shares that source', async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    const choose = await screen.findByTestId('video-screen-pick')
    expect(choose.getAttribute('aria-label')).toBe('Choose screen')
    expect(choose.hasAttribute('aria-pressed')).toBe(false)
    expect(choose.querySelector('svg')!.getAttribute('data-k2-icon')).toBe('chevron-down')
    fireEvent.click(choose)
    fireEvent.click(await screen.findByTestId('video-screen-source'))
    const menu = screen.getByTestId('setting-dropdown-menu')
    expect(menu.textContent).toContain('Screens')
    expect(menu.textContent).toContain('Quarterly plan · Acme Docs')
    const pick = Array.from(menu.querySelectorAll('button')).find((b) => b.textContent?.includes('Quarterly plan'))
    fireEvent.click(pick as HTMLButtonElement)
    await waitFor(() => expect(screen.getByTestId('video-screen').getAttribute('aria-label')).toBe('Stop sharing'))
    expect(screen.queryByTestId('video-screen-picker')).toBeNull()
    expect(s.engine.calls).toContain('screen {"on":true,"source":"window:42"}')
  })

  it('offers no Pick where the OS has no list', async () => {
    const s = setup([])
    s.engine.caps = async () => ({
      camera: true,
      screen: { supported: true, systemPicker: true, list: false, windows: true },
      publishCodecs: ['vp8'],
      decodeCodecs: ['vp8'],
      receive: true,
      selfView: true,
      liveSwitch: true,
      backend: 'fake',
    })
    render(<Controls s={s} />)
    await ready(s)
    await screen.findByTestId('video-screen')
    expect(screen.queryByTestId('video-screen-pick')).toBeNull()
  })

  it("greys out what the room's policy forbids, and says why", async () => {
    const s = setup([])
    s.engine.snapshot = async () => ({ tracks: [], local: { camera: 'off', screen: 'off', reason: null }, policy: { videoEnabled: true, cameraAllowed: false, screenAllowed: true } })
    render(<Controls s={s} />)
    await ready(s)
    const camera = (await screen.findByTestId('video-camera')) as HTMLButtonElement
    await waitFor(() => expect(camera.disabled).toBe(true))
    expect(camera.title).toBe("Video isn't allowed for you in this room. A room admin can change that.")
    expect((screen.getByTestId('video-screen') as HTMLButtonElement).disabled).toBe(false)
    expect(screen.getByTestId('video-notice').textContent).toBe("Video isn't allowed for you in this room. A room admin can change that.")
    act(() => s.engine.setPolicy({ videoEnabled: false, cameraAllowed: true, screenAllowed: true }))
    await waitFor(() => expect((screen.getByTestId('video-screen') as HTMLButtonElement).disabled).toBe(true))
    expect(screen.getByTestId('video-screen').title).toBe('Video is turned off for this room.')
  })

  it('explains a refusal, with the way to the privacy settings when the OS said no', async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    s.engine.refuseNext('camera', 'video_slots_full')
    fireEvent.click(await screen.findByTestId('video-camera'))
    await waitFor(() => expect(screen.getByTestId('video-notice').textContent).toMatch(/^25 cameras are already on in this room/))
    expect(screen.queryByTestId('video-privacy-settings')).toBeNull()
    s.engine.refuseNext('camera', 'camera_denied')
    fireEvent.click(screen.getByTestId('video-camera'))
    await waitFor(() => expect(screen.getByTestId('video-notice').textContent).toMatch(/isn't allowed to use your camera/))
    fireEvent.click(screen.getByTestId('video-privacy-settings'))
    await waitFor(() => expect(s.privacy).toEqual(['camera']))
  })

  it('says plainly when your camera disconnects, or sends no picture', async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    fireEvent.click(await screen.findByTestId('video-camera'))
    await waitFor(() => expect(screen.getByTestId('video-camera').getAttribute('aria-label')).toBe('Turn camera off'))
    act(() => s.engine.setLocal({ camera: 'off', screen: 'off', reason: 'camera_lost' }))
    await waitFor(() => expect(screen.getByTestId('video-notice').textContent).toBe('Your camera disconnected.'))
    expect(screen.getByTestId('video-notice').dataset.kind).toBe('error')
    expect(screen.getByTestId('video-camera').getAttribute('aria-label')).toBe('Turn camera on')
    fireEvent.click(screen.getByTestId('video-camera'))
    await waitFor(() => expect(screen.getByTestId('video-camera').getAttribute('aria-label')).toBe('Turn camera off'))
    act(() => s.engine.setLocal({ camera: 'off', screen: 'off', reason: 'no_frames' }))
    await waitFor(() => expect(screen.getByTestId('video-notice').textContent).toBe("Your camera isn't sending a picture. Is the lid closed?"))
  })

  it('says so before you try when the OS already denied the camera', async () => {
    const s = setup([])
    s.engine.devices = async () => ({ cameras: [{ id: 'cam:1', label: 'Acme HD Camera', facing: null, isDefault: true }], camera: null, permission: { camera: 'denied', screen: 'granted' } })
    render(<Controls s={s} />)
    await ready(s)
    await waitFor(() => expect(screen.getByTestId('video-notice').textContent).toBe("K2 isn't allowed to use your camera. Open privacy settings"))
  })

  it('picks a camera beside the audio devices', async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    fireEvent.click(await screen.findByTestId('video-camera-input'))
    const pick = Array.from(screen.getByTestId('setting-dropdown-menu').querySelectorAll('button')).find((b) => b.textContent === 'Globex Desk Camera')
    fireEvent.click(pick as HTMLButtonElement)
    await waitFor(() => expect(s.engine.calls).toContain('set_video_device {"camera":"cam:2"}'))
    expect(screen.getByTestId('video-camera-input').textContent).toBe('Globex Desk Camera')
  })

  it('says when this build has no video', async () => {
    const s = setup([])
    s.engine.caps = async () => {
      throw new Error('no video_caps')
    }
    render(<Controls s={s} />)
    await act(async () => {
      s.ctl.setActive({ enabled: true, inCall: true })
      await new Promise((r) => setTimeout(r, 0))
    })
    expect((await screen.findByTestId('video-unavailable')).textContent).toBe("Video isn't in this version of K2 yet.")
    expect(screen.queryByTestId('video-camera')).toBeNull()
  })
})
