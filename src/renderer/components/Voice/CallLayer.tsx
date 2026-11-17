// The call's video, once per window (mounted at the app's root): the rules
// for where this window shows it, full screen (CallExpanded, ⌃⌘V /
// Ctrl+Alt+V), the recording notice and Back to chat's hand-back. While
// video is on, the stage (VideoStage: the tiles, with the call's controls)
// is drawn in the call's place (CallPlace): a DM call's conversation while
// it is on screen, the page's top for a voice room. It isn't drawn while
// folded into the slim bar (Compact), full screen, or in the call's own
// window (Pop out). There is no floating tile and no corner. When the video
// stops, the stage goes and the call's controls are in its slim bar.
//
// One call, one home window (stores/call-home.ts): only the home window
// draws the stage and opens video sinks (the camera's self-view included);
// every other K2 window has the slim bar with Show call here. Audio is the
// native engine's, in the app's Rust process: it plays once, whichever
// window is home.
//
// Only where calls happen: the app's native engine with video rooms on. The
// web client's calls are seats with no media, so it never shows either.
// Everything it shows comes from the voice and video stores (the engine's
// truth); the only state here is this window's view of it. Back to chat in
// the call's own window brings the call back to the window that popped it
// out, as its stage with the same share in the middle.

import { useEffect, useMemo, useRef } from 'react'
import { createPortal } from 'react-dom'
import { isMacPlatform } from '@/lib/desktop-chrome'
import { isCallExpandChord, isCallWindow, noteCallConnected, takeCallHandBack } from '@/lib/voice/call-view'
import { getWindowLabel } from '@/lib/window-chrome'
import { toggleCallExpand, useCallExpand } from '@/stores/call-expand'
import { loadSharePick } from '@/stores/call-share'
import { useCallPopout, watchCallPopout } from '@/stores/call-popout'
import { callHasConversation, callPlaceKey, expandCallStage, useCallDocked, useCallPlace } from '@/stores/call-place'
import { noteCallWindowClosed, useIsCallHome, watchCallHome } from '@/stores/call-home'
import { useSettingsStore } from '@/stores/settings'
import { useVoice, voice } from '@/stores/voice'
import { video } from '@/stores/video'
import type { VoiceCall } from '@/stores/voice-core'
import type { CallPerson } from '@/lib/voice/video-labels'
import { useCallVideoLive } from './callHooks'
import VideoStage from './VideoStage'
import CallExpanded from './CallExpanded'
import RecordingNotice from './RecordingNotice'
import { callTitle } from '@/lib/home-chat/call-names'
import CallControls, { CallControlBar } from './CallControls'
import { CompactButton, FullScreenButton, PopOutButton, useFullScreenChordLabel } from './CallViewButtons'

const NO_PEOPLE: CallPerson[] = []

/** The call's video, as this window can show it: the call has video and
 *  isn't in its own window. */
export function useCallVideoHere(): boolean {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const preview = useSettingsStore((s) => s.videoRoomsPreview) === true
  const live = useCallVideoLive(video, call?.participants ?? NO_PEOPLE)
  const poppedOut = useCallPopout((s) => s.open)
  // One call, one home window: only the home draws the call's video.
  const home = useIsCallHome()
  return call !== null && native && preview && live && !poppedOut && home
}

/** The stage is up in this window, in the call's place (CallPlace): its
 *  conversation while that is on screen here, the page's top for a voice
 *  room. The top bar's call controls give way to the stage's own. */
export function useCallStageShown(): boolean {
  const here = useCallVideoHere()
  const call = useVoice((s) => s.call)
  const docked = useCallDocked(call)
  const compact = useCallPlace((s) => s.compact)
  const settingsOpen = useSettingsStore((s) => s.settingsOpen) === true
  // Full screen draws the video; the stage waits (one sink per track).
  const fullScreen = useCallExpand((s) => s.expanded)
  const placed = call !== null && (!callHasConversation(call) || docked)
  return here && placed && !compact && !settingsOpen && !fullScreen
}

/** The call's conversation is on screen in this window, with the call's
 *  stage or slim bar at its top: the top bar leaves the call to it. */
export function useCallInItsConversation(): boolean {
  const call = useVoice((s) => s.call)
  const native = useVoice((s) => s.engineKind) === 'native'
  const docked = useCallDocked(call)
  return call !== null && native && callHasConversation(call) && docked
}

/** This webview is the call's own window (it draws the call itself). */
export function inCallWindow(): boolean {
  return isCallWindow(window.location.hash, getWindowLabel())
}

/** The Full screen chord, while the call has video in this window. */
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
  setNoiseRemoval: (on: boolean) => voice.setNoiseRemoval(on),
}

/** The wave button's state: Settings → Voice → Noise removal (on unless
 *  turned off), with the native engine only; null on a seat-only call. */
export function useCallNoiseRemoval(): boolean | null {
  return useVoice((s) => (s.engineKind === 'native' ? s.prefs.noiseRemoval : null))
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

/** The stage's header: Pop out, Full screen, Compact. */
export function CallStageActions(): React.JSX.Element {
  const chord = useFullScreenChordLabel()
  return (
    <>
      <PopOutButton testId="video-stage-popout" />
      <FullScreenButton testId="video-stage-fullscreen" chord={chord} />
      <CompactButton testId="video-stage-compact" compact={false} />
    </>
  )
}

/** The call's stage, its controls on the call's bar (as full screen and the
 *  call's own window have them). */
export function CallStageView({ call, className }: { call: VoiceCall; className?: string }): React.JSX.Element {
  const noiseRemoval = useCallNoiseRemoval()
  return (
    <VideoStage
      ctl={video}
      people={call.participants}
      title={callTitle(call)}
      className={className}
      actions={<CallStageActions />}
      controls={
        <CallControlBar>
          <CallControls variant="overlay" call={call} actions={callActions} video={video} noiseRemoval={noiseRemoval} />
        </CallControlBar>
      }
    />
  )
}

function InCall({ call }: { call: VoiceCall }): React.JSX.Element | null {
  const noiseRemoval = useCallNoiseRemoval()
  const here = useCallVideoHere()
  const poppedOut = useCallPopout((s) => s.open)
  useCallExpandChord(here)
  // The call's window closed. Back to chat (not the OS's close) hands the
  // call back to the window that popped it out: its stage opens, and every
  // window takes up the share that was in the middle there.
  const wasOut = useRef(poppedOut)
  useEffect(() => {
    const was = wasOut.current
    wasOut.current = poppedOut
    if (!was || poppedOut) return
    // Closed by the OS too: the stage comes back without waiting out the
    // call window's lease.
    noteCallWindowClosed()
    loadSharePick(call.callId)
    if (takeCallHandBack(call.callId, getWindowLabel(), Date.now())) expandCallStage()
  }, [poppedOut, call.callId])
  // Which window is the call's home (one call, one home window). No window
  // left to show it: the call ends rather than going on unseen.
  const placeKey = callPlaceKey(call)
  useEffect(() => watchCallHome(placeKey, () => void voice.leave()), [placeKey])
  // The call ended (this unmounts): next call starts with its stage open.
  useEffect(() => () => expandCallStage(), [])
  // The call's own window counts its time from the same second.
  useEffect(() => {
    if (call.connectedAt !== null && call.callId) noteCallConnected(call.callId, call.connectedAt)
  }, [call.callId, call.connectedAt])

  // The stage itself is drawn in the call's place (CallPlace); full screen
  // is drawn here, over the window.
  if (!here) return null
  return <CallExpanded call={call} video={video} actions={callActions} noiseRemoval={noiseRemoval} />
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
