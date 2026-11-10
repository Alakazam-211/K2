// The call expanded over K2 (Rosson 2026-11-10, in place of full screen): the
// call view grows to nearly the whole window, a margin clear of its edges on
// every side, over the dim scrim, and stays inside the app. With a screen
// share, the share is in the middle and the people in a column of smaller
// tiles on the left (CallStage); with none, the grid. The call's controls
// (mute, deafen, camera, share, leave) sit along its bottom.
//
// Shrink (top right), a click on the scrim round it, or Esc put it back as it
// was: the tile, or the call view. Focus moves to Shrink on open, stays in the
// view while it's open, and goes back to the Expand button on shrink.
//
// Mounted while the call has video; it draws only while expanded, and its
// going (the video stopped, the call ended) shrinks it.

import { useEffect, useRef } from 'react'
import { createPortal } from 'react-dom'
import { Button } from '@/components/ui/Button'
import { DialogFrame, DialogScrim } from '@/components/ui/Dialog'
import { Surface } from '@/components/ui/Surface'
import { ExpandIcon } from '@/components/icons/K2Icons'
import { CALL_EXPANDED_Z } from '@/lib/overlay-layers'
import { CALL_EXPAND_MARGIN } from '@/lib/voice/call-view'
import { returnFocusAfterShrink, shrinkCall, useCallExpand } from '@/stores/call-expand'
import type { VideoController } from '@/stores/video-core'
import type { VoiceCall } from '@/stores/voice-core'
import CallControls, { type CallControlsProps } from './CallControls'
import CallStage from './CallStage'
import { PopOutButton } from './CallViewButtons'
import { useCallDuration } from './CallTile'
import { ScreenPicker, VideoNoticeLine } from './VideoControls'

export interface CallExpandedProps {
  call: VoiceCall
  video: VideoController
  actions: CallControlsProps['actions']
}

const FOCUSABLE = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), [tabindex]:not([tabindex="-1"])'

/** Tab and Shift+Tab go round the view, never out to the page under it. */
function keepFocusIn(e: React.KeyboardEvent<HTMLElement>): void {
  if (e.key !== 'Tab') return
  const all = [...e.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => !el.closest('[aria-hidden="true"]'))
  if (all.length === 0) return
  const first = all[0]
  const last = all[all.length - 1]
  const at = document.activeElement
  if (e.shiftKey && (at === first || !e.currentTarget.contains(at))) {
    e.preventDefault()
    last.focus()
  } else if (!e.shiftKey && at === last) {
    e.preventDefault()
    first.focus()
  }
}

function View({ call, video, actions }: CallExpandedProps): React.JSX.Element {
  const time = useCallDuration(call.connectedAt)
  const shrinkRef = useRef<HTMLButtonElement>(null)
  useEffect(() => {
    shrinkRef.current?.focus()
    const onKey = (e: KeyboardEvent): void => {
      if (e.key !== 'Escape') return
      // Only this: not a drawer or a dialog under the view too.
      e.preventDefault()
      e.stopPropagation()
      shrinkCall()
    }
    window.addEventListener('keydown', onKey, true)
    return () => {
      window.removeEventListener('keydown', onKey, true)
      // The tile or the call view is drawn again by now: its Expand button
      // takes focus back.
      returnFocusAfterShrink()
    }
  }, [])
  return (
    <>
      <DialogScrim zIndex={CALL_EXPANDED_Z} data-testid="call-expanded-scrim" onClick={() => shrinkCall()} style={{ WebkitAppRegion: 'no-drag' } as React.CSSProperties} />
      <DialogFrame
        zIndex={CALL_EXPANDED_Z + 1}
        role="dialog"
        aria-modal
        aria-label={`Video call in ${call.roomName}, expanded`}
        data-testid="call-expanded"
        className="flex flex-col overflow-hidden"
        style={{ width: `calc(100vw - ${CALL_EXPAND_MARGIN * 2}px)`, height: `calc(100vh - ${CALL_EXPAND_MARGIN * 2}px)`, WebkitAppRegion: 'no-drag' } as React.CSSProperties}
        onKeyDown={keepFocusIn}
      >
        <header className="flex flex-shrink-0 items-center gap-2 border-b border-[var(--color-border)] px-3 py-2">
          <span data-testid="call-expanded-title" className="min-w-0 flex-1 truncate text-[12px] text-[var(--color-text-primary)]">
            {call.roomName}
          </span>
          {time ? (
            <span data-testid="call-expanded-duration" className="font-mono text-[11px] tabular-nums text-[var(--color-text-secondary)]">
              {time}
            </span>
          ) : null}
          <PopOutButton testId="call-expanded-popout" look="panel" />
          <Button ref={shrinkRef} variant="quiet" data-testid="call-expanded-shrink" title="Shrink (Esc)" className="flex items-center gap-1.5" onClick={() => shrinkCall()}>
            <ExpandIcon shrink size={13} />
            Shrink
          </Button>
        </header>
        <CallStage ctl={video} people={call.participants} callId={call.callId} className="flex-1" />
        <footer data-testid="call-expanded-controls" className="flex flex-shrink-0 flex-col items-center gap-1 px-3 pb-3 pt-1">
          <Surface role2="elevated" chamfer data-material="frost" data-frost-shape="pill" data-frost-static="" className="flex items-center px-2 py-1.5">
            <CallControls variant="overlay" call={call} actions={actions} video={video} />
          </Surface>
          <ScreenPicker ctl={video} />
          <VideoNoticeLine ctl={video} noticeOnly />
        </footer>
      </DialogFrame>
    </>
  )
}

/** Mounted while the call has video; draws only while expanded. */
export default function CallExpanded(props: CallExpandedProps): React.JSX.Element | null {
  const expanded = useCallExpand((s) => s.expanded)
  // The video stopped or the call ended: next time it's the tile again.
  useEffect(() => () => shrinkCall(), [])
  if (!expanded || typeof document === 'undefined') return null
  return createPortal(<View {...props} />, document.body)
}
