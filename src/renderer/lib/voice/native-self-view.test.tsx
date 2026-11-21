// @vitest-environment jsdom
//
// The native self-view (macOS): the window that shows the call tells Rust
// whether it wants it (your camera on or paused there), Rust's state hides
// the page's own self-view tile while the native one shows, and the page's
// tile is back whenever it doesn't (never a blank tile).

import { act, cleanup, render, renderHook, screen, waitFor, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { FakeVideoEngine, fakeTrack } from '@/lib/voice/fake-video-engine'
import { FakeVideoDecoder, fakeWebCodecs, RecordingSurface } from '@/lib/voice/fake-webcodecs'
import type { CallPerson, VideoTile } from '@/lib/voice/video-labels'
import { createVideoController } from '@/stores/video-core'
import VideoStage from '@/components/Voice/VideoStage'
import VideoTileView from '@/components/Voice/VideoTile'
import {
  __setSelfViewOpsForTests,
  callViewAnchor,
  isOwnCameraTile,
  parseSelfViewState,
  selfViewWanted,
  stageTilesForNativeSelf,
  useNativeSelfView,
  useNativeSelfViewWant,
  type SelfViewAnchor,
} from './native-self-view'

const people: CallPerson[] = [
  { memberId: 'me', isSelf: true, user: 'You', speaking: false, muted: false },
  { memberId: '7', user: 'Ada', speaking: false, muted: false },
]

function tile(over: Partial<VideoTile>): VideoTile {
  return {
    key: 'TR_7',
    memberId: '7',
    self: false,
    source: 'camera',
    name: 'Ada',
    speaking: false,
    audioMuted: false,
    state: 'live',
    paused: null,
    mirrored: false,
    large: false,
    ...over,
  }
}

const mine = tile({ key: 'local:camera', memberId: 'me', self: true, name: 'You', mirrored: true })
const ada = tile({})

interface Told {
  want: boolean
  anchor: SelfViewAnchor | null
}

function fakeOps() {
  const told: Told[] = []
  let onState: ((p: unknown) => void) | null = null
  __setSelfViewOpsForTests({
    tell: async (want, anchor) => {
      told.push({ want, anchor })
      return true
    },
    listen: async (handler) => {
      onState = handler
      return () => {
        onState = null
      }
    },
  })
  return {
    told,
    emit: (p: unknown) => act(() => onState?.(p)),
    listening: () => onState !== null,
  }
}

beforeEach(() => FakeVideoDecoder.reset())
afterEach(() => {
  cleanup()
  __setSelfViewOpsForTests(null)
})

describe('the rule', () => {
  it('wants the self-view only where the call shows and the camera is on or paused', () => {
    expect(selfViewWanted('on', true)).toBe(true)
    expect(selfViewWanted('paused', true)).toBe(true)
    expect(selfViewWanted('asking', true)).toBe(false)
    expect(selfViewWanted('off', true)).toBe(false)
    expect(selfViewWanted('on', false)).toBe(false)
  })

  it('reads Rust state events and nothing else', () => {
    expect(parseSelfViewState({ available: true, showing: true, reason: null })).toEqual({ available: true, showing: true, reason: null })
    expect(parseSelfViewState({ available: true, showing: false, reason: 'closed' })).toEqual({ available: true, showing: false, reason: 'closed' })
    expect(parseSelfViewState({ available: true, showing: 'yes' })).toBeNull()
    expect(parseSelfViewState({ available: true, showing: false, reason: 4 })).toBeNull()
    expect(parseSelfViewState(null)).toBeNull()
    expect(parseSelfViewState([])).toBeNull()
  })

  it('knows your own camera tile from your screen and from everyone else', () => {
    expect(isOwnCameraTile(mine)).toBe(true)
    expect(isOwnCameraTile(ada)).toBe(false)
    expect(isOwnCameraTile(tile({ key: 'local:screen', self: true, source: 'screen' }))).toBe(false)
  })

  it('leaves your tile out of the stage while the native one shows, and never leaves it empty', () => {
    // Not showing: as before.
    expect(stageTilesForNativeSelf([ada, mine], people, false)).toEqual([ada, mine])
    // Showing, with someone else's video: theirs only.
    expect(stageTilesForNativeSelf([ada, mine], people, true)).toEqual([ada])
    // Showing, and yours was the only video: everyone as their picture.
    const off = tile({ state: 'off' })
    const shown = stageTilesForNativeSelf([off, mine], people, true)
    expect(shown.map((t) => [t.name, t.state, t.self])).toEqual([
      ['Ada', 'off', false],
      ['You', 'off', true],
    ])
    expect(shown.some(isOwnCameraTile)).toBe(false)
  })

  it('anchors on the stage grid or share when one is on screen', () => {
    expect(callViewAnchor(document)).toBeNull()
    const box = document.createElement('div')
    box.dataset.testid = 'video-grid-box'
    box.getBoundingClientRect = () => ({ left: 10, top: 20, width: 300, height: 200 }) as DOMRect
    document.body.appendChild(box)
    expect(callViewAnchor(document)).toEqual({ x: 10, y: 20, width: 300, height: 200 })
    box.getBoundingClientRect = () => ({ left: 10, top: 20, width: 0, height: 0 }) as DOMRect
    expect(callViewAnchor(document)).toBeNull()
    box.remove()
  })
})

describe('telling Rust', () => {
  it('tells on each change and lets go on unmount, and follows the state', async () => {
    const f = fakeOps()
    const { rerender, unmount } = renderHook(({ want }) => useNativeSelfViewWant(want), { initialProps: { want: false } })
    // Nothing wanted yet: nothing said.
    expect(f.told).toEqual([])
    await waitFor(() => expect(f.listening()).toBe(true))
    rerender({ want: true })
    rerender({ want: true })
    expect(f.told).toEqual([{ want: true, anchor: null }])
    f.emit({ available: true, showing: true, reason: null })
    expect(useNativeSelfView.getState()).toEqual({ available: true, showing: true, reason: null })
    rerender({ want: false })
    expect(f.told.map((t) => t.want)).toEqual([true, false])
    f.emit({ available: true, showing: false, reason: null })
    expect(useNativeSelfView.getState().showing).toBe(false)
    // Wanted again, then unmounted (the window stops showing the call).
    rerender({ want: true })
    unmount()
    expect(f.told.map((t) => t.want)).toEqual([true, false, true, false])
    // Unreadable events change nothing.
    f.emit({ nope: true })
    expect(useNativeSelfView.getState().showing).toBe(false)
  })
})

describe('the page\'s own self-view tile', () => {
  function setup(tracks = [fakeTrack('7')]) {
    const engine = new FakeVideoEngine()
    engine.tracks = tracks
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
      now: () => Date.now(),
    })
    return { engine, ctl }
  }

  async function cameraOn(s: ReturnType<typeof setup>): Promise<void> {
    await act(async () => {
      s.ctl.setActive({ enabled: true, inCall: true })
      await new Promise((r) => setTimeout(r, 0))
    })
    await waitFor(() => expect(s.ctl.store.getState().availability).toBe('ready'))
    await act(async () => s.ctl.setCamera(true))
  }

  it('hides from the stage while the native one shows and comes back when it stops', async () => {
    fakeOps()
    const s = setup()
    render(<VideoStage ctl={s.ctl} people={people} title="Standup" />)
    await cameraOn(s)
    await waitFor(() => expect(screen.getAllByTestId('video-tile').map((t) => t.dataset.track)).toEqual(['TR_7', 'local:camera']))
    act(() => useNativeSelfView.setState({ available: true, showing: true, reason: null }))
    expect(screen.getAllByTestId('video-tile').map((t) => t.dataset.track)).toEqual(['TR_7'])
    // Its webview sink closes after the grace period; the camera stays on.
    expect(s.ctl.store.getState().local.camera).toBe('on')
    // The person closed the native window (or it failed): the page's tile is back.
    act(() => useNativeSelfView.setState({ available: true, showing: false, reason: 'closed' }))
    const tiles = screen.getAllByTestId('video-tile')
    expect(tiles.map((t) => t.dataset.track)).toEqual(['TR_7', 'local:camera'])
    expect((within(tiles[1]).getByTestId('video-canvas') as HTMLCanvasElement).style.transform).toBe('scaleX(-1)')
  })

  it('a card that would show your camera shows your picture instead, never a blank tile', async () => {
    fakeOps()
    const s = setup([])
    act(() => useNativeSelfView.setState({ available: true, showing: true, reason: null }))
    await cameraOn(s)
    render(<VideoTileView tile={mine} ctl={s.ctl} />)
    const t = screen.getByTestId('video-tile')
    expect(t.dataset.nativeSelf).toBe('true')
    expect(within(t).getByTestId('video-tile-avatar')).toBeTruthy()
    expect(within(t).getByTestId('video-tile-line').textContent).toBe('Your camera is in its own window')
    // No webview sink for it.
    expect(s.ctl.sinks().map((x) => x.track)).not.toContain('local:camera')
  })
})
