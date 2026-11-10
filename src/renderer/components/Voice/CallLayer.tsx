// The call's video, once per window (mounted at the app's root, as K2
// Companion's `VoiceMiniTile` and `VoiceCallScreen`): while video is on in a
// call, either the call view (VideoStage, over the current page) or the
// floating tile (CallTile) in a corner. Clicking the tile opens the call
// view; its Hide returns to the page and the tile. When the video stops, both
// go and the call's controls are back in the top bar (VoiceButton).
//
// Only where calls happen: the app's native engine with video rooms on. The
// web client's calls are seats with no media, so it never shows either.
// Everything it shows comes from the voice and video stores (the engine's
// truth); the only state here is this window's view of it: the call view
// open or closed, full screen (CallFullscreen, ⌃⌘V / Ctrl+Alt+V), and
// whether the call is in its own window (Pop out), where every other window
// shows neither the tile nor the call view until that window closes.

import { useEffect, useMemo } from 'react'
import { createPortal } from 'react-dom'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { isCallFullscreenChord, isCallWindow, noteCallConnected } from '@/lib/voice/call-view'
import { getWindowLabel } from '@/lib/window-chrome'
import { toggleCallFullscreen, useCallFullscreen } from '@/stores/call-fullscreen'
import { useCallPopout, watchCallPopout } from '@/stores/call-popout'
import { useSettingsStore } from '@/stores/settings'
import { useCallScreen } from '@/stores/call-screen'
import { useVoice, voice } from '@/stores/voice'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import type { CallPerson } from '@/lib/voice/video-labels'
import CallTile, { useCallVideoLive } from './CallTile'
import VideoStage from './VideoStage'
import CallFullscreen from './CallFullscreen'
import { CALL_PANEL_ICON, CallPanelButton } from './CallControls'
import { MicIcon, MicOffIcon, PhoneOffIcon } from '@/components/icons/K2Icons'

const NO_PEOPLE: CallPerson[] = []

/** The floating tile is up in this window: the top bar's call controls give
 *  way to the tile's strip. */
export function useCallTileShown(): boolean {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  const open = useCallScreen((s) => s.open)
  const live = useCallVideoLive(video, call?.participants ?? NO_PEOPLE)
  const poppedOut = useCallPopout((s) => s.open)
  return call !== null && native && preview && live && !open && !settingsOpen && !poppedOut
}

/** This webview is the call's own window (it draws the call itself). */
export function inCallWindow(): boolean {
  return isCallWindow(window.location.hash, getWindowLabel())
}

/** The full-screen chord, while the call has video in this window. */
export function useCallFullscreenChord(active: boolean): void {
  useEffect(() => {
    if (!active) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.repeat || !isCallFullscreenChord(e, useSettingsStore.getState().keybindings ?? {}, isMacPlatform())) return
      e.preventDefault()
      e.stopPropagation()
      toggleCallFullscreen()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [active])
}

export const callActions = {
  setMuted: (m: boolean) => voice.setMuted(m),
  setDeafened: (d: boolean) => voice.setDeafened(d),
  leave: () => voice.leave(),
}

function InCall({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const open = useCallScreen((s) => s.open)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  const live = useCallVideoLive(video, call.participants)
  const fullscreen = useCallFullscreen((s) => s.mode) !== 'off'
  const poppedOut = useCallPopout((s) => s.open)
  useCallFullscreenChord(live && !poppedOut)
  // The video stopped: the call view goes with it, and next time the tile
  // comes first again.
  useEffect(() => {
    if (!live && open) useCallScreen.getState().close()
  }, [live, open])
  // The call ended (this unmounts): the same.
  useEffect(() => () => useCallScreen.getState().close(), [])
  // The call's own window counts its time from the same second.
  useEffect(() => {
    if (call.connectedAt !== null && call.callId) noteCallConnected(call.callId, call.connectedAt)
  }, [call.callId, call.connectedAt])

  // In its own window: that window draws it.
  if (poppedOut) return null
  const host = live ? <CallFullscreen call={call} video={video} actions={callActions} /> : null
  // Full screen: the view is the host's; the tile and call view wait.
  if (fullscreen) return host
  if (open && live) {
    const stage = createPortal(
      <VideoStage
        ctl={video}
        people={call.participants}
        title={call.roomName}
        viewButtons
        onHide={() => useCallScreen.getState().close()}
        footerStart={
          <CallPanelButton testId="video-stage-mute" label={call.muted ? 'Unmute' : 'Mute'} on={call.muted} onPress={() => voice.setMuted(!call.muted)}>
            {call.muted ? <MicOffIcon size={CALL_PANEL_ICON} /> : <MicIcon size={CALL_PANEL_ICON} />}
          </CallPanelButton>
        }
        footer={
          <CallPanelButton testId="video-stage-leave" label="Leave call" danger onPress={() => void voice.leave()}>
            <PhoneOffIcon size={CALL_PANEL_ICON} />
          </CallPanelButton>
        }
      />,
      document.body,
    )
    return (
      <>
        {host}
        {stage}
      </>
    )
  }
  if (live && !settingsOpen) {
    return (
      <>
        {host}
        <CallTile call={call} video={video} actions={callActions} />
      </>
    )
  }
  return host
}

export default function CallLayer(): React.JSX.Element | null {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const callWindow = useMemo(inCallWindow, [])
  // Whether the call's window is open, followed from the first render (it
  // may have opened before this window did).
  useEffect(() => (callWindow ? undefined : watchCallPopout()), [callWindow])
  if (callWindow || !call || !native || !preview) return null
  return <InCall call={call} />
}
