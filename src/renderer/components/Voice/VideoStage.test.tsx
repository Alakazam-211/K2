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
import VideoStage, { gridColumns, gridRows, stageTileWidth } from './VideoStage'
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

  it('shows your camera as a mirrored self-view; no Bigger, no Hide, never fixed over the page', async () => {
    const s = setup([])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    expect(screen.queryByTestId('video-stage')).toBeNull()
    await act(async () => s.ctl.setCamera(true))
    const self = await screen.findByTestId('video-tile')
    expect(self.dataset).toMatchObject({ track: 'local:camera', self: 'true' })
    expect((within(self).getByTestId('video-canvas') as HTMLCanvasElement).style.transform).toBe('scaleX(-1)')
    expect(screen.queryByTestId('video-stage-hide')).toBeNull()
    expect(screen.queryByTestId('video-stage-size')).toBeNull()
    expect(screen.getByTestId('video-stage').className).not.toMatch(/\bfixed\b/)
  })

  it('takes its header actions from the place it sits in', async () => {
    const s = setup([fakeTrack('7')])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" actions={<button type="button" data-testid="a-header-action" />} />)
    await ready(s)
    const stage = await screen.findByTestId('video-stage')
    expect(within(stage.querySelector('header') as HTMLElement).getByTestId('a-header-action')).toBeTruthy()
  })

  it('a tile sits on the look\'s tile surface: rounded by the tile radius, cut in Terminal, never a black box', async () => {
    const s = setup([fakeTrack('7')])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    const tile = await screen.findByTestId('video-tile')
    expect(tile.getAttribute('data-k2-chamfer')).toBe('surface')
    expect(tile.className).toContain('rounded-[var(--radius-box)]')
    expect(tile.className).toContain('bg-[var(--tile-raised)]')
    expect(tile.className).toContain('border-[var(--tile-edge)]')
    expect(tile.className).not.toMatch(/\bbg-black\b/)
    // The picture is clipped inside the tile's own shape.
    expect(within(tile).getByTestId('video-tile-media').contains(within(tile).getByTestId('video-canvas'))).toBe(true)
  })

  it("a tile's name sits on the look's chip: a pill in Glass, a cut chip in Terminal, on the media label colours", async () => {
    const s = setup([fakeTrack('7')])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    const label = within(await screen.findByTestId('video-tile')).getByTestId('video-tile-label')
    expect(label.getAttribute('data-k2-chamfer')).toBe('chip')
    expect(label.className).toContain('rounded-[var(--radius-pill)]')
    expect(label.className).toContain('bg-[var(--color-media-label-bg)]')
    expect(label.className).toContain('text-[var(--color-media-label-text)]')
    // Off the tile's edge, not flush in its corner.
    expect(label.className).toMatch(/\bbottom-1\.5\b/)
    expect(label.className).toMatch(/\bleft-1\.5\b/)
  })

  it("with the camera off: the person's avatar and name in the middle of the tile, the muted mic on the name", async () => {
    const s = setup([fakeTrack('7', { muted: true }), fakeTrack('8', { muted: true })])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    const [ada, grace] = await screen.findAllByTestId('video-tile')
    expect(ada.dataset.state).toBe('off')
    const off = within(ada).getByTestId('video-tile-off')
    expect(off.className).toContain('bg-[var(--tile-raised)]')
    const avatar = within(off).getByTestId('video-tile-avatar')
    expect(avatar.querySelector('[data-presence-chip]')).not.toBeNull()
    // Larger than the old 36px mark.
    expect(Number(avatar.dataset.size)).toBeGreaterThanOrEqual(48)
    // The name's chip moves to the middle, under the avatar, with the mic.
    const label = within(ada).getByTestId('video-tile-label')
    expect(label.dataset.at).toBe('center')
    expect(off.contains(label)).toBe(true)
    expect(within(label).getByTestId('video-tile-name').textContent).toBe('Ada')
    expect(within(label).queryByTestId('video-tile-muted')).not.toBeNull()
    expect(within(grace).getByTestId('video-tile-label').dataset.at).toBe('center')
    expect(within(grace).queryByTestId('video-tile-muted')).toBeNull()
    // Said for screen readers, not drawn over the avatar.
    expect(within(ada).getByTestId('video-tile-line').className).toContain('sr-only')
  })

  it("rings the speaker in the accent, following the tile's radius (Glass) or its cut (Terminal)", async () => {
    const s = setup([fakeTrack('7'), fakeTrack('8')])
    const { rerender } = render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await ready(s)
    await screen.findAllByTestId('video-tile')
    expect(screen.queryByTestId('video-tile-ring')).toBeNull()
    const speaking = people.map((p) => (p.memberId === '8' ? { ...p, speaking: true } : p))
    rerender(<VideoStage ctl={s.ctl} people={speaking} title="Standup" />)
    const tile = screen.getAllByTestId('video-tile').find((t) => t.dataset.track === 'TR_8')!
    const ring = within(tile).getByTestId('video-tile-ring')
    expect(ring.className).toContain('border-[var(--color-accent)]')
    expect(ring.className).toContain('rounded-[var(--radius-box)]')
    expect(ring.getAttribute('data-k2-chamfer')).toBe('surface')
    // No square outline around the tile any more.
    expect(tile.className).not.toMatch(/\boutline-2\b/)
  })

  it('picks columns for the tile count', () => {
    expect([1, 2, 4, 5, 9, 10].map(gridColumns)).toEqual([1, 2, 2, 3, 3, 4])
  })

  it('counts the rows the tiles take', () => {
    expect([
      [1, 1],
      [2, 2],
      [3, 2],
      [6, 3],
      [7, 3],
      [10, 4],
    ].map(([n, c]) => gridRows(n, c))).toEqual([1, 1, 2, 2, 3, 3])
  })

  it('sizes a tile to fit the box both ways: never wider than its column, never taller than its row', () => {
    const w = stageTileWidth(2, 3)
    // Across: the box's width over two columns, less the gap between them.
    expect(w).toContain('(100cqw - 1 * ')
    expect(w).toContain(') / 2)')
    // Down: the box's height over three rows, less two gaps, as a 16:9 width.
    expect(w).toContain('(100cqh - 2 * ')
    expect(w).toContain(') / 3 * 16 / 9)')
    expect(w.startsWith('min(')).toBe(true)
  })

  it('the tiles sit in a box of their own that the controls never share: it takes the room left and the tiles fit inside it', async () => {
    const s = setup([fakeTrack('7'), fakeTrack('8')])
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" controls={<div data-testid="the-controls" />} />)
    await ready(s)
    const box = await screen.findByTestId('video-grid-box')
    const grid = screen.getByTestId('video-grid')
    expect(box.contains(grid)).toBe(true)
    // A size container: the tiles measure it (cqw, cqh), so they shrink to fit.
    expect(box.className).toContain('@container-[size]')
    expect(box.className).toMatch(/\bflex-1\b/)
    expect(box.className).toMatch(/\bmin-h-0\b/)
    expect(grid.dataset.rows).toBe('1')
    // The controls are after the box, never over it.
    const controls = screen.getByTestId('video-stage-controls')
    expect(box.contains(controls)).toBe(false)
    expect(controls.className).toMatch(/\bflex-shrink-0\b/)
    expect(box.compareDocumentPosition(controls) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
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

  it("the share list has room for its words and a Cancel the size of the look's buttons", async () => {
    const s = setup([])
    render(<Controls s={s} />)
    await ready(s)
    fireEvent.click(await screen.findByTestId('video-screen-pick'))
    const picker = await screen.findByTestId('video-screen-picker')
    // Wraps onto a second line in a narrow window rather than cutting the prompt.
    expect(picker.className).toMatch(/\bflex-wrap\b/)
    const field = screen.getByTestId('video-screen-source')
    expect(field.textContent).toContain('Pick a screen or window')
    const holder = field.parentElement!
    // Wide enough for "Pick a screen or window", and never wider than the window.
    expect(holder.className).not.toMatch(/max-w-\[170px\]/)
    expect(holder.className).toContain('w-[15rem]')
    expect(holder.className).toMatch(/\bmax-w-full\b/)
    // Cancel is the look's own button: 32px or more, cut in Terminal, round in Glass, the shared focus ring.
    const cancel = screen.getByTestId('video-screen-cancel')
    expect(cancel.className).toMatch(/\bmin-h-8\b/)
    expect(cancel.getAttribute('data-k2-chamfer')).toBe('control')
    expect(cancel.className).toContain('rounded-[var(--radius-field)]')
    expect(cancel.className).toContain('k2-focus')
    fireEvent.click(cancel)
    expect(screen.queryByTestId('video-screen-picker')).toBeNull()
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
