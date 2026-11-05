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

import { useCallback, useEffect, useRef, useState } from 'react'
import TopBarPipe from '@/components/TopBar/TopBarPipe'
import { HeadsetIcon, TOP_BAR_ICON_ACTIVE_CLASS, TOP_BAR_ICON_REST_CLASS } from '@/components/icons/K2Icons'
import { anyVoiceServer } from '@/lib/voice/voice-servers'
import { retainVoice, useVoice, voice } from '@/stores/voice'
import { useSettingsStore } from '@/stores/settings'
import { headsetState } from './voice-labels'
import VoicePopover from './VoicePopover'

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
  const servers = useVoice((s) => s.servers)
  const rooms = useVoice((s) => s.rooms)
  const call = useVoice((s) => s.call)
  const open = useVoice((s) => s.popoverOpen)
  const preview = useSettingsStore((s) => s.voiceRoomsPreview)
  const rootRef = useRef<HTMLDivElement>(null)
  const [, setTick] = useState(0)

  const close = useCallback(() => voice.setPopoverOpen(false), [])
  useEffect(() => {
    if (!open) return
    const onPointer = (ev: MouseEvent): void => {
      const t = ev.target as Node
      if (rootRef.current?.contains(t)) return
      // The device menus portal to <body>: a pick there is not outside.
      if (t instanceof Element && t.closest('[data-setting-dropdown-menu]')) return
      close()
    }
    const onKey = (ev: KeyboardEvent): void => {
      if (ev.key === 'Escape') close()
    }
    document.addEventListener('mousedown', onPointer)
    document.addEventListener('keydown', onKey)
    // The call timer.
    const id = setInterval(() => setTick((n) => n + 1), 1000)
    return () => {
      document.removeEventListener('mousedown', onPointer)
      document.removeEventListener('keydown', onKey)
      clearInterval(id)
    }
  }, [open, close])

  const unavailable = !call && !anyVoiceServer(servers)
  if (unavailable && !preview) return null
  const look = unavailable ? { look: 'unavailable' as const, tooltip: UNAVAILABLE_TOOLTIP, muted: false } : headsetState(call, servers, rooms)

  return (
    <>
      <div className="relative flex items-center no-drag" ref={rootRef}>
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
          <HeadsetIcon muted={look.look === 'in-call' && look.muted} live={look.look === 'occupied' || look.look === 'in-call'} />
        </button>
        {open ? <VoicePopover unavailable={unavailable} /> : null}
      </div>
      {withPipe ? <TopBarPipe /> : null}
    </>
  )
}
