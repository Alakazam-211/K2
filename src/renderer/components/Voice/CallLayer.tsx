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
// open or closed, expanded over K2 (CallExpanded, ⌃⌘V / Ctrl+Alt+V), and
// whether the call is in its own window (Pop out), where every other window
// shows neither the tile nor the call view until that window closes. Pop in
// there brings it back to the window that popped it out, as its call view
// with the same share in the middle; the others get their tile back.

import { useEffect, useMemo, useRef } from 'react'
import { createPortal } from 'react-dom'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { isCallExpandChord, isCallWindow, noteCallConnected, takeCallHandBack } from '@/lib/voice/call-view'
import { getWindowLabel } from '@/lib/window-chrome'
import { toggleCallExpand, useCallExpand } from '@/stores/call-expand'
import { loadSharePick } from '@/stores/call-share'
import { useCallPopout, watchCallPopout } from '@/stores/call-popout'
import { useSettingsStore } from '@/stores/settings'
import { useCallScreen } from '@/stores/call-screen'
import { useVoice, voice } from '@/stores/voice'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import type { CallPerson } from '@/lib/voice/video-labels'
import CallTile, { useCallVideoLive } from './CallTile'
import VideoStage from './VideoStage'
import CallExpanded from './CallExpanded'
import RecordingNotice from './RecordingNotice'
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

/** The Expand chord, while the call has video in this window. */
export function useCallExpandChord(active: boolean): void {
  useEffect(() => {
    if (!active) return
    const onKey = (e: KeyboardEvent): void => {
      if (e.repeat || !isCallExpandChord(e, useSettingsStore.getState().keybindings ?? {}, isMacPlatform())) return
      e.preventDefault()
      e.stopPropagation()
      toggleCallExpand()
    }
    window.addEventListener('keydown', onKey, true)
    return () => window.removeEventListener('keydown', onKey, true)
  }, [active])
}

export const callActions = {
  setMuted: (m: boolean) => voice.setMuted(m),
  setDeafened: (d: boolean) => voice.setDeafened(d),
  leave: () => voice.leave(),
  startRecording: () => voice.startRecording(),
  stopRecording: () => voice.stopRecording(),
}

/** K2's recording notice, in every window, video or not. */
function NoticeLayer({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const agentReaders = useVoice((s) => s.rooms[call.hostKey]?.list?.agentReaders === true)
  if (!call.recordingNotice) return null
  return createPortal(
    <RecordingNotice call={call} agentReaders={agentReaders} onAnswer={(a) => void voice.answerRecordingNotice(a)} />,
    document.body,
  )
}

function InCall({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const open = useCallScreen((s) => s.open)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  const live = useCallVideoLive(video, call.participants)
  const expanded = useCallExpand((s) => s.expanded)
  const poppedOut = useCallPopout((s) => s.open)
  useCallExpandChord(live && !poppedOut)
  // The call's window closed. Pop in (not the OS's close) hands the call
  // back to the window that popped it out: its call view opens, and every
  // window takes up the share that was in the middle there.
  const wasOut = useRef(poppedOut)
  useEffect(() => {
    const was = wasOut.current
    wasOut.current = poppedOut
    if (!was || poppedOut) return
    loadSharePick(call.callId)
    if (takeCallHandBack(call.callId, getWindowLabel(), Date.now())) useCallScreen.getState().ask()
  }, [poppedOut, call.callId])
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
  const host = live ? <CallExpanded call={call} video={video} actions={callActions} /> : null
  // Expanded: the view is drawn there; the tile and the call view wait (one
  // sink per track).
  if (expanded) return host
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
  const notice = call && native ? <NoticeLayer call={call} /> : null
  if (callWindow || !call || !native || !preview) return notice
  return (
    <>
      {notice}
      <InCall call={call} />
    </>
  )
}
