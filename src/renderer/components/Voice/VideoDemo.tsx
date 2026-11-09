// The dev video demo (dev builds only, with `videoRoomsPreview` on): the room's
// video UI over the fake engine. Remote cameras are synthetic VP8 from this
// webview's own `VideoEncoder`, so the whole receive path runs (frame header,
// WebCodecs decoder, keyframe requests, canvas) with no plugin, no call and no
// camera. Camera on, Share screen and the refusals work against the fake.

import { useEffect, useMemo, useRef, useState } from 'react'
import { useBrowserOverlay } from '@/lib/browser-overlay'
import { FakeVideoEngine, fakeTrack } from '@/lib/voice/fake-video-engine'
import { canMakeSyntheticVideo, syntheticVp8Source } from '@/lib/voice/synthetic-video-source'
import { webCodecsEnv } from '@/lib/voice/video-decoder'
import { LOCAL_CAMERA_TRACK, LOCAL_SCREEN_TRACK } from '@/lib/voice/video-engine'
import type { CallPerson } from '@/lib/voice/video-labels'
import { createVideoController } from '@/stores/video-core'
import { watchTileBox } from '@/stores/video'
import VideoStage from './VideoStage'

const COLORS: Record<string, string> = {
  m1: '#3b5b8c',
  m2: '#6b4a8c',
  m3: '#2f7a5a',
  m4: '#8c5a2f',
  [LOCAL_CAMERA_TRACK]: '#444c56',
  [LOCAL_SCREEN_TRACK]: '#1f2933',
}

const NAMES: Record<string, string> = { m1: 'Ada', m2: 'Grace', m3: 'Linus', m4: 'Barbara' }

function demoController() {
  const engine = new FakeVideoEngine({
    source: (track) => {
      const member = track.replace(/^TR_/, '')
      const label = track === LOCAL_CAMERA_TRACK ? 'You (self-view)' : track === LOCAL_SCREEN_TRACK ? 'Your screen' : (NAMES[member] ?? track)
      return syntheticVp8Source({ label, color: COLORS[member] ?? COLORS[track] ?? '#333' })
    },
  })
  engine.tracks = [fakeTrack('m1'), fakeTrack('m2'), fakeTrack('m3', { muted: true }), fakeTrack('m4', { paused: 'congestion' })]
  const ctl = createVideoController({
    loadEngine: async () => engine,
    webCodecs: webCodecsEnv,
    watchBox: watchTileBox,
    devicePixelRatio: () => window.devicePixelRatio || 1,
    loadPrefs: () => ({ cameraId: null }),
    savePrefs: () => undefined,
    toast: () => undefined,
    openPrivacySettings: async () => false,
    now: () => Date.now(),
  })
  return { engine, ctl }
}

/** The demo needs `VideoEncoder` to make its frames. */
function NoEncoderNote({ onClose }: { onClose: () => void }): React.JSX.Element {
  const ref = useRef<HTMLDivElement>(null)
  useBrowserOverlay({ active: true, ref, kind: 'popover' })
  return (
    <div ref={ref} data-testid="video-demo-unsupported" data-material="tile" data-floating="" className="fixed bottom-3 right-3 z-40 border border-[var(--color-border)] bg-[var(--color-bg)] p-2 text-[11px] text-[var(--color-text-secondary)] shadow-lg">
      This webview has no VideoEncoder, so the demo can't make video.{' '}
      <button type="button" className="underline" onClick={onClose}>
        Close
      </button>
    </div>
  )
}

export default function VideoDemo({ onClose }: { onClose: () => void }): React.JSX.Element {
  const { engine, ctl } = useMemo(demoController, [])
  const [speaker, setSpeaker] = useState(0)
  useEffect(() => {
    ctl.setActive({ enabled: true, inCall: true })
    const id = setInterval(() => setSpeaker((n) => n + 1), 3000)
    return () => {
      clearInterval(id)
      ctl.stop()
    }
  }, [ctl])
  const people: CallPerson[] = useMemo(
    () => [
      { memberId: 'me', isSelf: true, user: 'You', speaking: false, muted: false },
      ...Object.entries(NAMES).map(([id, user], i) => ({ memberId: id, user, speaking: speaker % 4 === i, muted: id === 'm2' })),
    ],
    [speaker],
  )
  if (!canMakeSyntheticVideo()) return <NoEncoderNote onClose={onClose} />
  return (
    <VideoStage
      ctl={ctl}
      people={people}
      title="Video demo (fake engine)"
      onClose={onClose}
      footer={
        <button
          type="button"
          data-testid="video-demo-refuse"
          className="ml-auto border border-[var(--color-border)] px-2 py-0.5 text-[11px] text-[var(--color-text-secondary)]"
          onClick={() => engine.refuseNext('camera', 'video_slots_full')}
        >
          Next camera: room full
        </button>
      }
    />
  )
}
