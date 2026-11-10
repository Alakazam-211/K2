// Voice rooms (prd-k2-voice-rooms-v1 §4, lane D S4/S5): the top-bar
// headset and its list.
//
// Shown while a server this client is signed in to reports
// `voice-rooms-v1` (§4.1), which a daemon does only while it hosts rooms.
// When none does but the window's own server has `voice_rooms_preview` on
// (a Mac, no Pro, Sew off…), the headset shows dimmed: a click opens the
// list with the daemon's own reason (`/cli/voice/rooms` `hosting`, the line
// Settings → Voice Rooms shows). Preview off: no headset. It never follows
// a focused remote Home room: the call belongs to the app, not the window's
// server.

import { useCallback, useEffect, useState } from 'react'
import { createPortal } from 'react-dom'
import TopBarPipe from '@/components/TopBar/TopBarPipe'
import { useAnchoredMenu } from '@/hooks/useAnchoredMenu'
import { VoiceRoomsIcon, TOP_BAR_ICON_ACTIVE_CLASS, TOP_BAR_ICON_REST_CLASS } from '@/components/icons/K2Icons'
import { anyVoiceServer } from '@/lib/voice/voice-servers'
import { retainVoice, useVoice, voice } from '@/stores/voice'
import { useSettingsStore } from '@/stores/settings'
import { headsetState } from './voice-labels'
import VoicePopover from './VoicePopover'
import CallControls from './CallControls'
import { useCallTileShown } from './CallLayer'
import VideoDemo from './VideoDemo'
import { retainVideo, video } from '@/stores/video'
import { useVoiceOpenRequest } from '@/stores/voice-open-request'

// eslint-disable-next-line @typescript-eslint/no-explicit-any
const noDrag = { WebkitAppRegion: 'no-drag' } as any

const LOOK_CLASS = {
  rest: TOP_BAR_ICON_REST_CLASS,
  occupied: TOP_BAR_ICON_REST_CLASS,
  connecting: `${TOP_BAR_ICON_ACTIVE_CLASS} animate-pulse`,
  'in-call': 'text-[var(--color-accent)] hover:bg-[var(--color-bg-elevated)]',
  unavailable: `${TOP_BAR_ICON_REST_CLASS} opacity-40 hover:opacity-80`,
} as const

/** The dimmed headset's tooltip: the reason itself is in the list. */
export const UNAVAILABLE_TOOLTIP = "Voice rooms: this server doesn't host them. Click to see why."


/** `withPipe`: the roster is hidden, so the headset brings its own divider
 *  (§4.1); with the roster showing, the roster's divider follows it. */
export default function VoiceButton({ withPipe = false }: { withPipe?: boolean }): React.JSX.Element | null {
  useEffect(() => retainVoice(), [])
  useEffect(() => retainVideo(), [])
  const servers = useVoice((s) => s.servers)
  const rooms = useVoice((s) => s.rooms)
  const call = useVoice((s) => s.call)
  const open = useVoice((s) => s.popoverOpen)
  const preview = useSettingsStore((s) => s.voiceRoomsPreview)
  const videoPreview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const engineKind = useVoice((s) => s.engineKind)
  const [demo, setDemo] = useState(false)
  // In a call: Mute, Deafen, Camera and Hang up beside the headset, unless
  // the call's video is up in the floating tile, whose strip has them.
  const tileShown = useCallTileShown()
  const showControls = call !== null && engineKind === 'native' && !tileShown
  const [, setTick] = useState(0)

  const close = useCallback(() => voice.setPopoverOpen(false), [])
  // The list floats on the app's popover layer (portalled to <body>), so the
  // top bar's frosted capsule can't trap it under the tiles. The hook closes
  // it on an outside click (a pick in a device menu is not outside) or Escape.
  // Only while the headset shows: it hides with the list still flagged open
  // when the last voice server signs out, and nothing may stay on screen.
  const unavailable = !call && !anyVoiceServer(servers)
  const shown = !(unavailable && !preview)
  const menu = useAnchoredMenu<HTMLDivElement>({
    open: open && shown,
    onClose: close,
    width: 'min',
    minWidth: 320,
    align: 'end',
    gap: 4,
    tabOrder: true,
  })
  // §8.3: the Voice & Chat only gate screen's "Open voice rooms".
  const openAsked = useVoiceOpenRequest((s) => s.pending)
  useEffect(() => {
    if (openAsked && useVoiceOpenRequest.getState().take()) voice.setPopoverOpen(true)
  }, [openAsked])
  // The call timer.
  useEffect(() => {
    if (!open) return
    const id = setInterval(() => setTick((n) => n + 1), 1000)
    return () => clearInterval(id)
  }, [open])

  if (!shown) return null
  const look = unavailable ? { look: 'unavailable' as const, tooltip: UNAVAILABLE_TOOLTIP, muted: false } : headsetState(call, servers, rooms)

  return (
    <>
      <div className="relative flex items-center no-drag" ref={menu.anchorRef}>
        <button
          type="button"
          aria-label={look.tooltip}
          title={look.tooltip}
          aria-expanded={open}
          aria-haspopup="dialog"
          data-testid="voice-button"
          data-look={look.look}
          className={`flex h-6 w-6 items-center justify-center transition-colors ${LOOK_CLASS[look.look]}`}
          style={noDrag}
          onClick={() => {
            if (!open && unavailable) void voice.readWindowHosting()
            voice.setPopoverOpen(!open)
          }}
        >
          <VoiceRoomsIcon muted={look.look === 'in-call' && look.muted} live={look.look === 'occupied' || look.look === 'in-call'} />
        </button>
        {open
          ? menu.portal(
              <VoicePopover
                menuRef={menu.menuRef}
                style={menu.style}
                unavailable={unavailable}
                onVideoDemo={import.meta.env.DEV && videoPreview ? () => setDemo(true) : undefined}
              />,
            )
          : null}
      </div>
      {showControls && call ? (
        <div className="ml-0.5 flex items-center no-drag" style={noDrag}>
          <CallControls variant="topbar" call={call} actions={voice} video={videoPreview ? video : null} />
        </div>
      ) : null}
      {withPipe ? <TopBarPipe /> : null}
      {demo && videoPreview ? createPortal(<VideoDemo onClose={() => setDemo(false)} />, document.body) : null}
    </>
  )
}
