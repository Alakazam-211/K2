// The call's video full screen (Rosson 2026-11-10): the picture the floating
// tile shows (the active speaker, held 1.5 s), filling the screen, with the
// name at its bottom left and the call's time at its bottom right; everyone
// else's video in a column at the top right; and the call's controls (mute,
// deafen, camera, share, leave, pop out, exit) in a bar that hides after a
// moment with no pointer or key, and comes back on either.
//
// Mounted while the call has video, as an empty host the store asks the OS
// to put in full screen (stores/call-fullscreen.ts); where the OS can't, the
// same view covers the window. Esc leaves.

import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Surface } from '@/components/ui/Surface'
import { CALL_FULLSCREEN_Z } from '@/lib/overlay-layers'
import { tilePicture, hasVideo } from '@/lib/voice/call-tile-picture'
import { CONTROLS_HIDE_MS } from '@/lib/voice/call-view'
import { exitCallFullscreen, onFullscreenChange, setCallFullscreenHost, useCallFullscreen } from '@/stores/call-fullscreen'
import type { VideoController } from '@/stores/video-core'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { type CallControlsProps } from './CallControls'
import { FullscreenButton, PopOutButton } from './CallViewButtons'
import { useActiveSpeaker, useCallDuration, useCallTiles } from './CallTile'
import { ScreenPicker, VideoNoticeLine } from './VideoControls'
import VideoTile from './VideoTile'

/** Other people's video beside the picture, at most this many. */
const MAX_THUMBS = 4

export interface CallFullscreenProps {
  call: VoiceCall
  video: VideoController
  actions: CallControlsProps['actions']
  /** The call's own window: no Pop out (it's already out). */
  inWindow?: boolean
  /** When the call's time started (the call's window keeps the room's). */
  startedAt?: number | null
}

function View({ call, video, actions, inWindow, startedAt }: CallFullscreenProps): React.JSX.Element {
  const tiles = useCallTiles(video, call.participants)
  const speaker = useActiveSpeaker(call.participants)
  const picture = tilePicture(tiles, speaker)
  const thumbs = tiles.filter((t) => hasVideo(t) && t.key !== picture?.key).slice(0, MAX_THUMBS)
  const time = useCallDuration(startedAt === undefined ? call.connectedAt : startedAt)
  const [shown, setShown] = useState(true)
  const over = useRef(false)
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null)

  const wake = (): void => {
    setShown(true)
    if (timer.current) clearTimeout(timer.current)
    timer.current = setTimeout(() => {
      timer.current = null
      if (!over.current) setShown(false)
    }, CONTROLS_HIDE_MS)
  }
  useEffect(() => {
    wake()
    const onKey = (e: KeyboardEvent): void => {
      // The browser's own Esc leaves element full screen; this one leaves
      // the in-window view (and the OS one, where the key still reaches us).
      if (e.key === 'Escape') {
        // Only this: not a drawer or a dialog under the view too.
        e.preventDefault()
        e.stopPropagation()
        exitCallFullscreen()
        return
      }
      wake()
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      if (timer.current) clearTimeout(timer.current)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return (
    <div
      className={`relative h-full w-full ${shown ? '' : 'cursor-none'}`}
      onPointerMove={wake}
      onPointerDown={wake}
      onFocusCapture={wake}
    >
      <div data-testid="call-fullscreen-picture" className="absolute inset-0">
        {picture ? (
          <VideoTile
            tile={{ ...picture, large: true, speaking: false }}
            ctl={video}
            fill
            corner={time ? <span data-testid="call-fullscreen-duration" className="font-mono tabular-nums">{time}</span> : null}
          />
        ) : null}
      </div>
      {thumbs.length > 0 ? (
        <div data-testid="call-fullscreen-thumbs" className="absolute right-4 top-4 flex w-[200px] flex-col gap-2">
          {thumbs.map((t) => (
            <VideoTile key={t.key} tile={{ ...t, large: false }} ctl={video} />
          ))}
        </div>
      ) : null}
      <div
        data-testid="call-fullscreen-controls"
        data-shown={shown ? 'true' : 'false'}
        className={`absolute inset-x-0 bottom-10 flex justify-center transition-opacity duration-200 ${shown ? 'opacity-100' : 'opacity-0'}`}
        onPointerEnter={() => {
          over.current = true
        }}
        onPointerLeave={() => {
          over.current = false
          wake()
        }}
      >
        <Surface role2="elevated" elevation={4} className="flex flex-col gap-1 p-1.5">
          <div className="flex items-center gap-1.5">
            <CallControls variant="overlay" call={call} actions={actions} video={video} />
            {inWindow ? null : <PopOutButton testId="call-fullscreen-popout" look="panel" />}
            <FullscreenButton testId="call-fullscreen-exit" look="panel" chord={null} />
          </div>
          <ScreenPicker ctl={video} />
          <VideoNoticeLine ctl={video} noticeOnly />
        </Surface>
      </div>
    </div>
  )
}

/** The host, always there while the call has video; the view only while full
 *  screen is on. */
export default function CallFullscreen(props: CallFullscreenProps): React.JSX.Element | null {
  const mode = useCallFullscreen((s) => s.mode)
  const ref = useRef<HTMLDivElement>(null)
  useEffect(() => {
    setCallFullscreenHost(ref.current)
    document.addEventListener('fullscreenchange', onFullscreenChange)
    document.addEventListener('webkitfullscreenchange', onFullscreenChange)
    return () => {
      document.removeEventListener('fullscreenchange', onFullscreenChange)
      document.removeEventListener('webkitfullscreenchange', onFullscreenChange)
      exitCallFullscreen()
      setCallFullscreenHost(null)
    }
  }, [])
  if (typeof document === 'undefined') return null
  const on = mode !== 'off'
  return createPortal(
    <div
      ref={ref}
      role={on ? 'dialog' : undefined}
      aria-modal={on || undefined}
      aria-label={on ? `Video call in ${props.call.roomName}, full screen` : undefined}
      aria-hidden={on ? undefined : true}
      data-testid="call-fullscreen"
      data-mode={mode}
      className={on ? 'fixed inset-0 bg-[var(--color-bg-canvas)]' : 'hidden'}
      style={on ? ({ zIndex: CALL_FULLSCREEN_Z, WebkitAppRegion: 'no-drag' } as React.CSSProperties) : undefined}
    >
      {on ? <View {...props} /> : null}
    </div>,
    document.body,
  )
}
